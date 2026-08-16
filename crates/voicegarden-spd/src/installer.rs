//! User-local installation of the module into speech-dispatcher.
//!
//! Layout (all user-writable, no root needed):
//!
//! ```text
//! ~/.local/libexec/speech-dispatcher-modules/sd_voicegarden        # module
//! ~/.local/libexec/speech-dispatcher-modules/voicegarden-spd-refresh
//! ~/.config/speech-dispatcher/modules/voicegarden-spd.conf         # config
//! ~/.config/speech-dispatcher/speechd.conf                         # AddModule line
//! ```
//!
//! `~/.local/libexec/speech-dispatcher-modules` is the user module
//! directory speech-dispatcher logs about at startup ("User module dir
//! is …"), so binaries land exactly where the daemon looks for them.

use std::path::{Path, PathBuf};

/// Where user-installed module binaries live.
#[must_use]
pub fn user_module_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    Path::new(&home).join(".local/libexec/speech-dispatcher-modules")
}

/// User speech-dispatcher config directory.
#[must_use]
pub fn user_speechd_config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    Path::new(&home).join(".config/speech-dispatcher")
}

/// Copy `src` to `dst`, creating parent dirs; preserve the executable bit.
fn install_file(src: &Path, dst: &Path) -> Result<(), String> {
    let bytes = std::fs::read(src).map_err(|e| format!("{}: {e}", src.display()))?;
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(dst, bytes).map_err(|e| format!("{}: {e}", dst.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(src)
            .map(|m| m.permissions().mode())
            .unwrap_or(0o755);
        std::fs::set_permissions(dst, std::fs::Permissions::from_mode(mode & 0o777))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Install the module + refresh helper and register with speechd.conf.
///
/// * `module_src` / `refresh_src` — binaries to install (typically from a
///   release tarball or `target/release`).
/// * `models_dir` — value to write as `ModelsDir` (defaults kept when
///   `None`).
///
/// Idempotent: existing files are overwritten; the AddModule line is
/// appended only once (an existing registration for this binary path is
/// replaced, not duplicated).
pub fn install(
    module_src: &Path,
    refresh_src: &Path,
    models_dir: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut steps = Vec::new();

    let module_dst = user_module_dir().join("sd_voicegarden");
    install_file(module_src, &module_dst)?;
    steps.push(format!("installed {}", module_dst.display()));

    let refresh_dst = user_module_dir().join("voicegarden-spd-refresh");
    install_file(refresh_src, &refresh_dst)?;
    steps.push(format!("installed {}", refresh_dst.display()));

    // Module config: start from the shipped sample, override ModelsDir.
    let conf_dir = user_speechd_config_dir().join("modules");
    std::fs::create_dir_all(&conf_dir).map_err(|e| e.to_string())?;
    let conf_dst = conf_dir.join("voicegarden-spd.conf");
    let sample = include_str!("../../../config/voicegarden-spd.conf");
    let conf_text = match models_dir {
        Some(dir) => {
            // Replace the commented default with an explicit directive.
            sample.replace(
                "ModelsDir \"$HOME/.rust-tts-wrapper/sherpaonnx\"",
                &format!("ModelsDir \"{dir}\""),
            )
        }
        None => sample.to_string(),
    };
    std::fs::write(&conf_dst, conf_text).map_err(|e| e.to_string())?;
    steps.push(format!("wrote {}", conf_dst.display()));

    // Register in the user's speechd.conf.
    let speechd_conf = user_speechd_config_dir().join("speechd.conf");
    std::fs::create_dir_all(user_speechd_config_dir()).map_err(|e| e.to_string())?;
    let existing = std::fs::read_to_string(&speechd_conf).unwrap_or_default();
    let line = format!(
        "AddModule \"voicegarden-spd\" \"{}\" \"voicegarden-spd.conf\"",
        module_dst.display()
    );
    let already = existing
        .lines()
        .any(|l| l.trim_start().starts_with("AddModule") && l.contains("\"voicegarden-spd\""));
    let updated = if already {
        // Replace the existing voicegarden AddModule line (path may have
        // changed between installs).
        existing
            .lines()
            .map(|l| {
                if l.trim_start().starts_with("AddModule") && l.contains("\"voicegarden-spd\"") {
                    line.as_str()
                } else {
                    l
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    } else {
        let mut text = existing.clone();
        if !text.ends_with('\n') && !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&line);
        text.push('\n');
        text
    };
    if updated != existing {
        std::fs::write(&speechd_conf, updated)
            .map_err(|e| format!("{}: {e}", speechd_conf.display()))?;
        steps.push(format!("registered in {}", speechd_conf.display()));
    } else {
        steps.push(format!("already registered in {}", speechd_conf.display()));
    }

    Ok(steps)
}

/// Remove installed files and the AddModule registration.
pub fn uninstall() -> Result<Vec<String>, String> {
    let mut steps = Vec::new();
    for name in ["sd_voicegarden", "voicegarden-spd-refresh"] {
        let p = user_module_dir().join(name);
        if p.exists() && std::fs::remove_file(&p).is_ok() {
            steps.push(format!("removed {}", p.display()));
        }
    }
    let conf = user_speechd_config_dir().join("modules/voicegarden-spd.conf");
    if conf.exists() && std::fs::remove_file(&conf).is_ok() {
        steps.push(format!("removed {}", conf.display()));
    }
    let speechd_conf = user_speechd_config_dir().join("speechd.conf");
    if let Ok(existing) = std::fs::read_to_string(&speechd_conf) {
        let kept: Vec<&str> = existing
            .lines()
            .filter(|l| !(l.contains("AddModule") && l.contains("\"voicegarden-spd\"")))
            .collect();
        let updated = kept.join("\n") + "\n";
        if updated != existing {
            std::fs::write(&speechd_conf, updated).map_err(|e| e.to_string())?;
            steps.push(format!("unregistered from {}", speechd_conf.display()));
        }
    }
    Ok(steps)
}

/// Installation status snapshot (for `status` and humans).
pub struct Status {
    pub module_installed: Option<PathBuf>,
    pub refresh_installed: Option<PathBuf>,
    pub config_installed: Option<PathBuf>,
    pub registered: bool,
    pub local_voices: usize,
    pub cloud_voices: usize,
}

#[must_use]
pub fn status(cfg: &crate::config::ModuleConfig) -> Status {
    let module = user_module_dir().join("sd_voicegarden");
    let refresh = user_module_dir().join("voicegarden-spd-refresh");
    let conf = user_speechd_config_dir().join("modules/voicegarden-spd.conf");
    let registered = std::fs::read_to_string(user_speechd_config_dir().join("speechd.conf"))
        .map(|t| {
            t.lines()
                .any(|l| l.contains("AddModule") && l.contains("\"voicegarden-spd\""))
        })
        .unwrap_or(false);

    let local = crate::voices::local_sherpa_voices(&cfg.models_dirs(), cfg.num_threads);
    let cache = crate::voices::load_voice_cache(&cfg.voice_cache_file);
    let creds = crate::voices::load_credentials(&cfg.credentials_file);
    let cloud = crate::voices::cloud_voices(&cache, &creds);

    Status {
        module_installed: module.exists().then_some(module),
        refresh_installed: refresh.exists().then_some(refresh),
        config_installed: conf.exists().then_some(conf),
        registered,
        local_voices: local.len(),
        cloud_voices: cloud.len(),
    }
}

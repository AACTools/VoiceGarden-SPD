//! User-local installation of the module into speech-dispatcher.
//!
//! Layout (all user-writable, no root needed):
//!
//! ```text
//! ~/.local/libexec/speech-dispatcher-modules/sd_voicegarden-spd     # module
//! ~/.local/libexec/speech-dispatcher-modules/voicegarden-spd-refresh
//! ~/.config/speech-dispatcher/modules/voicegarden-spd.conf          # config
//! ```
//!
//! `~/.local/libexec/speech-dispatcher-modules` is the user module
//! directory speech-dispatcher scans at startup, so the module is
//! **auto-detected** under the name `voicegarden-spd` (derived from the
//! `sd_voicegarden-spd` binary). No user `speechd.conf` is written:
//! speech-dispatcher only auto-detects while *no* `AddModule` line is
//! configured, so a user `speechd.conf` containing just our registration
//! would shadow the system config and drop every other output module
//! from the session (issue #2). An explicit `AddModule` line is only
//! managed when the user's `speechd.conf` already registers other
//! modules (auto-detection already off for them).

use std::path::{Path, PathBuf};

/// Name of the module binary. Named so speech-dispatcher's
/// auto-detection derives the module name `voicegarden-spd` from it.
pub const MODULE_BIN: &str = "sd_voicegarden-spd";

/// Module name as seen by speech-dispatcher clients (`spd-say -o …`).
pub const MODULE_NAME: &str = "voicegarden-spd";

/// Binary name used by installs ≤ v0.3.0 (auto-detected as `voicegarden`).
const LEGACY_MODULE_BIN: &str = "sd_voicegarden";

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

/// Install the module + refresh helper and make speech-dispatcher load
/// them.
///
/// * `module_src` / `refresh_src` — binaries to install (typically from a
///   release tarball or `target/release`).
/// * `models_dir` — value to write as `ModelsDir` (defaults kept when
///   `None`).
///
/// Idempotent: existing files are overwritten; the legacy
/// `sd_voicegarden` binary from installs ≤ v0.3.0 is removed; when an
/// explicit `AddModule` line is being managed it is appended only once
/// (an existing registration for this binary path is replaced, not
/// duplicated).
pub fn install(
    module_src: &Path,
    refresh_src: &Path,
    models_dir: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut steps = Vec::new();

    let module_dst = user_module_dir().join(MODULE_BIN);
    install_file(module_src, &module_dst)?;
    steps.push(format!("installed {}", module_dst.display()));

    // Installs ≤ v0.3.0 left a sd_voicegarden binary behind; it would be
    // auto-detected as a second, stale module.
    let legacy = user_module_dir().join(LEGACY_MODULE_BIN);
    if legacy.exists() && std::fs::remove_file(&legacy).is_ok() {
        steps.push(format!(
            "removed legacy {} (renamed to {})",
            legacy.display(),
            MODULE_BIN
        ));
    }

    let refresh_dst = user_module_dir().join("voicegarden-spd-refresh");
    install_file(refresh_src, &refresh_dst)?;
    steps.push(format!("installed {}", refresh_dst.display()));

    // speech-dispatcher resolves its user module directory as
    // `~/.local/share/../libexec/speech-dispatcher-modules`. The `..` hop
    // needs `~/.local/share` to exist — on a fresh HOME (CI containers,
    // headless users) it doesn't, and auto-detection silently skips the
    // whole user module directory. Creating it is harmless either way.
    let share_dir = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share");
    if !share_dir.is_dir() && std::fs::create_dir_all(&share_dir).is_ok() {
        steps.push(format!(
            "created {} (speech-dispatcher's user module dir path hops through it)",
            share_dir.display()
        ));
    }

    // Module config: start from the shipped sample, override ModelsDir.
    let conf_dir = user_speechd_config_dir().join("modules");
    std::fs::create_dir_all(&conf_dir).map_err(|e| e.to_string())?;
    let conf_dst = conf_dir.join("voicegarden-spd.conf");
    let sample = include_str!("../../../config/voicegarden-spd.conf");
    let conf_text = match models_dir {
        Some(dir) => {
            // Replace the commented default with an explicit directive.
            sample.replace(
                "ModelsDir \"$HOME/.local/share/voicegarden/sherpa-onnx-models\"",
                &format!("ModelsDir \"{dir}\""),
            )
        }
        None => sample.to_string(),
    };
    std::fs::write(&conf_dst, conf_text).map_err(|e| e.to_string())?;
    steps.push(format!("wrote {}", conf_dst.display()));

    steps.extend(register_with_speechd(&module_dst)?);

    Ok(steps)
}

/// What `install` should do about the user's `speechd.conf`.
#[derive(Debug, PartialEq, Eq)]
enum RegistrationPlan {
    /// No user speechd.conf exists (or none is needed): the module is
    /// auto-detected from the user module directory.
    AutoDetect,
    /// The file registers other modules already; manage our explicit
    /// `AddModule` line in it.
    ManageLine,
    /// The file exists but registers no other module. Our line (if any)
    /// must go: auto-detection takes over. `keep_settings` = the file has
    /// other active directives and must be kept (minus our line);
    /// otherwise it should be deleted outright.
    DropLine { keep_settings: bool },
}

/// Decide how to register, given the current user `speechd.conf` content
/// (`None` = file absent).
fn plan_registration(existing: Option<&str>) -> RegistrationPlan {
    let Some(existing) = existing else {
        return RegistrationPlan::AutoDetect;
    };
    let foreign = existing
        .lines()
        .any(|l| is_addmodule(l) && !is_our_addmodule(l));
    if foreign {
        return RegistrationPlan::ManageLine;
    }
    let cleaned = strip_our_lines(existing);
    let keep_settings = cleaned.lines().any(is_active_directive);
    RegistrationPlan::DropLine { keep_settings }
}

/// True for a non-comment, non-blank line (an active speechd directive).
fn is_active_directive(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && !t.starts_with('#')
}

/// True for an `AddModule` directive line (not commented out).
fn is_addmodule(line: &str) -> bool {
    line.trim_start().starts_with("AddModule")
}

/// True for our `AddModule "voicegarden-spd" …` registration line.
fn is_our_addmodule(line: &str) -> bool {
    is_addmodule(line) && line.contains("\"voicegarden-spd\"")
}

/// Remove our AddModule lines from `text`.
fn strip_our_lines(text: &str) -> String {
    text.lines()
        .filter(|l| !is_our_addmodule(l))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// Apply [`RegistrationPlan`] against the real user config.
fn register_with_speechd(module_dst: &Path) -> Result<Vec<String>, String> {
    let speechd_conf = user_speechd_config_dir().join("speechd.conf");
    std::fs::create_dir_all(user_speechd_config_dir()).map_err(|e| e.to_string())?;
    let existing = std::fs::read_to_string(&speechd_conf).ok();

    match plan_registration(existing.as_deref()) {
        RegistrationPlan::AutoDetect => {
            Ok(vec!["module will be auto-detected by speech-dispatcher \
             (no speechd.conf written — an AddModule line there would \
             disable auto-detection of other modules)"
                .to_string()])
        }
        RegistrationPlan::ManageLine => {
            let existing = existing.unwrap_or_default();
            let line = format!(
                "AddModule \"{}\" \"{}\" \"voicegarden-spd.conf\"",
                MODULE_NAME,
                module_dst.display()
            );
            let updated = if existing.lines().any(is_our_addmodule) {
                // Replace the existing voicegarden AddModule line (path
                // may have changed between installs).
                existing
                    .lines()
                    .map(|l| {
                        if is_our_addmodule(l) {
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
                Ok(vec![format!(
                    "registered in {} (file already lists other modules)",
                    speechd_conf.display()
                )])
            } else {
                Ok(vec![format!(
                    "already registered in {}",
                    speechd_conf.display()
                )])
            }
        }
        RegistrationPlan::DropLine { keep_settings } => {
            // The file registers no foreign module: any AddModule line of
            // ours would turn off auto-detection and drop every other
            // output module from the session (issue #2) — including the
            // broken one-liner written by installs ≤ v0.3.0.
            if keep_settings {
                let cleaned = strip_our_lines(&existing.unwrap_or_default());
                std::fs::write(&speechd_conf, cleaned)
                    .map_err(|e| format!("{}: {e}", speechd_conf.display()))?;
                Ok(vec![format!(
                    "unregistered from {} — the module is auto-detected \
                     alongside other modules (kept your other settings)",
                    speechd_conf.display()
                )])
            } else if speechd_conf.exists() && std::fs::remove_file(&speechd_conf).is_ok() {
                Ok(vec![format!(
                    "removed {} (no other active settings) — the module \
                     is auto-detected alongside other modules",
                    speechd_conf.display()
                )])
            } else {
                Ok(vec![
                    "module will be auto-detected by speech-dispatcher".to_string()
                ])
            }
        }
    }
}

/// Remove installed files and the AddModule registration.
pub fn uninstall() -> Result<Vec<String>, String> {
    let mut steps = Vec::new();
    for name in [MODULE_BIN, LEGACY_MODULE_BIN, "voicegarden-spd-refresh"] {
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
        if existing.lines().any(is_our_addmodule) {
            let cleaned = strip_our_lines(&existing);
            if cleaned.lines().any(is_active_directive) {
                std::fs::write(&speechd_conf, cleaned).map_err(|e| e.to_string())?;
                steps.push(format!("unregistered from {}", speechd_conf.display()));
            } else if std::fs::remove_file(&speechd_conf).is_ok() {
                // Nothing active left: restore auto-detection + the
                // system config wholesale.
                steps.push(format!(
                    "removed {} — module auto-detection restored",
                    speechd_conf.display()
                ));
            }
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
    /// How the daemon loads us: auto-detection vs explicit AddModule.
    pub registration_mode: &'static str,
    pub local_voices: usize,
    pub cloud_voices: usize,
}

#[must_use]
pub fn status(cfg: &crate::config::ModuleConfig) -> Status {
    let module = user_module_dir().join(MODULE_BIN);
    let refresh = user_module_dir().join("voicegarden-spd-refresh");
    let conf = user_speechd_config_dir().join("modules/voicegarden-spd.conf");
    let explicit = std::fs::read_to_string(user_speechd_config_dir().join("speechd.conf"))
        .map(|t| t.lines().any(is_our_addmodule))
        .unwrap_or(false)
        || std::fs::read_to_string("/etc/speech-dispatcher/speechd.conf")
            .map(|t| t.lines().any(is_our_addmodule))
            .unwrap_or(false);
    let module_installed = module.exists().then_some(module);
    // Auto-detection loads the module whenever the binary sits in a
    // module directory; explicit registration is the fallback.
    let registered = explicit || module_installed.is_some();

    let local = crate::voices::local_voices(&cfg.models_dirs(), cfg.num_threads, &cfg.local_engine);
    let cache = crate::voices::load_voice_cache(&cfg.voice_cache_file);
    let creds = crate::voices::load_credentials(&cfg.credentials_file);
    let cloud = crate::voices::cloud_voices(&cache, &creds);

    Status {
        module_installed,
        refresh_installed: refresh.exists().then_some(refresh),
        config_installed: conf.exists().then_some(conf),
        registered,
        registration_mode: if explicit {
            "AddModule line"
        } else {
            "auto-detected from the module directory"
        },
        local_voices: local.len(),
        cloud_voices: cloud.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_absent_file_uses_autodetect() {
        assert_eq!(plan_registration(None), RegistrationPlan::AutoDetect);
    }

    #[test]
    fn plan_foreign_modules_manage_line() {
        let conf = "LogLevel 5\nAddModule \"espeak-ng\" \"sd_espeak-ng\" \"espeak-ng.conf\"\n";
        assert_eq!(plan_registration(Some(conf)), RegistrationPlan::ManageLine);
    }

    #[test]
    fn plan_broken_one_liner_from_v030_is_dropped() {
        // Exactly what installs ≤ v0.3.0 wrote: the file that broke other
        // modules (issue #2).
        let conf = "AddModule \"voicegarden-spd\" \"/home/u/.local/libexec/speech-dispatcher-modules/sd_voicegarden\" \"voicegarden-spd.conf\"\n";
        assert_eq!(
            plan_registration(Some(conf)),
            RegistrationPlan::DropLine {
                keep_settings: false
            }
        );
    }

    #[test]
    fn plan_ours_plus_settings_keeps_settings() {
        let conf = "LogLevel 4\nAddModule \"voicegarden-spd\" \"x\" \"voicegarden-spd.conf\"\n";
        assert_eq!(
            plan_registration(Some(conf)),
            RegistrationPlan::DropLine {
                keep_settings: true
            }
        );
    }

    #[test]
    fn plan_settings_only_file_stays_autodetect_side() {
        let conf = "# my settings\nLogLevel 3\n";
        assert_eq!(
            plan_registration(Some(conf)),
            RegistrationPlan::DropLine {
                keep_settings: true
            }
        );
        // and stripping is a no-op on it
        assert_eq!(strip_our_lines(conf), conf);
    }

    #[test]
    fn strip_removes_only_our_lines() {
        let conf = "AddModule \"espeak-ng\" \"sd_espeak-ng\" \"espeak-ng.conf\"\n\
                    AddModule \"voicegarden-spd\" \"x\" \"voicegarden-spd.conf\"\n\
                    #AddModule \"voicegarden-spd\" \"commented\"\nLogLevel 2\n";
        let cleaned = strip_our_lines(conf);
        assert!(cleaned.contains("espeak-ng"));
        assert!(cleaned.contains("#AddModule \"voicegarden-spd\" \"commented\""));
        assert!(cleaned.contains("LogLevel 2"));
        assert!(!cleaned.contains("\"voicegarden-spd\" \"x\""));
    }

    #[test]
    fn commented_addmodule_is_not_active() {
        assert!(is_addmodule("AddModule \"x\""));
        assert!(!is_addmodule("#AddModule \"x\""));
        assert!(!is_addmodule("  # AddModule \"x\""));
        assert!(is_active_directive(" AddModule \"x\""));
        assert!(!is_active_directive("# comment"));
        assert!(!is_active_directive(""));
    }
}

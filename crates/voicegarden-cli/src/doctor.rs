//! `doctor` — environment diagnostics, ported from the original CLI.
//! `check-model` is a hidden subcommand spawned per-model so ONNX runtime
//! aborts (uncatchable foreign exceptions) can't kill the doctor itself.

use std::process::ExitCode;

use clap::Subcommand;
use voicegarden_spd::config::ModuleConfig;
use voicegarden_spd::voices::merged_voices;

use crate::Style;

#[derive(Subcommand)]
pub(crate) enum DoctorCmd {
    /// Diagnose a broken setup
    Run,
}

pub(crate) fn run() -> Result<(), String> {
    doctor_impl()
}

struct Doctor {
    failures: usize,
}

impl Doctor {
    fn check(&mut self, ok: bool, label: &str, detail_ok: &str, detail_bad: &str) {
        if ok {
            println!("  [ok]   {label}: {detail_ok}");
        } else {
            println!("  [FAIL] {label}: {detail_bad}");
            self.failures += 1;
        }
    }
    fn info(&mut self, label: &str, detail: &str) {
        println!("  [--]   {label}: {detail}");
    }
}

fn doctor_impl() -> Result<(), String> {
    println!("voicegarden-spd {} — doctor", env!("CARGO_PKG_VERSION"));
    println!();
    let mut d = Doctor { failures: 0 };

    // 1. speech-dispatcher version
    // ("speech-dispatcher 0.12.1" — first line of --version stdout)
    let sd_version = std::process::Command::new("speech-dispatcher")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| {
            let text = String::from_utf8_lossy(&o.stdout);
            text.lines().next().and_then(|l| {
                l.split_whitespace()
                    .find(|t| t.chars().next().is_some_and(char::is_numeric))
                    .map(str::to_string)
            })
        })
        .unwrap_or_default();
    let sd_installed = !sd_version.is_empty();
    if !sd_installed {
        d.check(
            false,
            "speech-dispatcher",
            "",
            "not found — install your distro's speech-dispatcher package first",
        );
    } else {
        let ok = version_at_least(&sd_version, 0, 12);
        d.check(
            ok,
            "speech-dispatcher version",
            &sd_version,
            &format!(
                "{sd_version} — 0.12+ required (server-side audio). Debian 13/Ubuntu 25.04/Fedora 41+ ship it; older releases cannot play module audio"
            ),
        );
    }

    // 2. daemon socket
    let uid = libc_getuid();
    let sock = std::path::Path::new("/run/user")
        .join(uid.to_string())
        .join("speech-dispatcher/speechd.sock");
    let sock_ok = sock.exists();
    if sd_installed {
        d.check(
            sock_ok,
            "daemon socket",
            &sock.display().to_string(),
            "not found — start it: systemctl --user start speech-dispatcher.socket, or open a desktop session",
        );
    }

    // 3. module binary + registration
    let module_paths = [
        voicegarden_spd::installer::user_module_dir().join(voicegarden_spd::installer::MODULE_BIN),
        std::path::PathBuf::from(
            "/usr/lib/x86_64-linux-gnu/speech-dispatcher-modules/sd_voicegarden-spd",
        ),
        std::path::PathBuf::from(
            "/usr/lib/aarch64-linux-gnu/speech-dispatcher-modules/sd_voicegarden-spd",
        ),
        std::path::PathBuf::from("/usr/lib64/speech-dispatcher-modules/sd_voicegarden-spd"),
    ];
    let module_path = module_paths.iter().find(|p| p.exists());
    d.check(
        module_path.is_some(),
        "module binary",
        &module_path.map_or(String::new(), |p| p.display().to_string()),
        "not found — run `voicegarden-spd install` (user-local) or install the .deb/.rpm",
    );

    // Registration: an explicit AddModule line, or auto-detection from a
    // module directory (speech-dispatcher derives the module name from
    // the sd_voicegarden-spd binary).
    let speechd_conf = voicegarden_spd::installer::user_speechd_config_dir().join("speechd.conf");
    let registered = |p: &std::path::Path| {
        std::fs::read_to_string(p).map(|t| {
            t.lines()
                .any(|l| l.contains("AddModule") && l.contains("\"voicegarden-spd\""))
        })
    };
    let (reg_user, reg_system) = (
        registered(&speechd_conf).unwrap_or(false),
        registered(std::path::Path::new("/etc/speech-dispatcher/speechd.conf")).unwrap_or(false),
    );
    if reg_user || reg_system {
        let where_ = if reg_user {
            speechd_conf.display().to_string()
        } else {
            "/etc/speech-dispatcher/speechd.conf".to_string()
        };
        d.check(true, "registration", &format!("AddModule in {where_}"), "");
    } else if module_path.is_some() {
        d.check(
            true,
            "registration",
            "auto-detected by speech-dispatcher from the module directory",
            "",
        );
    } else {
        d.check(
            false,
            "registration",
            "",
            &format!(
                "no AddModule line in {} or /etc/speech-dispatcher/speechd.conf and no module \
                 binary in a module directory — run `voicegarden-spd install` or reinstall the \
                 package",
                speechd_conf.display()
            ),
        );
    }

    // 4. voice inventory + per-model validation
    let cfg = ModuleConfig::load(None);
    let voices = merged_voices(&cfg);
    let local = voices
        .iter()
        .filter(|v| v.engine_id == "sherpaonnx")
        .count();
    let cloud = voices.len() - local;
    d.info(
        "voices",
        &format!(
            "{local} local ({}), {cloud} cloud{}",
            cfg.models_dir.display(),
            if cloud == 0 {
                " (run `engine add <engine>` or `refresh` for cloud voices)"
            } else {
                ""
            }
        ),
    );
    if voices.is_empty() {
        d.check(
            false,
            "voice availability",
            "",
            "no voices at all — put a sherpa-onnx model under the models dir (see `model find`), or run `engine add <engine>`",
        );
    } else {
        doctor_check_models(&mut d, &cfg);
    }

    // 5. daemon-side check
    if sock_ok {
        match std::process::Command::new("spd-say")
            .args(["-o", "voicegarden-spd", "-L"])
            .output()
        {
            Ok(o) if o.status.success() => {
                let n = String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .count()
                    .saturating_sub(1);
                d.check(
                    true,
                    "daemon view (spd-say -o voicegarden-spd -L)",
                    &format!("{n} voices"),
                    "",
                );
            }
            _ => {
                d.check(
                    false,
                    "daemon view (spd-say -o voicegarden-spd -L)",
                    "",
                    &format!(
                        "spd-say failed — inspect /run/user/{uid}/speech-dispatcher/log/voicegarden-spd.log and speech-dispatcher.log"
                    ),
                );
            }
        }
    }

    println!();
    if d.failures == 0 {
        println!("All checks passed.");
    } else {
        println!("{} problem(s) found — see above.", d.failures);
    }
    println!("Module log: /run/user/{uid}/speech-dispatcher/log/voicegarden-spd.log");
    if d.failures > 0 {
        Err(String::new()) // message already printed above
    } else {
        Ok(())
    }
}

/// Per-model load + synthesis validation (subprocess-isolated).
fn doctor_check_models(d: &mut Doctor, cfg: &ModuleConfig) {
    use std::collections::BTreeSet;
    let models: BTreeSet<String> = merged_voices(cfg)
        .into_iter()
        .filter(|v| v.engine_id == "sherpaonnx" && v.engine_voice_id == "0")
        .map(|v| v.spd_name)
        .collect();
    if models.is_empty() {
        return;
    }
    println!();
    println!("  model load + synthesis checks (~2 s per model):");
    let exe = std::env::current_exe().unwrap_or_else(|_| "voicegarden-spd".into());
    for model in &models {
        let out = std::process::Command::new(&exe)
            .args(["check-model", model])
            .output();
        match out {
            Ok(o) if o.status.success() => {
                let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
                println!("  [ok]   {model}: {stdout}");
            }
            Ok(o) => {
                let code = o.status.code();
                let signal = {
                    #[cfg(unix)]
                    {
                        use std::os::unix::process::ExitStatusExt;
                        o.status.signal()
                    }
                    #[cfg(not(unix))]
                    {
                        None
                    }
                };
                let detail =
                    if signal.is_some() || matches!(code, Some(134) | Some(135) | Some(132)) {
                        "ONNX runtime aborted — the model archive is corrupt, an \
                     unsupported variant (e.g. fp16 on this runtime), or its files \
                     are missing. Redownload the model or pick another variant; \
                     every utterance through it will fail until then."
                            .to_string()
                    } else {
                        let err = String::from_utf8_lossy(&o.stderr);
                        format!(
                            "check failed (exit {}): {}",
                            code.map_or("killed by signal".to_string(), |c| c.to_string()),
                            err.lines().last().unwrap_or("no detail")
                        )
                    };
                println!("  [FAIL] {model}: {detail}");
                d.failures += 1;
            }
            Err(e) => {
                println!("  [FAIL] {model}: could not spawn check: {e}");
                d.failures += 1;
            }
        }
    }
}

/// `check-model` implementation: synthesise through one local voice with
/// no playback. Prints `OK <bytes>`; any failure exits non-zero.
pub(crate) fn check_model(voice_name: &str) -> Result<(), String> {
    let cfg = ModuleConfig::load(None);
    let v = merged_voices(&cfg)
        .into_iter()
        .find(|v| v.spd_name == voice_name && v.engine_id == "sherpaonnx")
        .ok_or_else(|| format!("voice '{voice_name}' not found"))?;

    let engine = rust_tts_wrapper::create_engine(&v.engine_id, &v.credentials)
        .ok_or_else(|| format!("engine '{}' unavailable", v.engine_id))?;
    let mut total = 0usize;
    engine
        .speak_sync(
            "one two three",
            Some(&v.engine_voice_id),
            1.0,
            1.0,
            1.0,
            Some(&mut |chunk: &[u8]| total += chunk.len()),
            None,
        )
        .map_err(|e| format!("synthesis failed: {e}"))?;
    if total == 0 {
        return Err("synthesis produced no audio".into());
    }
    println!("OK {total}");
    Ok(())
}

fn version_at_least(v: &str, major: u32, minor: u32) -> bool {
    let mut it = v.split('.');
    let maj: u32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let min: u32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (maj, min) >= (major, minor)
}

/// libc::getuid without a direct libc dependency in this crate.
fn libc_getuid() -> u32 {
    std::fs::metadata("/proc/self")
        .ok()
        .map(|m| {
            use std::os::unix::fs::MetadataExt;
            m.uid()
        })
        .unwrap_or(0)
}

/// Entry point for the hidden `check-model` subcommand.
pub(crate) fn check_model_main(voice: String) -> ExitCode {
    match check_model(&voice) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Suppress unused-warning for Style (doctor output is intentionally plain).
#[allow(dead_code)]
fn _style_marker(_s: &Style) {}

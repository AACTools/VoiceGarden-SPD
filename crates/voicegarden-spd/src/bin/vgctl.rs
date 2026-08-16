//! voicegarden-spd — management CLI for the VoiceGarden speech-dispatcher
//! module (install / uninstall / status / refresh / voices / speak).
//!
//! The headless companion to the module; a GTK config app is planned on
//! top of the same library calls (see README roadmap).

use std::path::PathBuf;
use std::process::ExitCode;

use voicegarden_spd::config::ModuleConfig;
use voicegarden_spd::installer;
use voicegarden_spd::voices::cloud_pcm_rate;

const USAGE: &str = "\
voicegarden-spd — manage the VoiceGarden speech-dispatcher module

Usage:
  voicegarden-spd install [--models-dir DIR] [--no-restart]
                                            Install module + register (user-local, no root)
  voicegarden-spd uninstall [--no-restart]  Remove installed files + registration
  voicegarden-spd status                    Installation + voice inventory
  voicegarden-spd doctor                    Diagnose a broken setup
  voicegarden-spd refresh [--config FILE]   Refresh the cloud voice cache
  voicegarden-spd voices [--config FILE]    List all merged voices
  voicegarden-spd speak <voice> <text>      Speak once through rust-tts-wrapper directly
  voicegarden-spd bench <voice> [text] [N]  Cold + warm synthesis timings (no playback)
  voicegarden-spd migrate-models            Move legacy models into the primary models dir
  voicegarden-spd --version

Environment:
  VOICEGARDEN_ENGINES_JSON   Override credentials file for refresh
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    };

    let result = match cmd {
        "--version" | "-V" => {
            println!("voicegarden-spd {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "install" => cmd_install(&args[1..]),
        "uninstall" => cmd_uninstall(&args[1..]),
        "status" => cmd_status(),
        "doctor" => cmd_doctor(),
        "refresh" => cmd_refresh(&args[1..]),
        "voices" => cmd_voices(&args[1..]),
        "speak" => cmd_speak(&args[1..]),
        "bench" => cmd_bench(&args[1..]),
        "migrate-models" => cmd_migrate_models(),
        "--help" | "-h" => {
            print!("{USAGE}");
            Ok(())
        }
        other => {
            eprintln!("unknown command: {other}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("voicegarden-spd: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Locate our own binaries: beside this executable first (release
/// tarball / target layout — what `install` should deploy), then the
/// installed copy (re-registration), so re-installing over an existing
/// install never copies the old binary onto itself.
fn find_binary(name: &str) -> Result<PathBuf, String> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(name)));
    let candidates = [exe_dir, Some(installer::user_module_dir().join(name))];
    candidates
        .iter()
        .flatten()
        .find(|p| p.is_file())
        .cloned()
        .ok_or_else(|| {
            format!(
                "cannot locate '{name}' — run from the release tarball or target/, \
                 or install first"
            )
        })
}

fn cmd_install(args: &[String]) -> Result<(), String> {
    let mut models_dir = None;
    let mut restart = true;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--models-dir" => {
                i += 1;
                models_dir = Some(args.get(i).ok_or("--models-dir requires a value")?.clone());
            }
            "--no-restart" => restart = false,
            other => return Err(format!("unknown argument: {other}")),
        }
        i += 1;
    }

    let module = find_binary("sd_voicegarden")?;
    let refresh = find_binary("voicegarden-spd-refresh")?;
    let steps = installer::install(&module, &refresh, models_dir.as_deref())?;
    for s in &steps {
        println!("  {s}");
    }
    if restart && restart_speechd() {
        println!("  restarted speech-dispatcher");
    } else if restart {
        println!();
        println!("Restart speech-dispatcher to activate:");
        println!("  systemctl --user restart speech-dispatcher.service  (or re-login)");
    }
    println!();
    println!("Test:  spd-say -o voicegarden-spd \"Hello from VoiceGarden\"");
    println!("Setup: voicegarden-spd doctor   (troubleshooting)");
    Ok(())
}

fn cmd_uninstall(args: &[String]) -> Result<(), String> {
    let restart = !args.iter().any(|a| a == "--no-restart");
    let steps = installer::uninstall()?;
    if steps.is_empty() {
        println!("nothing installed — no changes made");
    }
    for s in &steps {
        println!("  {s}");
    }
    if restart {
        restart_speechd();
    }
    Ok(())
}

/// Restart the user's speech-dispatcher so config changes take effect.
/// Returns true when a restart was actually performed.
fn restart_speechd() -> bool {
    let status = std::process::Command::new("systemctl")
        .args(["--user", "is-active", "speech-dispatcher.service"])
        .output();
    let active = status
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "active")
        .unwrap_or(false);
    if !active {
        // Not running inside a user session (root install, SSH without
        // linger): the daemon starts on the user's next login anyway.
        return false;
    }
    std::process::Command::new("systemctl")
        .args(["--user", "restart", "speech-dispatcher.service"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Numbered check-list output helper.
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

fn cmd_doctor() -> Result<(), String> {
    println!("voicegarden-spd {} — doctor", env!("CARGO_PKG_VERSION"));
    println!();
    let mut d = Doctor { failures: 0 };

    // 1. Is speech-dispatcher installed and new enough?
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

    // 2. Is the daemon reachable?
    let uid = unsafe { libc::getuid() };
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

    // 3. Module binary + registration
    let module_paths = [
        installer::user_module_dir().join("sd_voicegarden"),
        std::path::PathBuf::from(
            "/usr/lib/x86_64-linux-gnu/speech-dispatcher-modules/sd_voicegarden",
        ),
        std::path::PathBuf::from(
            "/usr/lib/aarch64-linux-gnu/speech-dispatcher-modules/sd_voicegarden",
        ),
        std::path::PathBuf::from("/usr/lib64/speech-dispatcher-modules/sd_voicegarden"),
    ];
    let module_path = module_paths.iter().find(|p| p.exists());
    d.check(
        module_path.is_some(),
        "module binary",
        &module_path.map_or(String::new(), |p| p.display().to_string()),
        "not found — run `voicegarden-spd install` (user-local) or install the .deb/.rpm",
    );

    let speechd_conf = installer::user_speechd_config_dir().join("speechd.conf");
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
    } else {
        d.check(
            false,
            "registration",
            "",
            &format!(
                "no AddModule line in {} or /etc/speech-dispatcher/speechd.conf — run `voicegarden-spd install` or reinstall the package",
                speechd_conf.display()
            ),
        );
    }

    // 4. Voice inventory
    let cfg = ModuleConfig::load(None);
    let local = voicegarden_spd::voices::local_sherpa_voices(&cfg.models_dirs(), cfg.num_threads);
    let credentials = voicegarden_spd::voices::load_credentials(&cfg.credentials_file);
    let cache = voicegarden_spd::voices::load_voice_cache(&cfg.voice_cache_file);
    let cloud = voicegarden_spd::voices::cloud_voices(&cache, &credentials);
    d.info(
        "voices",
        &format!(
            "{} local ({}), {} cloud{}",
            local.len(),
            cfg.models_dir.display(),
            cloud.len(),
            if cloud.is_empty() {
                " (run `voicegarden-spd refresh` for cloud voices)"
            } else {
                ""
            }
        ),
    );
    if local.is_empty() && cloud.is_empty() {
        d.check(
            false,
            "voice availability",
            "",
            "no voices at all — put a sherpa-onnx model under the models dir (README) or configure cloud credentials and run refresh",
        );
    }

    // 5. Daemon-side check: can spd-say see the module?
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
    Ok(())
}

/// Parse a `0.12.1`-style version and compare against (major, minor).
fn version_at_least(v: &str, major: u32, minor: u32) -> bool {
    let mut it = v.split('.');
    let maj: u32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let min: u32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (maj, min) >= (major, minor)
}

fn cmd_status() -> Result<(), String> {
    let cfg = ModuleConfig::load(None);
    let st = installer::status(&cfg);
    println!("VoiceGarden-SPD {}", env!("CARGO_PKG_VERSION"));
    println!();
    println!(
        "  module binary : {}",
        st.module_installed
            .as_ref()
            .map_or("-".into(), |p| p.display().to_string())
    );
    println!(
        "  refresh tool  : {}",
        st.refresh_installed
            .as_ref()
            .map_or("-".into(), |p| p.display().to_string())
    );
    println!(
        "  module config : {}",
        st.config_installed
            .as_ref()
            .map_or("-".into(), |p| p.display().to_string())
    );
    println!(
        "  registered    : {}",
        if st.registered {
            "yes (AddModule in ~/.config/speech-dispatcher/speechd.conf)"
        } else {
            "no"
        }
    );
    println!();
    println!("  models dir    : {}", cfg.models_dir.display());
    println!("  credentials   : {}", cfg.credentials_file.display());
    println!("  voice cache   : {}", cfg.voice_cache_file.display());
    println!();
    println!(
        "  voices        : {} local (sherpa-onnx), {} cloud (cache)",
        st.local_voices, st.cloud_voices
    );
    if st.cloud_voices == 0 {
        println!("                 run `voicegarden-spd refresh` to populate cloud voices");
    }
    Ok(())
}

fn config_arg(args: &[String]) -> Result<Option<String>, String> {
    let mut cfg = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--config" => {
                i += 1;
                cfg = Some(args.get(i).ok_or("--config requires a value")?.clone());
            }
            other => return Err(format!("unknown argument: {other}")),
        }
        i += 1;
    }
    Ok(cfg)
}

fn cmd_refresh(args: &[String]) -> Result<(), String> {
    let cfg = config_arg(args)?;
    let engines_override = std::env::var_os("VOICEGARDEN_ENGINES_JSON").map(PathBuf::from);
    let report =
        voicegarden_spd::refresh::run_refresh(cfg.as_deref(), engines_override.as_deref())?;
    if !report.failures.is_empty() {
        eprintln!(
            "voicegarden-spd: {} engine(s) failed: {}",
            report.failures.len(),
            report.failures.join("; ")
        );
    }
    Ok(())
}

fn cmd_voices(args: &[String]) -> Result<(), String> {
    let cfg = ModuleConfig::load(config_arg(args)?.as_deref());
    let credentials = voicegarden_spd::voices::load_credentials(&cfg.credentials_file);
    let cache = voicegarden_spd::voices::load_voice_cache(&cfg.voice_cache_file);
    let mut local =
        voicegarden_spd::voices::local_sherpa_voices(&cfg.models_dirs(), cfg.num_threads);
    let mut cloud = voicegarden_spd::voices::cloud_voices(&cache, &credentials);

    println!("{:<44} {:<8} {:<8}", "VOICE", "LANG", "VARIANT");
    for v in local.drain(..).chain(cloud.drain(..)) {
        println!("{:<44} {:<8} {:<8}", v.spd_name, v.language, v.variant);
    }
    Ok(())
}

fn cmd_speak(args: &[String]) -> Result<(), String> {
    let voice_name = args
        .first()
        .ok_or("usage: voicegarden-spd speak <voice> <text>")?
        .clone();
    let text = args
        .get(1)
        .ok_or("usage: voicegarden-spd speak <voice> <text>")?
        .clone();

    // Resolve the voice from the same merged list the module uses.
    let cfg = ModuleConfig::load(None);
    let credentials = voicegarden_spd::voices::load_credentials(&cfg.credentials_file);
    let cache = voicegarden_spd::voices::load_voice_cache(&cfg.voice_cache_file);
    let local = voicegarden_spd::voices::local_sherpa_voices(&cfg.models_dirs(), cfg.num_threads);
    let cloud = voicegarden_spd::voices::cloud_voices(&cache, &credentials);
    let voice = local
        .iter()
        .chain(cloud.iter())
        .find(|v| v.spd_name == voice_name)
        .ok_or_else(|| format!("voice '{voice_name}' not found — see `voicegarden-spd voices`"))?
        .clone();

    let engine = rust_tts_wrapper::create_engine(&voice.engine_id, &voice.credentials)
        .ok_or_else(|| format!("engine '{}' unavailable", voice.engine_id))?;

    let rate = if voice.engine_id == "sherpaonnx" {
        voice.sample_rate.unwrap_or(22_050)
    } else {
        cloud_pcm_rate(&voice.engine_id)
    };
    let mut pcm: Vec<u8> = Vec::new();
    engine
        .speak_sync(
            &text,
            Some(&voice.engine_voice_id),
            1.0,
            1.0,
            1.0,
            Some(&mut |chunk: &[u8]| pcm.extend_from_slice(chunk)),
            None,
        )
        .map_err(|e| e.to_string())?;

    // Write a minimal WAV and hand it to the first available player.
    write_wav_and_play(&pcm, rate)
}

/// Synthesise `text` through `voice`, returning (bytes, elapsed).
fn synth_once(
    engine: &std::sync::Arc<dyn rust_tts_wrapper::engine::TtsEngine>,
    voice_id: &str,
    text: &str,
) -> Result<(usize, std::time::Duration), String> {
    let mut total = 0usize;
    let t0 = std::time::Instant::now();
    engine
        .speak_sync(
            text,
            Some(voice_id),
            1.0,
            1.0,
            1.0,
            Some(&mut |chunk: &[u8]| total += chunk.len()),
            None,
        )
        .map_err(|e| e.to_string())?;
    Ok((total, t0.elapsed()))
}

fn cmd_bench(args: &[String]) -> Result<(), String> {
    let voice_name = args
        .first()
        .ok_or("usage: voicegarden-spd bench <voice> [text] [runs]")?
        .clone();
    let text = args.get(1).map_or_else(
        || "The quick brown fox jumps over the lazy dog.".to_string(),
        Clone::clone,
    );
    let runs: usize = match args.get(2) {
        Some(v) => v.parse().map_err(|_| "runs must be a number".to_string())?,
        None => 5,
    };

    let cfg = ModuleConfig::load(None);
    let credentials = voicegarden_spd::voices::load_credentials(&cfg.credentials_file);
    let cache = voicegarden_spd::voices::load_voice_cache(&cfg.voice_cache_file);
    let local = voicegarden_spd::voices::local_sherpa_voices(&cfg.models_dirs(), cfg.num_threads);
    let cloud = voicegarden_spd::voices::cloud_voices(&cache, &credentials);
    let voice = local
        .iter()
        .chain(cloud.iter())
        .find(|v| v.spd_name == voice_name)
        .ok_or_else(|| format!("voice '{voice_name}' not found — see `voicegarden-spd voices`"))?
        .clone();

    let engine = rust_tts_wrapper::create_engine(&voice.engine_id, &voice.credentials)
        .ok_or_else(|| format!("engine '{}' unavailable", voice.engine_id))?;

    // Cold run: first synthesis on this engine instance. For sherpa-onnx
    // this includes loading + initialising the ONNX model; for cloud it is
    // one network round trip.
    let (cold_bytes, cold) = synth_once(&engine, &voice.engine_voice_id, &text)?;
    println!(
        "cold  : {:>8.1} ms  ({cold_bytes} bytes)  [includes model load for local engines]",
        cold.as_secs_f64() * 1000.0
    );

    let mut samples: Vec<f64> = Vec::with_capacity(runs);
    let mut warm_bytes = cold_bytes;
    for _ in 0..runs {
        let (bytes, d) = synth_once(&engine, &voice.engine_voice_id, &text)?;
        warm_bytes = bytes;
        samples.push(d.as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    let median = samples[samples.len() / 2];
    let best = samples[0];
    println!("warm  : {runs} runs, {warm_bytes} bytes each");
    for (i, s) in samples.iter().enumerate() {
        println!("  run {i:>2}: {s:>8.1} ms");
    }
    println!("  median: {median:>8.1} ms   best: {best:>8.1} ms   mean: {mean:>8.1} ms");
    println!();
    println!(
        "note: engine instances (and their loaded ONNX models) are cached for the\n\
         module's lifetime — in sd_voicegarden, only the first utterance per\n\
         model pays the cold cost."
    );
    Ok(())
}

fn cmd_migrate_models() -> Result<(), String> {
    let cfg = ModuleConfig::load(None);
    let primary = cfg.models_dir.clone();
    std::fs::create_dir_all(&primary).map_err(|e| format!("{}: {e}", primary.display()))?;

    let mut moved = 0usize;
    for legacy in &cfg.legacy_models_dirs {
        if legacy == &primary {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(legacy) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let src = entry.path();
            let dst = primary.join(&name);
            if !src.is_dir() || dst.exists() {
                continue;
            }
            match std::fs::rename(&src, &dst) {
                Ok(()) => {
                    println!("  moved {} → {}", src.display(), dst.display());
                    moved += 1;
                }
                Err(e) => eprintln!("  skipped {}: {e}", src.display()),
            }
        }
    }
    if moved == 0 {
        println!("nothing to migrate — legacy directories are empty or already moved");
    } else {
        println!();
        println!("migrated {moved} model(s) into {}", primary.display());
        println!("restart speech-dispatcher to pick up the new locations");
    }
    Ok(())
}

fn write_wav_and_play(pcm: &[u8], rate: u32) -> Result<(), String> {
    let path = std::env::temp_dir().join("voicegarden-spd-preview.wav");
    let data_len = pcm.len() as u32;
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(pcm);
    std::fs::write(&path, wav).map_err(|e| e.to_string())?;

    let players = [
        "pw-play",
        "paplay",
        "aplay",
        "ffplay -autoexit -nodisp -loglevel quiet",
    ];
    for player in players {
        let mut parts = player.split_whitespace();
        let prog = parts.next().expect("nonempty");
        let args: Vec<&str> = parts.collect();
        let full = format!("{} {path:?}", prog);
        if which(prog) {
            let status = std::process::Command::new(prog)
                .args(args)
                .arg(&path)
                .status()
                .map_err(|e| format!("{full}: {e}"))?;
            if status.success() {
                return Ok(());
            }
        }
    }
    Err(format!(
        "no audio player found (tried pw-play, paplay, aplay, ffplay); \
         the WAV was written to {}",
        path.display()
    ))
}

fn which(prog: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(prog).is_file()))
        .unwrap_or(false)
}

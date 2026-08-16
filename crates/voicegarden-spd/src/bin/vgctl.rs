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
  voicegarden-spd install [--models-dir DIR]   Install module + register (user-local, no root)
  voicegarden-spd uninstall                   Remove installed files + registration
  voicegarden-spd status                      Installation + voice inventory
  voicegarden-spd refresh [--config FILE]     Refresh the cloud voice cache
  voicegarden-spd voices [--config FILE]      List all merged voices
  voicegarden-spd speak <voice> <text>        Speak once through rust-tts-wrapper directly
  voicegarden-spd bench <voice> [text] [N]    Cold + warm synthesis timings (no playback)
  voicegarden-spd migrate-models              Move legacy models into the primary models dir
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
        "uninstall" => cmd_uninstall(),
        "status" => cmd_status(),
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
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--models-dir" => {
                i += 1;
                models_dir = Some(args.get(i).ok_or("--models-dir requires a value")?.clone());
            }
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
    println!();
    println!("Done. Next steps:");
    println!("  1. voicegarden-spd refresh        (fetch cloud voices — optional)");
    println!("  2. systemctl --user restart speech-dispatcher.socket  (or re-login)");
    println!("  3. spd-say -o voicegarden-spd \"Hello from VoiceGarden\"");
    Ok(())
}

fn cmd_uninstall() -> Result<(), String> {
    let steps = installer::uninstall()?;
    if steps.is_empty() {
        println!("nothing installed — no changes made");
    }
    for s in &steps {
        println!("  {s}");
    }
    println!("Restart speech-dispatcher for the change to take effect.");
    Ok(())
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

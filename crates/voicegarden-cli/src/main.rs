//! voicegarden-spd — management CLI for the VoiceGarden speech-dispatcher
//! module: engine credentials, voice search, install/diagnostics.

mod doctor;
mod engine;
mod model;
mod voice;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use voicegarden_spd::config::ModuleConfig;
use voicegarden_spd::installer;

const USAGE_FOOTER: &str = "\
Full docs: https://github.com/AACTools/VoiceGarden-SPD";

#[derive(Parser)]
#[command(
    name = "voicegarden-spd",
    version,
    about = "Manage the VoiceGarden speech-dispatcher module",
    after_help = USAGE_FOOTER
)]
struct Cli {
    /// Use this module config file instead of the default
    #[arg(long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Install the module + register with speech-dispatcher (user-local, no root)
    Install {
        /// Directory for sherpa-onnx models (written into the config)
        #[arg(long, value_name = "DIR")]
        models_dir: Option<String>,
        /// Do not restart speech-dispatcher afterwards
        #[arg(long)]
        no_restart: bool,
    },
    /// Remove installed files + registration
    Uninstall {
        #[arg(long)]
        no_restart: bool,
    },
    /// Installation + voice inventory
    Status,
    /// Diagnose a broken setup
    Doctor,
    /// Refresh the cloud voice cache (network)
    Refresh {
        /// Refresh only these engines (default: all + edge)
        engines: Vec<String>,
    },
    /// Speak once directly through rust-tts-wrapper (no speechd)
    Speak {
        /// Voice name (see `voice search`)
        voice: String,
        text: String,
    },
    /// Cold + warm synthesis timings (no playback)
    Bench {
        voice: String,
        #[arg(default_value = "The quick brown fox jumps over the lazy dog.")]
        text: String,
        #[arg(default_value_t = 5)]
        runs: usize,
    },
    /// Move legacy model directories into the primary models dir
    MigrateModels,
    /// (alias for `voice list`)
    Voices,
    /// Engine management: credentials, verification, cache
    #[command(subcommand)]
    Engine(engine::EngineCmd),
    /// Voice search over local models + cached cloud voices
    #[command(subcommand)]
    Voice(voice::VoiceCmd),
    /// Search the full sherpa-onnx model registry (incl. not-installed)
    #[command(subcommand)]
    Model(model::ModelCmd),
    /// Internal: crash-isolated model check used by `doctor`
    #[command(hide = true)]
    CheckModel { voice: String },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let cfg_path = cli.config.as_deref().and_then(|p| p.to_str());

    let result = match cli.cmd {
        Cmd::Install {
            models_dir,
            no_restart,
        } => cmd_install(models_dir.as_deref(), !no_restart),
        Cmd::Uninstall { no_restart } => cmd_uninstall(!no_restart),
        Cmd::Status => cmd_status(cfg_path),
        Cmd::Doctor => doctor::run().map_err(|e| {
            // failures already printed with [FAIL] markers
            if e.is_empty() {
                "problems found — see above".to_string()
            } else {
                e
            }
        }),
        Cmd::Refresh { engines } => cmd_refresh(cfg_path, &engines),
        Cmd::Speak { voice, text } => crate::voice::preview(&voice, &text, cfg_path),
        Cmd::Bench { voice, text, runs } => cmd_bench(&voice, &text, runs, cfg_path),
        Cmd::MigrateModels => cmd_migrate_models(),
        Cmd::Voices => crate::voice::list(cfg_path, &voice::ListArgs),
        Cmd::Engine(cmd) => engine::run(cmd, cfg_path),
        Cmd::Voice(cmd) => voice::run(cmd, cfg_path),
        Cmd::Model(cmd) => model::run(cmd),
        Cmd::CheckModel { voice } => {
            return doctor::check_model_main(voice);
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

// ---------------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------------

pub(crate) struct Style {
    pub enabled: bool,
}

impl Style {
    pub fn new() -> Self {
        Self {
            enabled: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
        }
    }
    pub fn bold(&self, s: &str) -> String {
        if self.enabled {
            format!("\x1b[1m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    pub fn dim(&self, s: &str) -> String {
        if self.enabled {
            format!("\x1b[2m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    pub fn cyan(&self, s: &str) -> String {
        if self.enabled {
            format!("\x1b[36m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    pub fn magenta(&self, s: &str) -> String {
        if self.enabled {
            format!("\x1b[35m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    pub fn green(&self, s: &str) -> String {
        if self.enabled {
            format!("\x1b[32m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    pub fn red(&self, s: &str) -> String {
        if self.enabled {
            format!("\x1b[31m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    pub fn yellow(&self, s: &str) -> String {
        if self.enabled {
            format!("\x1b[33m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
}

/// Render rows as a left-aligned table with computed column widths.
pub(crate) fn render_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let n = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().take(n).enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    // cap absurd columns so a long licence URL can't blow up the layout
    for w in &mut widths {
        *w = (*w).min(48);
    }
    let mut out = String::new();
    for (i, h) in headers.iter().enumerate() {
        out.push_str(&format!("{h:<width$}  ", width = widths[i]));
    }
    out.push('\n');
    for row in rows {
        for (i, cell) in row.iter().take(n).enumerate() {
            // pad by display width (chars), truncate visually if over cap
            let vis: String = cell.chars().take(widths[i]).collect();
            let pad = widths[i].saturating_sub(vis.chars().count());
            out.push_str(&vis);
            out.push_str(&" ".repeat(pad));
            out.push_str("  ");
        }
        out.push('\n');
    }
    out
}

pub(crate) fn load_cfg(cfg_path: Option<&str>) -> ModuleConfig {
    ModuleConfig::load(cfg_path)
}

// ---------------------------------------------------------------------------
// simple commands (formerly vgctl)
// ---------------------------------------------------------------------------

fn cmd_install(models_dir: Option<&str>, restart: bool) -> Result<(), String> {
    let module = find_binary(voicegarden_spd::installer::MODULE_BIN)?;
    let refresh_bin = find_binary("voicegarden-spd-refresh")?;
    let steps = installer::install(&module, &refresh_bin, models_dir)?;
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

fn cmd_uninstall(restart: bool) -> Result<(), String> {
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

/// Locate our own binaries: beside this executable first (release tarball /
/// target layout), then the installed copy, so re-installing over an
/// existing install never copies the old binary onto itself.
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
            format!("cannot locate '{name}' — run from the release tarball or target/, or install first")
        })
}

fn restart_speechd() -> bool {
    let status = std::process::Command::new("systemctl")
        .args(["--user", "is-active", "speech-dispatcher.service"])
        .output();
    let active = status
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "active")
        .unwrap_or(false);
    if !active {
        return false;
    }
    std::process::Command::new("systemctl")
        .args(["--user", "restart", "speech-dispatcher.service"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn cmd_status(cfg_path: Option<&str>) -> Result<(), String> {
    let cfg = load_cfg(cfg_path);
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
        "  registered    : {} ({})",
        if st.registered { "yes" } else { "no" },
        st.registration_mode
    );
    println!();
    println!("  models dir    : {}", cfg.models_dir.display());
    println!("  credentials   : {}", cfg.credentials_file.display());
    println!("  voice cache   : {}", cfg.voice_cache_file.display());
    println!();
    println!(
        "  voices        : {} local (offline), {} cloud (cache)",
        st.local_voices, st.cloud_voices
    );
    if st.cloud_voices == 0 {
        println!("                 run `voicegarden-spd engine add <engine>` or `refresh` for cloud voices");
    }
    Ok(())
}

fn cmd_refresh(cfg_path: Option<&str>, engines: &[String]) -> Result<(), String> {
    let engines_override = std::env::var_os("VOICEGARDEN_ENGINES_JSON").map(PathBuf::from);
    let only: Option<Vec<String>> = (!engines.is_empty()).then(|| engines.to_vec());
    let report = voicegarden_spd::refresh::run_refresh(
        cfg_path,
        engines_override.as_deref(),
        only.as_deref(),
    )?;
    if !report.failures.is_empty() {
        eprintln!(
            "voicegarden-spd: {} engine(s) failed: {}",
            report.failures.len(),
            report.failures.join("; ")
        );
    }
    Ok(())
}

fn cmd_bench(voice: &str, text: &str, runs: usize, cfg_path: Option<&str>) -> Result<(), String> {
    use voicegarden_spd::voices::{cloud_pcm_rate, merged_voices};

    let cfg = load_cfg(cfg_path);
    let v = merged_voices(&cfg)
        .into_iter()
        .find(|v| v.spd_name == voice)
        .ok_or_else(|| format!("voice '{voice}' not found — see `voice search`"))?;

    let engine = rust_tts_wrapper::create_engine(&v.engine_id, &v.credentials)
        .ok_or_else(|| format!("engine '{}' unavailable", v.engine_id))?;

    let synth_once = || -> Result<(usize, std::time::Duration), String> {
        let mut total = 0usize;
        let t0 = std::time::Instant::now();
        engine
            .speak_sync(
                text,
                Some(&v.engine_voice_id),
                1.0,
                1.0,
                1.0,
                Some(&mut |chunk: &[u8]| total += chunk.len()),
                None,
                None,
            )
            .map_err(|e| e.to_string())?;
        Ok((total, t0.elapsed()))
    };

    let (cold_bytes, cold) = synth_once()?;
    println!(
        "cold  : {:>8.1} ms  ({cold_bytes} bytes)  [includes model load for local engines]",
        cold.as_secs_f64() * 1000.0
    );

    let mut samples: Vec<f64> = Vec::with_capacity(runs);
    let mut warm_bytes = cold_bytes;
    for _ in 0..runs {
        let (bytes, d) = synth_once()?;
        warm_bytes = bytes;
        samples.push(d.as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    println!("warm  : {runs} runs, {warm_bytes} bytes each");
    println!(
        "  median: {:>8.1} ms   best: {:>8.1} ms   mean: {mean:>8.1} ms",
        samples[samples.len() / 2],
        samples[0]
    );
    let _ = cloud_pcm_rate(&v.engine_id);
    println!();
    println!("note: engine instances (and loaded ONNX models) are cached for the");
    println!("module's lifetime — only the first utterance per model pays the cold cost.");
    Ok(())
}

fn cmd_migrate_models() -> Result<(), String> {
    let cfg = load_cfg(None);
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

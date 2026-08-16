//! voicegarden-spd-refresh — populate the cloud voice cache.
//!
//! Thin wrapper over [`voicegarden_spd::refresh::run_refresh`]; the logic
//! lives in the library so the management CLI shares it.
//!
//! Usage:
//!   voicegarden-spd-refresh [--config /path/to/voicegarden-spd.conf]

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut config_path = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => config_path = args.next(),
            "--version" => {
                println!("voicegarden-spd-refresh {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument: {other}");
                return ExitCode::FAILURE;
            }
        }
    }

    let engines_override =
        std::env::var_os("VOICEGARDEN_ENGINES_JSON").map(std::path::PathBuf::from);

    match voicegarden_spd::refresh::run_refresh(
        config_path.as_deref(),
        engines_override.as_deref(),
        None,
    ) {
        Ok(report) if report.failures.is_empty() => ExitCode::SUCCESS,
        Ok(report) => {
            eprintln!(
                "voicegarden-spd-refresh: {} engine(s) failed",
                report.failures.len()
            );
            ExitCode::from(2)
        }
        Err(e) => {
            eprintln!("voicegarden-spd-refresh: {e}");
            ExitCode::FAILURE
        }
    }
}

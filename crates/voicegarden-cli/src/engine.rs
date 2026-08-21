//! `voicegarden-spd engine …` — credential management with live
//! verification, engine inventory, and cache control.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};

use clap::Subcommand;
use voicegarden_spd::config::ModuleConfig;
use voicegarden_spd::credentials as creds;
use voicegarden_spd::voices::{load_voice_cache, VoiceCache};

use crate::Style;

#[derive(Subcommand)]
pub enum EngineCmd {
    /// List all known engines: credential status + cached voice counts
    List,
    /// Add or update credentials for an engine (interactive, verified live)
    Add {
        /// Engine id (see `engine list`)
        id: String,
        /// Provide credentials non-interactively: --set apiKey=…  (repeatable)
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set_values: Vec<String>,
        /// Save even if live verification fails
        #[arg(long)]
        force: bool,
        /// Skip the voice-cache refresh after saving
        #[arg(long)]
        no_refresh: bool,
    },
    /// Remove an engine's credentials (and its cached voices)
    Remove {
        id: String,
        /// Also drop its cached voice list
        #[arg(long)]
        keep_cache: bool,
    },
    /// Verify an engine's stored credentials live
    Test { id: String },
    /// Show stored credentials (masked) + cache state
    Show { id: String },
}

pub fn run(cmd: EngineCmd, cfg_path: Option<&str>) -> Result<(), String> {
    match cmd {
        EngineCmd::List => list(cfg_path),
        EngineCmd::Add {
            id,
            set_values,
            force,
            no_refresh,
        } => add(cfg_path, &id, &set_values, force, no_refresh),
        EngineCmd::Remove { id, keep_cache } => remove(cfg_path, &id, keep_cache),
        EngineCmd::Test { id } => test(cfg_path, &id),
        EngineCmd::Show { id } => show(cfg_path, &id),
    }
}

fn load_store(cfg: &ModuleConfig) -> BTreeMap<String, serde_json::Map<String, serde_json::Value>> {
    creds::load(&cfg.credentials_file)
}

fn cache_count(cfg: &ModuleConfig, id: &str) -> usize {
    load_voice_cache(&cfg.voice_cache_file)
        .engines
        .get(id)
        .map_or(0, Vec::len)
}

// ---------------------------------------------------------------------------
// engine list
// ---------------------------------------------------------------------------

fn list(cfg_path: Option<&str>) -> Result<(), String> {
    let cfg = ModuleConfig::load(cfg_path);
    let store = load_store(&cfg);
    let st = Style::new();

    let engines = creds::known_engines();
    let mut rows = Vec::new();
    for e in &engines {
        let configured = store.contains_key(&e.id);
        // Local engines have no cloud cache — count installed model
        // voices instead (a 0 here used to hide a working setup, #8).
        let local_count = || {
            voicegarden_spd::voices::merged_voices(&cfg)
                .into_iter()
                .filter(|v| v.engine_id == e.id)
                .count()
                .to_string()
        };
        let (cred_state, voices) = if matches!(e.id.as_str(), "floravox" | "sherpaonnx") {
            (st.dim("none needed"), local_count())
        } else if !e.needs_credentials {
            (st.dim("none needed"), cache_count(&cfg, &e.id).to_string())
        } else if configured {
            (st.green("configured"), cache_count(&cfg, &e.id).to_string())
        } else {
            (st.yellow("—"), st.dim("0").to_string())
        };
        rows.push(vec![
            e.id.clone(),
            e.name.clone(),
            cred_state,
            voices,
            st.dim(&e.keys.join(", ")),
        ]);
    }
    println!("{}", st.bold("ENGINES"));
    println!(
        "{}",
        crate::render_table(
            &["ENGINE", "NAME", "CREDENTIALS", "VOICES", "REQUIRED KEYS"],
            &rows
        )
    );
    println!(
        "{}",
        st.dim("configure with: voicegarden-spd engine add <engine>")
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// engine add
// ---------------------------------------------------------------------------

fn add(
    cfg_path: Option<&str>,
    id: &str,
    set_values: &[String],
    force: bool,
    no_refresh: bool,
) -> Result<(), String> {
    if id == "system" {
        return Err(
            "the speech-dispatcher 'system' engine is deliberately not exposed here \
             (it would route speech-dispatcher back into itself)"
                .into(),
        );
    }
    let cfg = ModuleConfig::load(cfg_path);
    let Some(info) = creds::engine_info(id) else {
        return Err(format!(
            "unknown engine '{id}' — see `voicegarden-spd engine list`"
        ));
    };

    if !info.needs_credentials {
        // edge / sherpaonnx
        println!("{} needs no credentials.", info.name);
        if id == "sherpaonnx" {
            println!("local models are configured via `model find` / the models dir:");
            println!("  {}", cfg.models_dir.display());
        } else {
            println!("refreshing its voice cache…");
            refresh_only(cfg_path, id)?;
        }
        return Ok(());
    }

    println!("{}", info.name);
    if load_store(&cfg).contains_key(id) {
        println!("  (updating existing credentials)");
    }

    // --set values first
    let mut collected: BTreeMap<String, String> = BTreeMap::new();
    for pair in set_values {
        let (k, v) = pair
            .split_once('=')
            .ok_or_else(|| format!("--set expects KEY=VALUE, got {pair:?}"))?;
        if !info.keys.iter().any(|key| key.eq_ignore_ascii_case(k)) {
            return Err(format!(
                "engine '{id}' has no credential key {k:?} (needs: {})",
                info.keys.join(", ")
            ));
        }
        collected.insert(k.to_string(), v.to_string());
    }

    // prompt for the rest
    let tty = std::io::stdin().is_terminal();
    for key in &info.keys {
        if collected.contains_key(key) {
            continue;
        }
        if !tty {
            return Err(format!(
                "missing credential {key:?} — pass --set {key}=… (needs: {})",
                info.keys.join(", ")
            ));
        }
        let value = if creds::is_secret_key(key) {
            rpassword::prompt_password(format!("  {key}: "))
                .map_err(|e| format!("could not read {key}: {e}"))?
        } else {
            print!("  {key}: ");
            std::io::stdout().flush().ok();
            let mut line = String::new();
            std::io::stdin()
                .read_line(&mut line)
                .map_err(|e| format!("could not read {key}: {e}"))?;
            line.trim().to_string()
        };
        if value.is_empty() {
            return Err(format!("{key} cannot be empty — aborted, nothing saved"));
        }
        collected.insert(key.clone(), value);
    }

    // verify live
    println!("  verifying…");
    let json = creds::creds_to_json(&collected);
    let result = creds::check(id, &json);
    if result.ok {
        println!(
            "  {} — {}{}",
            Style::new().green("verified"),
            result.detail,
            result
                .voice_count
                .map(|n| format!(" ({n} voices listed)"))
                .unwrap_or_default()
        );
    } else {
        eprintln!(
            "  {} — {}",
            Style::new().red("verification FAILED"),
            result.detail
        );
        if !force {
            eprintln!("  nothing saved (use --force to save anyway)");
            return Err(String::new());
        }
        eprintln!("  saving anyway (--force)");
    }

    // save
    let mut store = load_store(&cfg);
    let entry: serde_json::Map<String, serde_json::Value> = collected
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();
    store.insert(id.to_string(), entry);
    creds::save(&cfg.credentials_file, &store)?;
    println!("  saved {} (mode 0600)", cfg.credentials_file.display());

    if !no_refresh {
        refresh_only(cfg_path, id)?;
    } else {
        println!("  refresh the cache later with: voicegarden-spd refresh {id}");
    }
    Ok(())
}

fn refresh_only(cfg_path: Option<&str>, id: &str) -> Result<(), String> {
    let only = vec![id.to_string()];
    let report = voicegarden_spd::refresh::run_refresh(cfg_path, None, Some(&only))?;
    let added = report.engines.get(id).copied().unwrap_or(0);
    println!("  {added} voices cached for {id}");
    println!("  restart speech-dispatcher (or re-login) for the module to see them");
    Ok(())
}

// ---------------------------------------------------------------------------
// engine remove / test / show
// ---------------------------------------------------------------------------

fn remove(cfg_path: Option<&str>, id: &str, keep_cache: bool) -> Result<(), String> {
    let cfg = ModuleConfig::load(cfg_path);
    let mut store = load_store(&cfg);
    if store.remove(id).is_none() {
        return Err(format!("engine '{id}' has no stored credentials"));
    }
    creds::save(&cfg.credentials_file, &store)?;
    println!("removed credentials for {id}");

    if !keep_cache {
        let cache = load_voice_cache(&cfg.voice_cache_file);
        if cache.engines.contains_key(id) {
            let pruned = VoiceCache {
                engines: cache.engines.into_iter().filter(|(k, _)| k != id).collect(),
            };
            if let Some(parent) = cfg.voice_cache_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let text = serde_json::to_string_pretty(&pruned).map_err(|e| e.to_string())?;
            std::fs::write(&cfg.voice_cache_file, text)
                .map_err(|e| format!("{}: {e}", cfg.voice_cache_file.display()))?;
            println!("dropped cached voices for {id}");
        }
    }
    println!("restart speech-dispatcher (or re-login) for the change to take effect");
    Ok(())
}

fn test(cfg_path: Option<&str>, id: &str) -> Result<(), String> {
    let cfg = ModuleConfig::load(cfg_path);
    let json = load_store(&cfg)
        .get(id)
        .map(|m| serde_json::Value::Object(m.clone()).to_string())
        .unwrap_or_else(|| "{}".into());
    let result = creds::check(id, &json);
    let st = Style::new();
    if result.ok {
        println!(
            "{} {id}: {} ({} ms{})",
            st.green("ok"),
            result.detail,
            result.elapsed_ms,
            result
                .voice_count
                .map(|n| format!(", {n} voices"))
                .unwrap_or_default()
        );
        Ok(())
    } else {
        println!("{} {id}: {}", st.red("fail"), result.detail);
        Err(String::new())
    }
}

fn show(cfg_path: Option<&str>, id: &str) -> Result<(), String> {
    let cfg = ModuleConfig::load(cfg_path);
    let info = creds::engine_info(id)
        .ok_or_else(|| format!("unknown engine '{id}' — see `engine list`"))?;
    println!("{}", info.name);
    println!("  id:                {}", info.id);
    println!("  needs credentials: {}", info.needs_credentials);
    let store = load_store(&cfg);
    if let Some(entry) = store.get(id) {
        for (k, v) in entry {
            let shown = v.as_str().map(creds::mask).unwrap_or_else(|| v.to_string());
            println!("  {k}: {shown}");
        }
    } else if info.needs_credentials {
        println!("  {}", Style::new().yellow("no credentials configured"));
    }
    println!("  cached voices:     {}", cache_count(&cfg, id));
    Ok(())
}

//! Cloud voice-cache refresh: enumerate voices for every configured
//! engine and write the merged cache the module reads at startup.
//!
//! Shared by the `voicegarden-spd-refresh` binary and the `voicegarden-spd`
//! management CLI (`refresh` subcommand).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::ModuleConfig;
use crate::voices::{load_voice_cache, CachedVoice, VoiceCache};

/// Result of a refresh run.
pub struct RefreshReport {
    pub engines: HashMap<String, usize>,
    pub failures: Vec<String>,
    pub cache_path: PathBuf,
    pub total: usize,
}

/// Populate the cloud voice cache.
///
/// * `config_path` — optional module config file (same `--config` as the
///   module binary); defaults are used when absent.
/// * `engines_json_override` — use this credentials file instead of the
///   configured one (used by the `VOICEGARDEN_ENGINES_JSON` env var and
///   tests).
/// * `only` — refresh just these engine ids (e.g. after `engine add`);
///   `None` refreshes everything (edge + all credentialed engines).
#[allow(clippy::too_many_lines)]
pub fn run_refresh(
    config_path: Option<&str>,
    engines_json_override: Option<&Path>,
    only: Option<&[String]>,
) -> Result<RefreshReport, String> {
    let cfg = ModuleConfig::load(config_path);

    let engines_path: PathBuf =
        engines_json_override.map_or_else(|| cfg.credentials_file.clone(), Path::to_path_buf);

    let credentials = crate::voices::load_credentials(&engines_path);
    if credentials.as_object().is_none_or(|o| o.is_empty()) {
        eprintln!(
            "voicegarden-spd: no engines configured in {} — only 'edge' will be fetched.",
            engines_path.display()
        );
    }

    // Always try edge (credential-free); then every engine with creds.
    let mut wanted: Vec<String> = vec!["edge".into()];
    if let Some(obj) = credentials.as_object() {
        for key in obj.keys() {
            if key != "edge" && !wanted.contains(key) {
                wanted.push(key.clone());
            }
        }
    }
    if let Some(ids) = only {
        wanted.retain(|id| ids.iter().any(|want| want == id));
        if wanted.is_empty() {
            return Err(format!(
                "none of the requested engines are refreshable (asked for: {})",
                ids.join(", ")
            ));
        }
    }

    let mut cache = VoiceCache::default();
    let mut failures = Vec::new();
    let mut engines = HashMap::new();

    for engine_id in &wanted {
        let creds_json = credentials
            .get(engine_id)
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}))
            .to_string();
        println!("voicegarden-spd: listing voices for {engine_id}…");
        match rust_tts_wrapper::create_engine(engine_id, &creds_json) {
            Some(engine) => match engine.get_voices() {
                Ok(list) => {
                    let cached: Vec<CachedVoice> = list
                        .iter()
                        .map(|v| CachedVoice {
                            id: v.id.clone(),
                            name: v.name.clone(),
                            gender: v.gender.to_string(),
                            lang: v.primary_language().to_string(),
                        })
                        .collect();
                    println!("voicegarden-spd:   {} voices for {engine_id}", cached.len());
                    engines.insert(engine_id.clone(), cached.len());
                    cache.engines.insert(engine_id.clone(), cached);
                }
                Err(e) => {
                    failures.push(format!("{engine_id}: {e}"));
                    eprintln!("voicegarden-spd:   {engine_id} failed: {e}");
                }
            },
            None => {
                failures.push(format!("{engine_id}: unknown engine"));
                eprintln!("voicegarden-spd:   unknown engine '{engine_id}'");
            }
        }
    }

    // Preserve engines from the previous cache that we did not refresh.
    let old = load_voice_cache(&cfg.voice_cache_file);
    for (id, voices) in old.engines {
        cache.engines.entry(id).or_insert(voices);
    }

    if let Some(parent) = cfg.voice_cache_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let text = serde_json::to_string_pretty(&cache).map_err(|e| e.to_string())?;
    std::fs::write(&cfg.voice_cache_file, text)
        .map_err(|e| format!("cannot write {}: {e}", cfg.voice_cache_file.display()))?;

    let total: usize = cache.engines.values().map(Vec::len).sum();
    println!(
        "voicegarden-spd: wrote {total} voices ({} engines) to {}",
        cache.engines.len(),
        cfg.voice_cache_file.display()
    );
    Ok(RefreshReport {
        engines,
        failures,
        cache_path: cfg.voice_cache_file,
        total,
    })
}

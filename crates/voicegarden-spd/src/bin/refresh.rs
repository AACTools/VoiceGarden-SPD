//! voicegarden-spd-refresh — populate the cloud voice cache.
//!
//! Enumerates voices for every engine with credentials in `engines.json`
//! (plus credential-free `edge`) and writes the merged list to the voice
//! cache file that `sd_voicegarden` reads at startup. Run this whenever
//! credentials change; the module itself never touches the network.
//!
//! Usage:
//!   voicegarden-spd-refresh [--config /path/to/voicegarden-spd.conf]
//!
//! Environment override: VOICEGARDEN_ENGINES_JSON points at an engines
//! file directly, skipping config parsing (handy for testing).

use std::path::PathBuf;
use std::process::ExitCode;

use voicegarden_spd::config::ModuleConfig;
use voicegarden_spd::voices::{load_credentials, load_voice_cache, CachedVoice, VoiceCache};

fn main() -> ExitCode {
    let config_path = std::env::args().nth(1);
    let cfg = match config_path.as_deref() {
        Some(p) if p == "--config" => {
            let path = std::env::args().nth(2).unwrap_or_default();
            ModuleConfig::load(Some(&path))
        }
        _ => ModuleConfig::load(None),
    };

    let engines_path = std::env::var("VOICEGARDEN_ENGINES_JSON")
        .map(PathBuf::from)
        .unwrap_or_else(|_| cfg.credentials_file.clone());

    let credentials = load_credentials(&engines_path);
    if credentials.as_object().map_or(true, |o| o.is_empty()) {
        eprintln!(
            "voicegarden-spd-refresh: no engines configured in {} — only 'edge' will be fetched.",
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

    let mut cache = VoiceCache::default();
    let mut failures = 0usize;

    for engine_id in &wanted {
        let creds_value = credentials
            .get(engine_id)
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let creds_json = creds_value.to_string();
        println!("voicegarden-spd-refresh: listing voices for {engine_id}…");
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
                    println!(
                        "voicegarden-spd-refresh:   {} voices for {engine_id}",
                        cached.len()
                    );
                    cache.engines.insert(engine_id.clone(), cached);
                }
                Err(e) => {
                    failures += 1;
                    eprintln!("voicegarden-spd-refresh:   {engine_id} failed: {e}");
                }
            },
            None => {
                failures += 1;
                eprintln!("voicegarden-spd-refresh:   unknown engine '{engine_id}'");
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
    match serde_json::to_string_pretty(&cache) {
        Ok(text) => {
            if let Err(e) = std::fs::write(&cfg.voice_cache_file, text) {
                eprintln!(
                    "voicegarden-spd-refresh: cannot write {}: {e}",
                    cfg.voice_cache_file.display()
                );
                return ExitCode::FAILURE;
            }
        }
        Err(e) => {
            eprintln!("voicegarden-spd-refresh: cannot serialise cache: {e}");
            return ExitCode::FAILURE;
        }
    }

    let total: usize = cache.engines.values().map(Vec::len).sum();
    println!(
        "voicegarden-spd-refresh: wrote {total} voices ({} engines) to {}",
        cache.engines.len(),
        cfg.voice_cache_file.display()
    );
    if failures > 0 {
        eprintln!("voicegarden-spd-refresh: {failures} engine(s) failed");
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    }
}

//! Voice-list construction.
//!
//! VoiceGarden exposes one merged voice list to speech-dispatcher:
//!
//! * **Local sherpa-onnx models** — every model directory found under
//!   `ModelsDir` that matches an id in the embedded 1300-model registry.
//!   Voices are named `<model-id>#<speaker-id>`.
//! * **Cloud engines** — voices enumerated by `voicegarden-spd-refresh`
//!   (or the config app) and cached on disk; the module never touches the
//!   network during init. Voices are named `<engine>/<voice-id>`.
//!
//! Cloud voice lists live in a small JSON cache:
//!
//! ```json
//! { "engines": { "edge": [ {"id": "en-US-AriaNeural", "name": "...",
//!                            "gender": "Female", "lang": "en-US"} ] } }
//! ```

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use rust_tts_wrapper::SherpaOnnxEngine;

/// A VoiceGarden voice as presented to speech-dispatcher.
#[derive(Debug, Clone)]
pub struct VgVoice {
    /// Name the server selects via `synthesis_voice` (unique).
    pub spd_name: String,
    /// Primary language code ("en", "en-US", ...).
    pub language: String,
    /// Voice-type variant ("male1", "female1", or "" when unknown).
    pub variant: String,
    /// rust-tts-wrapper engine id ("sherpaonnx", "azure", "edge", ...).
    pub engine_id: String,
    /// Voice id as passed to `TtsEngine::speak` (speaker id / cloud id).
    pub engine_voice_id: String,
    /// Credentials JSON for engine construction.
    pub credentials: String,
    /// Sample rate when statically known (sherpa registry); None for cloud.
    pub sample_rate: Option<u32>,
    /// PCM sample rate the engine delivers via `on_audio`.
    ///
    /// rust-tts-wrapper decodes every cloud engine to PCM16 **mono** before
    /// delivery, but the callback carries no rate, so the module supplies it:
    /// sherpa rates come from the registry; cloud rates are the provider's
    /// fixed output rate (Azure/Cartesia request raw 24 kHz explicitly,
    /// Edge's WS stream is 24 kHz MP3, OpenAI's default MP3 is 24 kHz,
    /// ElevenLabs' default MP3 is 44.1 kHz).
    pub pcm_rate: u32,
    /// Whether the engine accepts SSML input (the crate's azure/edge/google
    /// paths build or forward SSML natively). When true and the client sent
    /// SSML, the module passes it through (minus `<mark>` tags, which the
    /// module times itself) — giving clients `<prosody>`, `<break>`,
    /// `<say-as>`, `<sub>` etc., and SpeechMarkdown via the crate's
    /// in-`speak()` conversion.
    pub ssml_capable: bool,
}

/// Engines whose rust-tts-wrapper implementation accepts SSML input.
#[must_use]
pub fn engine_accepts_ssml(engine_id: &str) -> bool {
    matches!(engine_id, "azure" | "edge" | "google")
}

/// Fixed PCM output rate per cloud engine (see [`VgVoice::pcm_rate`]).
#[must_use]
pub fn cloud_pcm_rate(engine_id: &str) -> u32 {
    match engine_id {
        "elevenlabs" => 44_100,
        _ => 24_000,
    }
}

/// On-disk cloud voice cache.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct VoiceCache {
    pub engines: HashMap<String, Vec<CachedVoice>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedVoice {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub gender: String,
    #[serde(default)]
    pub lang: String,
}

/// Scan the model directories (primary first, then legacy fallbacks)
/// against the sherpa-onnx registry and produce one voice per
/// (installed model × speaker). A model found in an earlier directory
/// wins; duplicates are skipped.
pub fn local_sherpa_voices(models_dirs: &[std::path::PathBuf], num_threads: i32) -> Vec<VgVoice> {
    let registry_engine = SherpaOnnxEngine::new("{}");
    let registry = registry_engine.available_models();

    let mut voices = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for models_dir in models_dirs {
        let Ok(entries) = std::fs::read_dir(models_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Some(id) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if !entry.path().is_dir() || !seen.insert(id.clone()) {
                continue;
            }
            let Some(info) = registry.get(&id) else {
                continue;
            };
            let lang = info
                .language
                .first()
                .map(|l| l.lang_code.clone())
                .unwrap_or_default();
            let num_speakers = info.num_speakers.max(1);
            for sid in 0..num_speakers {
                voices.push(VgVoice {
                    spd_name: format!("{id}#{sid}"),
                    language: lang.clone(),
                    variant: String::new(),
                    engine_id: "sherpaonnx".into(),
                    engine_voice_id: sid.to_string(),
                    // SherpaOnnxEngine::new parses credentials as
                    // HashMap<String, String>, so every value must be a
                    // string (a JSON number makes the whole parse fail
                    // silently and the engine comes up with no model).
                    // modelPath points at the directory the model was
                    // actually found in, so legacy layouts load in place.
                    credentials: serde_json::json!({
                        "modelPath": models_dir.to_string_lossy(),
                        "modelId": id,
                        "numThreads": num_threads.to_string(),
                    })
                    .to_string(),
                    sample_rate: Some(info.sample_rate),
                    pcm_rate: info.sample_rate,
                    ssml_capable: false,
                });
            }
        }
    }
    voices
}

/// Load the cloud voice cache file (missing file → empty cache, not an
/// error — the module must start without network).
pub fn load_voice_cache(path: &Path) -> VoiceCache {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Turn the cache + credentials into cloud voices.
pub fn cloud_voices(cache: &VoiceCache, credentials: &serde_json::Value) -> Vec<VgVoice> {
    let mut voices = Vec::new();
    // `edge` needs no credentials; include it whenever the cache has it.
    let mut engine_ids: Vec<&String> = cache.engines.keys().collect();
    engine_ids.sort();
    for engine_id in engine_ids {
        let has_creds = credentials.get(engine_id).is_some();
        if !has_creds && engine_id != "edge" {
            continue;
        }
        let creds_value = credentials
            .get(engine_id)
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let creds_json = creds_value.to_string();
        for v in &cache.engines[engine_id] {
            voices.push(VgVoice {
                spd_name: format!("{engine_id}/{}", v.id),
                language: v.lang.clone(),
                variant: variant_for_gender(&v.gender),
                engine_id: engine_id.clone(),
                engine_voice_id: v.id.clone(),
                credentials: creds_json.clone(),
                sample_rate: None,
                pcm_rate: cloud_pcm_rate(engine_id),
                ssml_capable: engine_accepts_ssml(engine_id),
            });
        }
    }
    voices
}

/// Map a gender string to the speech-dispatcher voice-type vocabulary used
/// in the `variant` field (lowercase to match `cmd_list_voices`'s
/// case-insensitive comparison).
fn variant_for_gender(gender: &str) -> String {
    match gender.to_ascii_lowercase().as_str() {
        "male" => "male1".into(),
        "female" => "female1".into(),
        _ => String::new(),
    }
}

/// Load the credentials file (`engines.json`).
pub fn load_credentials(path: &Path) -> serde_json::Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_roundtrip_and_cloud_voices() {
        let mut cache = VoiceCache::default();
        cache.engines.insert(
            "edge".into(),
            vec![CachedVoice {
                id: "en-US-AriaNeural".into(),
                name: "Aria".into(),
                gender: "Female".into(),
                lang: "en-US".into(),
            }],
        );
        cache.engines.insert(
            "openai".into(),
            vec![CachedVoice {
                id: "alloy".into(),
                name: "Alloy".into(),
                gender: String::new(),
                lang: "en".into(),
            }],
        );

        // No credentials → only edge survives.
        let voices = cloud_voices(&cache, &serde_json::json!({}));
        assert_eq!(voices.len(), 1);
        assert_eq!(voices[0].spd_name, "edge/en-US-AriaNeural");
        assert_eq!(voices[0].variant, "female1");

        // With openai credentials both engines appear.
        let creds = serde_json::json!({"openai": {"apiKey": "k"}});
        let voices = cloud_voices(&cache, &creds);
        assert_eq!(voices.len(), 2);
        let openai = voices.iter().find(|v| v.engine_id == "openai").unwrap();
        assert_eq!(openai.credentials, r#"{"apiKey":"k"}"#);
    }

    #[test]
    fn missing_cache_is_empty() {
        let cache = load_voice_cache(Path::new("/nonexistent/voices.json"));
        assert!(cache.engines.is_empty());
    }
}

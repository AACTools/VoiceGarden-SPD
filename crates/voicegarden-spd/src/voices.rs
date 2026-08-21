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
    /// Safety net for the local-engine default: (engine id, credentials)
    /// to retry with when the primary engine fails before any audio
    /// flowed (e.g. floravox cannot load an unexpected graph variant).
    /// `None` when there is no meaningful alternative.
    pub fallback: Option<(String, String)>,
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
    // ---- searchable metadata (not used by the module protocol) ----
    /// Human-readable display name.
    pub display_name: String,
    /// "Male" / "Female" / "Unknown" (local sherpa voices are Unknown).
    pub gender: String,
    /// Registry quality/variant tier (local only; "" for cloud).
    pub quality: String,
    /// Model family (local only; "" for cloud).
    pub model_type: String,
    /// Every language this voice covers (local multilingual models carry
    /// several; cloud voices carry one).
    pub languages: Vec<String>,
    /// True when the voice covers several languages (local registry data,
    /// or "Multilingual" in an Azure/Edge voice id).
    pub multilingual: bool,
    /// SPDX licence id (local only).
    pub license: String,
    /// Speaker count of the underlying model (local only).
    pub num_speakers: u32,
}

/// Where a voice synthesises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Local,
    Cloud,
}

impl VgVoice {
    /// Local vs cloud source.
    #[must_use]
    pub fn source(&self) -> Source {
        if matches!(self.engine_id.as_str(), "sherpaonnx" | "floravox") {
            Source::Local
        } else {
            Source::Cloud
        }
    }
}

/// Search filter for [`filter_voices`]. All conditions AND together;
/// `terms` match case-insensitively against name/id/engine/language.
#[derive(Debug, Clone, Default)]
pub struct VoiceFilter {
    /// Free-text terms (all must match somewhere).
    pub terms: Vec<String>,
    /// Restrict to local (offline) or cloud voices.
    pub source: Option<Source>,
    /// Restrict to these engine ids (case-insensitive).
    pub engines: Vec<String>,
    /// Base-language match: "en" matches "en-US"; "en-GB" matches exactly.
    pub lang: Option<String>,
    /// "male" / "female" / "unknown" (case-insensitive).
    pub gender: Option<String>,
    /// Exact registry tier ("low", "medium", "high", ...) — local only.
    pub quality: Option<String>,
    /// Only multilingual voices.
    pub multilingual: bool,
}

/// Base language of a BCP-47-ish code ("en-US" → "en").
fn base_lang(code: &str) -> &str {
    code.split('-').next().unwrap_or(code)
}

#[must_use]
pub fn filter_voices(voices: &[VgVoice], f: &VoiceFilter) -> Vec<VgVoice> {
    let lang = f.lang.as_deref().map(str::to_lowercase);
    let gender = f.gender.as_deref().map(str::to_lowercase);
    let quality = f.quality.as_deref().map(str::to_lowercase);
    voices
        .iter()
        .filter(|v| {
            if let Some(want) = f.source {
                if v.source() != want {
                    return false;
                }
            }
            if !f.engines.is_empty()
                && !f
                    .engines
                    .iter()
                    .any(|e| e.eq_ignore_ascii_case(&v.engine_id))
            {
                return false;
            }
            if let Some(want) = &lang {
                let exact = v.language.eq_ignore_ascii_case(want);
                let base = v.languages.iter().any(|l| {
                    l.eq_ignore_ascii_case(want) || base_lang(l).eq_ignore_ascii_case(want)
                });
                if !(exact || base) {
                    return false;
                }
            }
            if let Some(want) = &gender {
                if !v.gender.eq_ignore_ascii_case(want) {
                    return false;
                }
            }
            if let Some(want) = &quality {
                if !v.quality.eq_ignore_ascii_case(want) {
                    return false;
                }
            }
            if f.multilingual && !v.multilingual {
                return false;
            }
            for term in &f.terms {
                let t = term.to_lowercase();
                let hay = format!(
                    "{} {} {} {} {}",
                    v.spd_name, v.display_name, v.engine_id, v.language, v.model_type
                )
                .to_lowercase();
                let langs = v.languages.iter().any(|l| l.to_lowercase().contains(&t));
                if !hay.contains(&t) && !langs {
                    return false;
                }
            }
            true
        })
        .cloned()
        .collect()
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

/// Build one voice per installed model, driven by the configured local
/// engine (`ModuleConfig::local_engine`, default floravox):
///
/// * drivable family (registry `engines` field: vits/mms/matcha/kokoro)
///   with floravox preferred results in a floravox voice (native SSML,
///   SpeechMarkdown, measured word timings, plus a sherpa fallback for
///   load failures)
/// * everything else results in a sherpa-onnx voice (the only engine
///   that can)
///
/// Voice names stay `<id>#<speaker>` either way, so saved selections keep
/// working when the engine preference flips.
pub fn local_voices(
    models_dirs: &[std::path::PathBuf],
    num_threads: i32,
    engine_pref: &str,
) -> Vec<VgVoice> {
    let registry = sherpa_onnx_models::models();

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
            let langs: Vec<String> = info.language.iter().map(|l| l.lang_code.clone()).collect();
            let lang = langs.first().cloned().unwrap_or_default();
            let multilingual = langs.len() > 1;
            let num_speakers = info.num_speakers.max(1);
            for sid in 0..num_speakers {
                let display = if num_speakers > 1 {
                    format!("{} (speaker {sid})", info.name)
                } else {
                    info.name.clone()
                };
                let drivable = matches!(
                    info.model_type.as_str(),
                    "vits" | "mms" | "matcha" | "kokoro"
                );
                if drivable && engine_pref == "floravox" {
                    // lang routes the published lexicon bundle
                    // (voicegarden-lexicons) for phoneme-map voices;
                    // MMS-style character-table voices are auto-detected
                    // by the engine and must NOT get lang — a lexicon
                    // would phonemize characters.
                    let mut creds = serde_json::json!({
                        "modelsDir": models_dir.to_string_lossy(),
                        "modelId": id,
                        "numThreads": num_threads.to_string(),
                    });
                    if info.model_type != "mms" {
                        creds["lang"] = serde_json::Value::String(lang.clone());
                    }
                    voices.push(VgVoice {
                        spd_name: format!("{id}#{sid}"),
                        language: lang.clone(),
                        variant: String::new(),
                        engine_id: "floravox".into(),
                        engine_voice_id: id.clone(),
                        credentials: creds.to_string(),
                        // If floravox cannot load this model's graph,
                        // speak through sherpa-onnx instead of failing.
                        fallback: Some((
                            "sherpaonnx".into(),
                            serde_json::json!({
                                "modelPath": models_dir.to_string_lossy(),
                                "modelId": id,
                                "numThreads": num_threads.to_string(),
                            })
                            .to_string(),
                        )),
                        sample_rate: Some(info.sample_rate),
                        pcm_rate: info.sample_rate,
                        ssml_capable: true,
                        display_name: display.clone(),
                        gender: "Unknown".into(),
                        quality: info.quality.clone(),
                        model_type: info.model_type.clone(),
                        languages: langs.clone(),
                        multilingual,
                        license: info.license.clone(),
                        num_speakers: info.num_speakers,
                    });
                } else {
                    voices.push(VgVoice {
                        spd_name: format!("{id}#{sid}"),
                        language: lang.clone(),
                        variant: String::new(),
                        engine_id: "sherpaonnx".into(),
                        engine_voice_id: sid.to_string(),
                        credentials: serde_json::json!({
                            "modelPath": models_dir.to_string_lossy(),
                            "modelId": id,
                            "numThreads": num_threads.to_string(),
                        })
                        .to_string(),
                        fallback: None,
                        sample_rate: Some(info.sample_rate),
                        pcm_rate: info.sample_rate,
                        ssml_capable: false,
                        display_name: display.clone(),
                        gender: "Unknown".into(),
                        quality: info.quality.clone(),
                        model_type: info.model_type.clone(),
                        languages: langs.clone(),
                        multilingual,
                        license: info.license.clone(),
                        num_speakers: info.num_speakers,
                    });
                }
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
            let multilingual = v.id.to_lowercase().contains("multilingual")
                || v.name.to_lowercase().contains("multilingual");
            voices.push(VgVoice {
                spd_name: format!("{engine_id}/{}", v.id),
                fallback: None,
                language: v.lang.clone(),
                variant: variant_for_gender(&v.gender),
                engine_id: engine_id.clone(),
                engine_voice_id: v.id.clone(),
                credentials: creds_json.clone(),
                sample_rate: None,
                pcm_rate: cloud_pcm_rate(engine_id),
                ssml_capable: engine_accepts_ssml(engine_id),
                display_name: if v.name.is_empty() {
                    v.id.clone()
                } else {
                    v.name.clone()
                },
                gender: v.gender.clone(),
                quality: String::new(),
                model_type: String::new(),
                languages: vec![v.lang.clone()],
                multilingual,
                license: String::new(),
                num_speakers: 1,
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

/// The full merged voice list for a config: local models first, then the
/// cached cloud voices of credentialed engines (+ edge).
pub fn merged_voices(cfg: &crate::config::ModuleConfig) -> Vec<VgVoice> {
    let mut list = local_voices(&cfg.models_dirs(), cfg.num_threads, &cfg.local_engine);
    let cache = load_voice_cache(&cfg.voice_cache_file);
    let credentials = load_credentials(&cfg.credentials_file);
    list.extend(cloud_voices(&cache, &credentials));
    list
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
    fn local_voices_default_to_floravox_for_drivable_families() {
        let dir = tempfile::tempdir().unwrap();
        // piper-en_US-lessac-high is in the real registry (vits family)
        let vits = dir.path().join("piper-en_US-lessac-high");
        std::fs::create_dir_all(&vits).unwrap();
        // a kitten-family id that exists in the registry
        let kitten = dir.path().join("micro-en-v0_8");
        std::fs::create_dir_all(&kitten).unwrap();

        let fv = local_voices(&[dir.path().to_path_buf()], 2, "floravox");
        assert_eq!(fv.len(), 2, "{fv:?}");
        let vits_v = fv.iter().find(|v| v.spd_name.starts_with("piper")).unwrap();
        assert_eq!(vits_v.engine_id, "floravox");
        assert!(vits_v.ssml_capable);
        // plain name, no engine prefix
        assert_eq!(vits_v.spd_name, "piper-en_US-lessac-high#0");
        // sherpa fallback attached
        let (fb_id, fb_creds) = vits_v.fallback.as_ref().unwrap();
        assert_eq!(fb_id, "sherpaonnx");
        assert!(fb_creds.contains("\"modelPath\""));
        // kitten stays on sherpa (audio-LM family)
        let kitten_v = fv
            .iter()
            .find(|v| v.spd_name.starts_with("micro-en"))
            .unwrap();
        assert_eq!(kitten_v.engine_id, "sherpaonnx");
        assert!(!kitten_v.ssml_capable);
        assert!(kitten_v.fallback.is_none());
    }

    #[test]
    fn local_voices_sherpa_pref_restores_sherpa_for_everything() {
        let dir = tempfile::tempdir().unwrap();
        let vits = dir.path().join("piper-en_US-lessac-high");
        std::fs::create_dir_all(&vits).unwrap();
        let sv = local_voices(&[dir.path().to_path_buf()], 2, "sherpaonnx");
        assert_eq!(sv.len(), 1);
        assert_eq!(sv[0].engine_id, "sherpaonnx");
        assert!(!sv[0].ssml_capable);
        // same plain name: saved selections survive the preference flip
        assert_eq!(sv[0].spd_name, "piper-en_US-lessac-high#0");
    }

    #[test]
    fn local_engine_config_parses_and_rejects() {
        let mut cfg = crate::config::ModuleConfig::default();
        assert_eq!(cfg.local_engine, "floravox");
        cfg.apply("LocalEngine sherpaonnx\n");
        assert_eq!(cfg.local_engine, "sherpaonnx");
        cfg.apply("LocalEngine nonsense\n");
        assert_eq!(
            cfg.local_engine, "sherpaonnx",
            "invalid value must not change it"
        );
    }

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

    fn fixture_voices() -> Vec<VgVoice> {
        let mk = |spd: &str,
                  engine: &str,
                  lang: &str,
                  gender: &str,
                  quality: &str,
                  langs: Vec<&str>,
                  multi: bool| VgVoice {
            spd_name: spd.into(),
            language: lang.into(),
            variant: String::new(),
            engine_id: engine.into(),
            engine_voice_id: "x".into(),
            credentials: "{}".into(),
            fallback: None,
            sample_rate: None,
            pcm_rate: 24_000,
            ssml_capable: false,
            display_name: spd.into(),
            gender: gender.into(),
            quality: quality.into(),
            model_type: "vits".into(),
            languages: langs.iter().map(|s| (*s).to_string()).collect(),
            multilingual: multi,
            license: "MIT".into(),
            num_speakers: 1,
        };
        vec![
            mk(
                "kokoro-multi#0",
                "sherpaonnx",
                "zh",
                "Unknown",
                "high",
                vec!["zh", "en"],
                true,
            ),
            mk(
                "piper-nl#0",
                "sherpaonnx",
                "nl",
                "Unknown",
                "low",
                vec!["nl"],
                false,
            ),
            mk(
                "edge/en-US-AriaNeural",
                "edge",
                "en-US",
                "Female",
                "",
                vec!["en-US"],
                false,
            ),
            mk(
                "azure/en-US-AvaMultilingualNeural",
                "azure",
                "en-US",
                "Female",
                "",
                vec!["en-US"],
                true,
            ),
            mk(
                "azure/de-DE-KatjaNeural",
                "azure",
                "de-DE",
                "Female",
                "",
                vec!["de-DE"],
                false,
            ),
            mk(
                "openai/alloy",
                "openai",
                "en",
                "Unknown",
                "",
                vec!["en"],
                false,
            ),
        ]
    }

    #[test]
    fn filter_by_source_and_gender() {
        let vs = fixture_voices();
        let f = VoiceFilter {
            source: Some(Source::Cloud),
            gender: Some("female".into()),
            ..Default::default()
        };
        let out = filter_voices(&vs, &f);
        let names: Vec<&str> = out.iter().map(|v| v.spd_name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "edge/en-US-AriaNeural",
                "azure/en-US-AvaMultilingualNeural",
                "azure/de-DE-KatjaNeural"
            ]
        );
    }

    #[test]
    fn filter_by_quality_local_only() {
        let vs = fixture_voices();
        let f = VoiceFilter {
            quality: Some("high".into()),
            ..Default::default()
        };
        let out = filter_voices(&vs, &f);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].spd_name, "kokoro-multi#0");
    }

    #[test]
    fn filter_multilingual() {
        let vs = fixture_voices();
        let f = VoiceFilter {
            multilingual: true,
            ..Default::default()
        };
        let out = filter_voices(&vs, &f);
        // local multilingual + cloud voice with "Multilingual" in the id
        let names: Vec<&str> = out.iter().map(|v| v.spd_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["kokoro-multi#0", "azure/en-US-AvaMultilingualNeural"]
        );
    }

    #[test]
    fn filter_lang_base_and_exact() {
        let vs = fixture_voices();
        let base = VoiceFilter {
            lang: Some("en".into()),
            ..Default::default()
        };
        assert_eq!(filter_voices(&vs, &base).len(), 4);
        let exact = VoiceFilter {
            lang: Some("en-GB".into()),
            ..Default::default()
        };
        assert_eq!(filter_voices(&vs, &exact).len(), 0);
    }

    #[test]
    fn filter_engine_and_terms() {
        let vs = fixture_voices();
        let f = VoiceFilter {
            engines: vec!["AZURE".into()],
            terms: vec!["katja".into()],
            ..Default::default()
        };
        let out = filter_voices(&vs, &f);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].spd_name, "azure/de-DE-KatjaNeural");
    }
}

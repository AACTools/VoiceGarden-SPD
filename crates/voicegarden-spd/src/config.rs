//! Module configuration file parsing.
//!
//! The file (conventionally `voicegarden-spd.conf`, passed as `argv[1]` by
//! speech-dispatcher's `AddModule` directive) uses the same simple
//! `Key value` / `Key "value"` line syntax as the stock modules.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ModuleConfig {
    /// Directory containing downloaded sherpa-onnx models (one subdirectory
    /// per model id).
    pub models_dir: PathBuf,
    /// JSON file mapping engine id → credentials object.
    pub credentials_file: PathBuf,
    /// Voice-list cache written by `voicegarden-spd-refresh`.
    pub voice_cache_file: PathBuf,
    /// Fully-qualified VoiceGarden voice name used when the server's
    /// voice/language selection matches nothing.
    pub default_voice: Option<String>,
    /// Target size of audio chunks streamed to the server, in milliseconds.
    pub chunk_ms: u32,
    /// ONNX runtime intra-op thread count for sherpa-onnx models.
    pub num_threads: i32,
}

impl Default for ModuleConfig {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        Self {
            models_dir: PathBuf::from(&home).join(".rust-tts-wrapper/sherpaonnx"),
            credentials_file: PathBuf::from(&home).join(".config/voicegarden-spd/engines.json"),
            voice_cache_file: PathBuf::from(&home).join(".cache/voicegarden-spd/voices.json"),
            default_voice: None,
            chunk_ms: 250,
            num_threads: 2,
        }
    }
}

impl ModuleConfig {
    /// Parse a configuration file. Unknown keys are ignored (with a
    /// warning to stderr) so future versions stay compatible. A missing
    /// file yields the defaults.
    pub fn load(path: Option<&str>) -> Self {
        let mut cfg = Self::default();
        let Some(path) = path else {
            return cfg;
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            eprintln!("voicegarden-spd: config file {path} not readable, using defaults");
            return cfg;
        };
        cfg.apply(&text);
        cfg
    }

    /// Apply `Key value` lines to this config.
    pub fn apply(&mut self, text: &str) {
        for (lineno, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = match line.split_once(char::is_whitespace) {
                Some((k, v)) => (k.trim(), v.trim()),
                None => continue,
            };
            let value = unquote(value);
            match key {
                "ModelsDir" => self.models_dir = PathBuf::from(expand(&value)),
                "CredentialsFile" => self.credentials_file = PathBuf::from(expand(&value)),
                "VoiceCacheFile" => self.voice_cache_file = PathBuf::from(expand(&value)),
                "DefaultVoice" => self.default_voice = Some(value),
                "ChunkMs" => {
                    if let Ok(ms) = value.parse::<u32>() {
                        self.chunk_ms = ms.clamp(20, 2000);
                    }
                }
                "NumThreads" => {
                    if let Ok(t) = value.parse::<i32>() {
                        if t > 0 {
                            self.num_threads = t;
                        }
                    }
                }
                _ => {
                    eprintln!(
                        "voicegarden-spd: ignoring unknown config key {key:?} (line {})",
                        lineno + 1
                    );
                }
            }
        }
    }
}

/// Strip matching double quotes from a value.
fn unquote(v: &str) -> String {
    let v = v.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        v[1..v.len() - 1].to_string()
    } else {
        v.to_string()
    }
}

/// Expand `$HOME`-style environment references.
fn expand(v: &str) -> String {
    if let Some(rest) = v.strip_prefix("$HOME") {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{home}{rest}")
    } else if let Some(rest) = v.strip_prefix('~') {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{home}{rest}")
    } else {
        v.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_documented_keys() {
        let mut cfg = ModuleConfig::default();
        cfg.apply(
            "# comment\nModelsDir \"/opt/models\"\nCredentialsFile /etc/vg/creds.json\nDefaultVoice \"kokoro-en-v0_19#1\"\nChunkMs 100\n",
        );
        assert_eq!(cfg.models_dir, PathBuf::from("/opt/models"));
        assert_eq!(cfg.credentials_file, PathBuf::from("/etc/vg/creds.json"));
        assert_eq!(cfg.default_voice.as_deref(), Some("kokoro-en-v0_19#1"));
        assert_eq!(cfg.chunk_ms, 100);
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let mut cfg = ModuleConfig::default();
        cfg.apply("FutureKey whatever\nChunkMs 99999\n");
        assert_eq!(cfg.chunk_ms, 2000, "ChunkMs clamps to 2000");
    }

    #[test]
    fn tilde_expansion() {
        std::env::set_var("HOME", "/home/tester");
        let mut cfg = ModuleConfig::default();
        cfg.apply("ModelsDir ~/models\n");
        assert_eq!(cfg.models_dir, PathBuf::from("/home/tester/models"));
    }
}

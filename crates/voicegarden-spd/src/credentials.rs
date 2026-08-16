//! Engine credential management for the CLI: the engines.json store,
//! engine descriptors (required keys), masking, and live verification.

use std::collections::BTreeMap;
use std::path::Path;

/// One row of engine knowledge, merged from rust-tts-wrapper's factory.
#[derive(Debug, Clone)]
pub struct EngineInfo {
    pub id: String,
    pub name: String,
    /// True when the engine needs credentials to synthesise.
    pub needs_credentials: bool,
    /// Credential key names, in prompt order (empty for no-cred engines).
    pub keys: Vec<String>,
}

/// All engines this build knows, minus `system` (routing speech-dispatcher
/// back into itself) and platform-native engines that don't apply.
#[must_use]
pub fn known_engines() -> Vec<EngineInfo> {
    rust_tts_wrapper::factory::engine_list()
        .into_iter()
        .filter(|e| e.id != "system" && e.id != "avsynth" && e.id != "sapi")
        .map(|e| EngineInfo {
            keys: serde_json::from_str::<Vec<String>>(&e.credential_keys_json).unwrap_or_default(),
            id: e.id,
            name: e.name,
            needs_credentials: e.needs_credentials,
        })
        .collect()
}

/// Look up one engine descriptor.
#[must_use]
pub fn engine_info(id: &str) -> Option<EngineInfo> {
    known_engines().into_iter().find(|e| e.id == id)
}

/// Load engines.json as an ordered map (missing file → empty).
#[must_use]
pub fn load(path: &Path) -> BTreeMap<String, serde_json::Map<String, serde_json::Value>> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Save engines.json with mode 0600 (creating parent dirs as needed).
pub fn save(
    path: &Path,
    engines: &BTreeMap<String, serde_json::Map<String, serde_json::Value>>,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(engines).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Mask a secret for display: first 4 chars + "…" + last 2 for long
/// values, fully masked for short ones.
#[must_use]
pub fn mask(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 8 {
        "••••".to_string()
    } else {
        let head: String = chars.iter().take(4).collect();
        let tail: String = chars.iter().skip(chars.len() - 2).collect();
        format!("{head}…{tail}")
    }
}

/// Should a credential key be entered with hidden input?
#[must_use]
pub fn is_secret_key(key: &str) -> bool {
    let k = key.to_lowercase();
    k.contains("key") || k.contains("token") || k.contains("secret") || k.contains("password")
}

/// Outcome of a live credential check.
pub struct CheckResult {
    pub ok: bool,
    pub elapsed_ms: u128,
    pub voice_count: Option<usize>,
    pub detail: String,
}

/// Create the engine and verify live (credentials + API reachability).
/// Voice count is reported when the engine can enumerate without extra
/// calls beyond the check itself.
pub fn check(id: &str, credentials_json: &str) -> CheckResult {
    let t0 = std::time::Instant::now();
    let Some(engine) = rust_tts_wrapper::create_engine(id, credentials_json) else {
        return CheckResult {
            ok: false,
            elapsed_ms: t0.elapsed().as_millis(),
            voice_count: None,
            detail: format!("engine '{id}' not available in this build"),
        };
    };
    match engine.check_credentials() {
        Ok(true) => {
            let voices = engine.get_voices().map(|v| v.len()).ok();
            CheckResult {
                ok: true,
                elapsed_ms: t0.elapsed().as_millis(),
                voice_count: voices,
                detail: "credentials verified".into(),
            }
        }
        Ok(false) => CheckResult {
            ok: false,
            elapsed_ms: t0.elapsed().as_millis(),
            voice_count: None,
            detail: "server rejected the credentials".into(),
        },
        Err(e) => CheckResult {
            ok: false,
            elapsed_ms: t0.elapsed().as_millis(),
            voice_count: None,
            detail: e.to_string(),
        },
    }
}

/// JSON wire representation for `check`.
#[must_use]
pub fn creds_to_json(values: &BTreeMap<String, String>) -> String {
    let map: serde_json::Map<String, serde_json::Value> = values
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();
    serde_json::Value::Object(map).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_short_and_long() {
        assert_eq!(mask("abc"), "••••");
        assert_eq!(mask("12345678"), "••••");
        assert_eq!(mask("AIzaSyDsu-p6Tel1io9cOW7tnhqg5Rp8g0lgUHA"), "AIza…HA");
    }

    #[test]
    fn secret_key_detection() {
        assert!(is_secret_key("apiKey"));
        assert!(is_secret_key("subscriptionKey"));
        assert!(is_secret_key("token"));
        assert!(is_secret_key("secretAccessKey"));
        assert!(!is_secret_key("region"));
        assert!(!is_secret_key("instanceId"));
        assert!(!is_secret_key("userId"));
    }

    #[test]
    fn save_load_roundtrip_and_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/engines.json");
        let mut store = BTreeMap::new();
        let mut m = serde_json::Map::new();
        m.insert("apiKey".into(), serde_json::Value::String("sk-test".into()));
        store.insert("openai".into(), m);
        save(&path, &store).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "credentials must be 0600");
        }
        let loaded = load(&path);
        assert!(loaded.contains_key("openai"));
    }

    #[test]
    fn known_engines_exclude_system_and_have_keys() {
        let engines = known_engines();
        assert!(engines.iter().all(|e| e.id != "system"));
        let azure = engines.iter().find(|e| e.id == "azure").unwrap();
        assert!(azure.needs_credentials);
        assert_eq!(azure.keys, vec!["subscriptionKey", "region"]);
        let edge = engines.iter().find(|e| e.id == "edge").unwrap();
        assert!(!edge.needs_credentials);
        assert!(edge.keys.is_empty());
    }

    #[test]
    fn creds_json_string_values() {
        let mut m = BTreeMap::new();
        m.insert("region".to_string(), "uksouth".to_string());
        assert_eq!(creds_to_json(&m), r#"{"region":"uksouth"}"#);
    }
}

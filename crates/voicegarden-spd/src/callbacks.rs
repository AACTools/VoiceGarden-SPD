//! The `module_*` callbacks required by `libspeechd_module`, plus the
//! process-wide state they operate on.
//!
//! Threading: all callbacks are invoked from the main thread (inside
//! `module_process`), except that the synthesis worker spawned by
//! `pipeline::speak` reads the `STOP_REQUESTED` flag. State is therefore
//! guarded conservatively (atomics + mutexes) rather than assuming
//! single-threaded access.

use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

use rust_tts_wrapper::engine::TtsEngine;

use crate::config::ModuleConfig;
use crate::glue::{self, msgtype, SPDVoice, STDIN_FILENO};
use crate::pipeline::{self, Prosody};
use crate::ssml::strip_ssml_with_marks;
use crate::voices::{
    cloud_voices, load_credentials, load_voice_cache, local_sherpa_voices, VgVoice,
};

/// Set by STOP and PAUSE; cleared at the start of each utterance.
static STOP_REQUESTED: LazyLock<Arc<AtomicBool>> =
    LazyLock::new(|| Arc::new(AtomicBool::new(false)));

static LOG_LEVEL: AtomicI32 = AtomicI32::new(1);

static CONFIG: OnceLock<ModuleConfig> = OnceLock::new();

/// Server-provided message settings (SSIP `SET`), snapshotted per
/// utterance. Values follow the speechd conventions (rate/pitch/volume in
/// -100..100, voice as enum name, language as ISO code, "NULL" = unset).
#[derive(Debug, Clone)]
struct MsgSettings {
    rate: i32,
    pitch: i32,
    pitch_range: i32,
    volume: i32,
    voice_type: i32, // -1 unspecified, else SPDVoiceType value
    language: Option<String>,
    synthesis_voice: Option<String>,
}

impl Default for MsgSettings {
    fn default() -> Self {
        Self {
            rate: 0,
            pitch: 0,
            pitch_range: 0,
            volume: 0,
            voice_type: -1,
            language: None,
            synthesis_voice: None,
        }
    }
}

static SETTINGS: Mutex<Option<MsgSettings>> = Mutex::new(None);

static VOICES: OnceLock<Vec<VgVoice>> = OnceLock::new();

/// The C voice array handed to the library. Built once; owned for the
/// process lifetime (the library only reads it). Stored as an address
/// because raw pointers are not `Send`/`Sync`.
static C_VOICES: OnceLock<usize> = OnceLock::new();

static ENGINE_CACHE: LazyLock<Mutex<HashMap<String, Arc<dyn TtsEngine>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn config() -> &'static ModuleConfig {
    CONFIG.get_or_init(ModuleConfig::default)
}

fn voices() -> &'static [VgVoice] {
    VOICES.get().map_or(&[], Vec::as_slice)
}

/// Build (or fetch from cache) the engine for a voice.
fn engine_for(voice: &VgVoice) -> Option<Arc<dyn TtsEngine>> {
    let cache_key = format!("{}:{}", voice.engine_id, voice.credentials);
    let mut cache = ENGINE_CACHE.lock().ok()?;
    if let Some(engine) = cache.get(&cache_key) {
        return Some(Arc::clone(engine));
    }
    let engine = rust_tts_wrapper::create_engine(&voice.engine_id, &voice.credentials)?;
    cache.insert(cache_key, Arc::clone(&engine));
    Some(engine)
}

/// Choose the voice for the current settings: exact `synthesis_voice`
/// match, then language + voice-type matching, then the configured
/// default, then the first available voice.
fn resolve_voice(settings: &MsgSettings) -> Option<VgVoice> {
    let all = voices();
    if all.is_empty() {
        return None;
    }
    if let Some(wanted) = &settings.synthesis_voice {
        if let Some(v) = all.iter().find(|v| v.spd_name.eq_ignore_ascii_case(wanted)) {
            return Some(v.clone());
        }
    }
    if let Some(lang) = &settings.language {
        let want_type = voice_type_name(settings.voice_type);
        // Score every voice and keep the best language match, breaking
        // ties on voice-type.
        let mut best: Option<(u8, u8, &VgVoice)> = None;
        for v in all {
            let lang_score = language_score(lang, &v.language);
            if lang_score == 0 {
                continue;
            }
            let type_score = match want_type {
                Some(t) if !t.is_empty() && v.variant.eq_ignore_ascii_case(t) => 1,
                _ => 0,
            };
            let better = match &best {
                None => true,
                Some((bl, bt, _)) => (lang_score, type_score) > (*bl, *bt),
            };
            if better {
                best = Some((lang_score, type_score, v));
            }
        }
        if let Some((_, _, v)) = best {
            return Some(v.clone());
        }
    }
    if let Some(default) = &config().default_voice {
        if let Some(v) = all.iter().find(|v| v.spd_name == *default) {
            return Some(v.clone());
        }
    }
    all.first().cloned()
}

/// Language match quality: 3 = exact, 2 = voice is more specific
/// ("en-US" for requested "en"), 1 = same base language.
fn language_score(requested: &str, offered: &str) -> u8 {
    let req = requested.to_ascii_lowercase();
    let off = offered.to_ascii_lowercase();
    if req == off {
        return 3;
    }
    if off.starts_with(&format!("{req}-")) {
        return 2;
    }
    let req_base = req.split('-').next().unwrap_or(&req);
    let off_base = off.split('-').next().unwrap_or(&off);
    if !req_base.is_empty() && req_base == off_base {
        return 1;
    }
    0
}

/// SPDVoiceType number → variant name used in our voice list.
fn voice_type_name(v: i32) -> Option<&'static str> {
    match v {
        1 => Some("male1"),
        2 => Some("male2"),
        3 => Some("male3"),
        4 => Some("female1"),
        5 => Some("female2"),
        6 => Some("female3"),
        7 => Some("child_male"),
        8 => Some("child_female"),
        _ => None,
    }
}

/// Allocate a C string with libc `malloc` (the library frees some of these
/// with `free()`).
fn malloc_cstring(s: &str) -> *mut c_char {
    unsafe {
        let bytes = s.as_bytes();
        let ptr = libc::malloc(bytes.len() + 1) as *mut c_char;
        if ptr.is_null() {
            return ptr;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, ptr, bytes.len());
        *ptr.add(bytes.len()) = 0;
        ptr
    }
}

// ---------------------------------------------------------------------------
// module_* callbacks (called by libspeechd_module)
// ---------------------------------------------------------------------------

/// Parse the module configuration file (argv[1] from our own main).
#[no_mangle]
pub extern "C" fn module_config(configfile: *const c_char) -> c_int {
    let path = if configfile.is_null() {
        None
    } else {
        unsafe { CStr::from_ptr(configfile) }.to_str().ok()
    };
    let cfg = ModuleConfig::load(path);
    let _ = CONFIG.set(cfg);
    0
}

#[no_mangle]
pub extern "C" fn module_init(msg: *mut *mut c_char) -> c_int {
    unsafe { glue::module_audio_set_server() };

    let cfg = config();
    let credentials = load_credentials(&cfg.credentials_file);
    let cache = load_voice_cache(&cfg.voice_cache_file);
    let mut list = local_sherpa_voices(&cfg.models_dir, cfg.num_threads);
    let cloud_count = {
        let cloud = cloud_voices(&cache, &credentials);
        let n = cloud.len();
        list.extend(cloud);
        n
    };
    let local_count = list.len() - cloud_count;

    let status = if list.is_empty() {
        format!(
            "VoiceGarden-SPD {}: no voices. Install sherpa-onnx models under {} \
             and/or run voicegarden-spd-refresh for cloud voices.",
            env!("CARGO_PKG_VERSION"),
            cfg.models_dir.display()
        )
    } else {
        format!(
            "VoiceGarden-SPD {}: {} voices ({local_count} local sherpa-onnx, \
             {cloud_count} cloud)",
            env!("CARGO_PKG_VERSION"),
            list.len()
        )
    };
    eprintln!("voicegarden-spd: {status}");

    let c_list = build_c_voices(&list);
    let _ = VOICES.set(list);
    let _ = C_VOICES.set(c_list as usize);

    if !msg.is_null() {
        unsafe {
            *msg = malloc_cstring(&status);
        }
    }
    0
}

/// Build the NULL-terminated `SPDVoice**` array. Leaks intentionally:
/// owned for the process lifetime, freed never (the library only reads).
fn build_c_voices(list: &[VgVoice]) -> *mut *mut SPDVoice {
    unsafe {
        let arr = libc::malloc((list.len() + 1) * std::mem::size_of::<*mut SPDVoice>())
            as *mut *mut SPDVoice;
        if arr.is_null() {
            return std::ptr::null_mut();
        }
        for (i, v) in list.iter().enumerate() {
            let voice = libc::malloc(std::mem::size_of::<SPDVoice>()) as *mut SPDVoice;
            (*voice).name = malloc_cstring(&v.spd_name);
            (*voice).language = malloc_cstring(&v.language);
            (*voice).variant = malloc_cstring(&v.variant);
            *arr.add(i) = voice;
        }
        *arr.add(list.len()) = std::ptr::null_mut();
        arr
    }
}

#[no_mangle]
pub extern "C" fn module_list_voices() -> *mut *mut SPDVoice {
    C_VOICES
        .get()
        .map_or(std::ptr::null_mut(), |addr| *addr as *mut *mut SPDVoice)
}

#[no_mangle]
pub extern "C" fn module_speak_sync(data: *const c_char, bytes: usize, msgtype_: c_int) {
    STOP_REQUESTED.store(false, Ordering::SeqCst);

    let text = if data.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(data) }
            .to_str()
            .unwrap_or_default()
            .to_string()
    };
    let _ = bytes;

    let settings = SETTINGS
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .unwrap_or_default();

    let Some(voice) = resolve_voice(&settings) else {
        eprintln!("voicegarden-spd: no voice available for speak request");
        unsafe { glue::module_speak_error() };
        return;
    };
    let Some(engine) = engine_for(&voice) else {
        eprintln!(
            "voicegarden-spd: engine '{}' unavailable (not compiled in?)",
            voice.engine_id
        );
        unsafe { glue::module_speak_error() };
        return;
    };

    let (clean, marks) = strip_ssml_with_marks(&text);
    if clean.trim().is_empty() && msgtype_ != msgtype::SOUND_ICON {
        unsafe { glue::module_speak_error() };
        return;
    }

    unsafe { glue::module_speak_ok() };

    // Sound icons and single characters are spoken as text (no icon files
    // shipped); everything else is a normal utterance.
    let spoken = if msgtype_ == msgtype::SOUND_ICON && clean.trim().is_empty() {
        text.trim().to_string()
    } else {
        clean
    };

    let prosody = Prosody::from_spd(settings.rate, settings.pitch, settings.volume);
    pipeline::speak(
        engine,
        &voice,
        &spoken,
        prosody,
        &marks,
        config().chunk_ms,
        &STOP_REQUESTED,
        &|| unsafe {
            glue::module_process(STDIN_FILENO, 0);
        },
    );
}

#[no_mangle]
pub extern "C" fn module_stop() -> c_int {
    STOP_REQUESTED.store(true, Ordering::SeqCst);
    0
}

#[no_mangle]
pub extern "C" fn module_pause() -> usize {
    // We don't support mid-audio pause; abort the utterance (the server
    // re-speaks from the pause mark on resume, like most modules).
    STOP_REQUESTED.store(true, Ordering::SeqCst);
    0
}

#[no_mangle]
pub extern "C" fn module_close() -> c_int {
    0
}

#[no_mangle]
pub extern "C" fn module_set(var: *const c_char, val: *const c_char) -> c_int {
    let (Some(var), Some(val)) = (safe_str(var), safe_str(val)) else {
        return -1;
    };
    let mut guard = match SETTINGS.lock() {
        Ok(g) => g,
        Err(_) => return -1,
    };
    let settings = guard.get_or_insert_with(MsgSettings::default);

    let int_val = || -> Option<i32> { val.trim().parse::<i32>().ok() };

    match var.as_str() {
        "rate" | "pitch" | "pitch_range" | "volume" => {
            let Some(n) = int_val() else { return -1 };
            if !(-100..=100).contains(&n) {
                return -1;
            }
            match var.as_str() {
                "rate" => settings.rate = n,
                "pitch" => settings.pitch = n,
                "pitch_range" => settings.pitch_range = n,
                _ => settings.volume = n,
            }
            0
        }
        "voice" => {
            let Some(n) = parse_voice_type(&val) else {
                return -1;
            };
            settings.voice_type = n;
            0
        }
        "synthesis_voice" => {
            settings.synthesis_voice = null_or_value(&val);
            0
        }
        "language" => {
            settings.language = null_or_value(&val);
            0
        }
        // Punctuation/spelling/capital modes are accepted but not yet
        // applied (the engines handle punctuation themselves).
        "punctuation_mode" | "spelling_mode" | "cap_let_recogn" => 0,
        _ => -1,
    }
}

#[no_mangle]
pub extern "C" fn module_audio_set(var: *const c_char, val: *const c_char) -> c_int {
    match (safe_str(var).as_deref(), safe_str(val).as_deref()) {
        (Some("audio_output_method"), Some("server")) => 0,
        _ => -1,
    }
}

#[no_mangle]
pub extern "C" fn module_audio_init(status: *mut *mut c_char) -> c_int {
    // Only reached if the server-audio negotiation above failed, which
    // never happens with a 0.11+ server. Report success and keep going.
    if !status.is_null() {
        unsafe {
            *status = malloc_cstring("VoiceGarden-SPD routes audio through the server");
        }
    }
    0
}

#[no_mangle]
pub extern "C" fn module_loglevel_set(var: *const c_char, val: *const c_char) -> c_int {
    if let (Some("log_level"), Some(v)) = (safe_str(var).as_deref(), safe_str(val)) {
        if let Ok(n) = v.trim().parse::<i32>() {
            LOG_LEVEL.store(n, Ordering::Relaxed);
            return 0;
        }
    }
    -1
}

#[no_mangle]
pub extern "C" fn module_debug(_enable: c_int, _file: *const c_char) -> c_int {
    // Debugging to a custom file is not implemented; accept so the server
    // does not report an error (our stderr goes to the speechd log).
    0
}

#[no_mangle]
pub extern "C" fn module_loop() -> c_int {
    unsafe { glue::module_process(STDIN_FILENO, 1) }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Copy a C string into an owned `String` (NULL-safe). The library's
/// strings live in its own buffers only for the duration of the call, so
/// we never borrow them.
fn safe_str(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(p) }
        .to_str()
        .ok()
        .map(str::to_string)
}

fn null_or_value(v: &str) -> Option<String> {
    if v == "NULL" || v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

fn parse_voice_type(v: &str) -> Option<i32> {
    match v.to_ascii_uppercase().as_str() {
        "MALE1" => Some(1),
        "MALE2" => Some(2),
        "MALE3" => Some(3),
        "FEMALE1" => Some(4),
        "FEMALE2" => Some(5),
        "FEMALE3" => Some(6),
        "CHILD_MALE" => Some(7),
        "CHILD_FEMALE" => Some(8),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn language_scoring() {
        assert_eq!(language_score("en", "en"), 3);
        assert_eq!(language_score("en", "en-US"), 2);
        assert_eq!(language_score("en-US", "en"), 1);
        assert_eq!(language_score("fr", "en-US"), 0);
        assert_eq!(language_score("EN-us", "en-US"), 3);
    }

    #[test]
    fn voice_type_parsing() {
        assert_eq!(parse_voice_type("MALE1"), Some(1));
        assert_eq!(parse_voice_type("female3"), Some(6));
        assert_eq!(parse_voice_type("bogus"), None);
    }

    #[test]
    fn malloc_cstring_roundtrip() {
        let ptr = malloc_cstring("hello");
        assert!(!ptr.is_null());
        unsafe {
            assert_eq!(CStr::from_ptr(ptr).to_bytes(), b"hello");
            libc::free(ptr as *mut libc::c_void);
        }
    }

    #[test]
    fn module_set_stores_settings() {
        assert_eq!(
            module_set(
                CString::new("rate").unwrap().as_ptr(),
                CString::new("50").unwrap().as_ptr()
            ),
            0
        );
        assert_eq!(
            module_set(
                CString::new("rate").unwrap().as_ptr(),
                CString::new("500").unwrap().as_ptr()
            ),
            -1
        );
        let guard = SETTINGS.lock().unwrap();
        let s = guard.as_ref().unwrap();
        assert_eq!(s.rate, 50);
    }
}

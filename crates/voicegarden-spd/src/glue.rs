//! FFI declarations for the speech-dispatcher output-module protocol
//! machinery (`module_process.c` / `module_readline.c`, vendored under
//! `vendor/` and compiled into this crate by `build.rs`).
//!
//! The vendored code implements the server-side of the module protocol:
//! it parses incoming SSIP-module commands (SPEAK/STOP/PAUSE/SET/LIST
//! VOICES/...), escapes and streams audio to the server with
//! `module_tts_output_server`, and prints event replies. Our side of the
//! contract is a set of `module_*` callbacks declared in
//! `vendor/spd_module_main.h` — see `callbacks.rs`.

#![allow(non_camel_case_types)]

use std::os::raw::{c_char, c_int};

/// Standard input file descriptor (the server talks to us over pipes).
pub const STDIN_FILENO: c_int = 0;

/// `SPDVoice` from `speechd_types.h`: a NULL-terminated triple of strings.
/// The array returned by `module_list_voices` is NULL-terminated and owned
/// by us; the library only reads it.
#[repr(C)]
pub struct SPDVoice {
    pub name: *mut c_char,
    pub language: *mut c_char,
    pub variant: *mut c_char,
}

/// `AudioTrack` from `spd_audio_plugin.h`. `samples` points at interleaved
/// 16-bit PCM, `num_samples` is frames (not bytes).
#[repr(C)]
pub struct AudioTrack {
    pub bits: c_int,
    pub num_channels: c_int,
    pub sample_rate: c_int,
    pub num_samples: c_int,
    pub samples: *mut i16,
}

/// `AudioFormat`: 0 = little-endian, 1 = big-endian.
pub type AudioFormat = c_int;
pub const SPD_AUDIO_LE: AudioFormat = 0;

/// `SPDMessageType` values (subset we care about).
pub mod msgtype {
    pub const TEXT: i32 = 0;
    pub const SOUND_ICON: i32 = 1;
    pub const CHAR: i32 = 2;
    pub const KEY: i32 = 3;
    pub const SPELL: i32 = 99;
}

extern "C" {
    /// Read one line from `fd`. Returns a `malloc`ed line (free with
    /// `libc::free`) including the trailing newline, or NULL when `block`
    /// is 0 and no full line is buffered. Exits the process on I/O error.
    pub fn module_readline(fd: c_int, block: c_int) -> *mut c_char;

    /// Process buffered (block=0) or all incoming (block=1) commands,
    /// dispatching them to our callbacks. Returns -1 when nothing was
    /// buffered (non-blocking) — not an error. Mirrors usage inside
    /// `module_tts_output_server`, which ignores the return value.
    pub fn module_process(fd: c_int, block: c_int) -> c_int;

    /// Reply `200 OK SPEAKING` — call from `module_speak_sync` once the
    /// message has been accepted, before any events.
    pub fn module_speak_ok();

    /// Reply `301 ERROR CANT SPEAK` — call when the message is rejected.
    pub fn module_speak_error();

    /// Report `701 BEGIN`.
    pub fn module_report_event_begin();

    /// Report `702 END`. With server audio the module reports END even
    /// after a stop; the server's speak queue turns that into the correct
    /// client-side stop/pause/end event.
    pub fn module_report_event_end();

    /// Report `700 INDEX MARK`. The server's speak queue fires the client
    /// event when playback reaches the audio position the mark was
    /// reported at, so marks must be interleaved between audio chunks.
    pub fn module_report_index_mark(mark: *const c_char);

    /// Report `706 ICON` (sound icon played).
    pub fn module_report_icon(icon: *const c_char);

    /// Send an audio chunk to the server (HDLC-escaped on the wire) and
    /// poll for STOP between internal ~10 KB pieces. Must be called from
    /// the same thread that runs `module_process`.
    pub fn module_tts_output_server(track: *const AudioTrack, format: AudioFormat);

    /// Tell the library we will route audio through the server; makes the
    /// AUDIO negotiation accept `audio_output_method=server`.
    pub fn module_audio_set_server();
}

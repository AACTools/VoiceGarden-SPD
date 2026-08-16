//! VoiceGarden-SPD internals shared between the `sd_voicegarden` module
//! binary, the `voicegarden-spd-refresh` helper, and the `voicegarden-spd`
//! management CLI.

// The module_* callbacks are extern "C" entry points invoked by
// libspeechd_module with pointers it owns; their signatures are fixed by
// the C header, so the raw-pointer lints don't apply.
#![allow(
    clippy::not_unsafe_ptr_arg_deref,
    clippy::missing_safety_doc,
    non_camel_case_types
)]

pub mod callbacks;
pub mod config;
pub mod glue;
pub mod installer;
pub mod pipeline;
pub mod refresh;
pub mod ssml;
pub mod voices;

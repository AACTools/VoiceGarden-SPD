//! sd_voicegarden — VoiceGarden speech-dispatcher output module.
//!
//! Binary layout mirrors upstream `module_main.c`: read config, perform
//! the INIT handshake, then run `module_loop` (blocking `module_process`).
//! We link `libspeechd_module.a` statically for the protocol machinery;
//! `module_main.o` from that archive is never pulled because we define
//! our own `main`.

use std::ffi::CStr;
use std::os::raw::c_char;

use voicegarden_spd::callbacks;
use voicegarden_spd::glue::{self, STDIN_FILENO};

fn main() {
    let configfile: Option<std::ffi::CString> = std::env::args_os()
        .nth(1)
        .and_then(|a| std::ffi::CString::new(a.into_string().ok()?).ok());

    // Read configuration
    let ret = callbacks::module_config(
        configfile
            .as_ref()
            .map_or(std::ptr::null(), |c| c.as_ptr()),
    );
    if ret != 0 {
        callbacks::module_close();
        std::process::exit(1);
    }

    // Wait for the server's INIT line. module_readline owns the input
    // buffering, so we must not touch stdin ourselves.
    let line = unsafe { glue::module_readline(STDIN_FILENO, 1) };
    if line.is_null() {
        eprintln!("voicegarden-spd: EOF before INIT");
        std::process::exit(2);
    }
    let is_init = unsafe { CStr::from_ptr(line) }.to_bytes() == b"INIT\n";
    unsafe { libc::free(line as *mut libc::c_void) };
    if !is_init {
        eprintln!("voicegarden-spd: server did not start with INIT");
        callbacks::module_close();
        std::process::exit(3);
    }

    // Initialize the module
    let mut msg: *mut c_char = std::ptr::null_mut();
    if callbacks::module_init(&mut msg) != 0 {
        let text = if msg.is_null() {
            "Unspecified initialization error".to_string()
        } else {
            unsafe { CStr::from_ptr(msg) }.to_string_lossy().into_owned()
        };
        println!("399-{text}");
        println!("399 ERR CANT INIT MODULE");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        if !msg.is_null() {
            unsafe { libc::free(msg as *mut libc::c_void) };
        }
        callbacks::module_close();
        std::process::exit(1);
    }
    let text = if msg.is_null() {
        "Unspecified initialization success".to_string()
    } else {
        unsafe { CStr::from_ptr(msg) }.to_string_lossy().into_owned()
    };
    println!("299-{text}");
    println!("299 OK LOADED SUCCESSFULLY");
    use std::io::Write;
    let _ = std::io::stdout().flush();
    if !msg.is_null() {
        unsafe { libc::free(msg as *mut libc::c_void) };
    }

    // Run the module until the server quits or the pipe breaks.
    let ret = callbacks::module_loop();
    if ret != 0 {
        println!("399 ERR MODULE CLOSED");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        callbacks::module_close();
    }
    std::process::exit(ret);
}

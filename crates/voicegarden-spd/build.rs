use std::path::PathBuf;

fn main() {
    // The speech-dispatcher output-module protocol helpers ship in
    // libspeechd_module (from libspeechd-dev). We link statically so our
    // exported module_* symbols resolve its undefined references at link
    // time; the archive only contains module_main.o, module_readline.o and
    // module_process.o, and module_main.o is never pulled because we define
    // our own main().
    let libdir = pkg_config_libdir().unwrap_or_else(|| {
        for candidate in [
            "/usr/lib/x86_64-linux-gnu",
            "/usr/lib64",
            "/usr/lib",
            "/usr/local/lib",
        ] {
            let p = PathBuf::from(candidate).join("libspeechd_module.a");
            if p.exists() {
                return candidate.to_string();
            }
        }
        "/usr/lib".to_string()
    });

    println!("cargo:rustc-link-search=native={libdir}");
    println!("cargo:rustc-link-lib=static=speechd_module");
    println!("cargo:rerun-if-changed=build.rs");
}

/// Ask pkg-config for the speech-dispatcher library directory.
fn pkg_config_libdir() -> Option<String> {
    let out = std::process::Command::new("pkg-config")
        .args(["--variable=libdir", "speech-dispatcher"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if dir.is_empty() {
        None
    } else {
        Some(dir)
    }
}

fn main() {
    // Compile the vendored speech-dispatcher module-protocol sources
    // (module_process.c + module_readline.c, BSD-2-Clause — see
    // vendor/LICENSE.md) straight into the crate. This replaces linking
    // the distro's libspeechd_module.a, which only exists on
    // speech-dispatcher 0.12+ distros (Ubuntu 24.04 still ships 0.11).
    let vendor = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor");

    let mut build = cc::Build::new();
    build
        .file(vendor.join("module_process.c"))
        .file(vendor.join("module_readline.c"))
        .include(&vendor)
        .flag_if_supported("-Wno-unused-parameter")
        .compile("vgspd_module_proto");

    println!("cargo:rerun-if-changed={}", vendor.display());
}

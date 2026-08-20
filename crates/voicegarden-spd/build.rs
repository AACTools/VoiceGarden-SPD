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

    // ort-sys (floravox's onnxruntime) bundles C++ sources that need the
    // C++ runtime at final link. sherpa-onnx-sys's build script used to
    // emit this as a side effect; now that sherpa is gone we do it here.
    // (ort-sys emits it too, but only in its static-link path — the
    // download path relies on the consumer, which broke aarch64.)
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("apple") {
        println!("cargo:rustc-link-lib=c++");
    } else if !target.contains("msvc") {
        println!("cargo:rustc-link-lib=dylib=stdc++");
    }
}

fn main() {
    // ort-sys (floravox's onnxruntime, pulled in via the model registry
    // search) bundles C++ sources that need the C++ runtime at final
    // link. sherpa-onnx-sys's build script used to emit this as a side
    // effect; now that sherpa is gone we do it here too (voicegarden-spd
    // has the same block).
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("apple") {
        println!("cargo:rustc-link-lib=c++");
    } else if !target.contains("msvc") {
        println!("cargo:rustc-link-lib=dylib=stdc++");
    }
}

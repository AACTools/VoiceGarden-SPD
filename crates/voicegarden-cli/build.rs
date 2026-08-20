fn main() {
    // ort-sys (floravox's onnxruntime, pulled in via the model registry
    // search) bundles C++ sources that need the C++ runtime at final
    // link. sherpa-onnx-sys's build script used to emit this as a side
    // effect; now that sherpa is gone we do it here too (voicegarden-spd
    // has the same block).
}

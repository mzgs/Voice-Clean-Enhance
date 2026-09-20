# Vendored DeepFilterNet

Source: https://github.com/Rikorose/DeepFilterNet
Tag: v0.5.6
Commit: 978576aa8400552a4ce9730838c635aa30db5e61

Included: libDF Rust library, README, and models/DeepFilterNet3_onnx.tar.gz.
Original licenses are preserved in libDF/LICENSE, LICENSE-MIT, LICENSE-APACHE.

Local change in libDF/src/tract.rs: removed the RMS-based early return in
DfTract::process. Silent frames must advance the analysis/synthesis and model
states, especially when flushing end-of-file latency. Otherwise delayed samples
are lost and recurrent state freezes across quiet sections. The unused rms
binding is renamed _rms. No weights or model architecture were modified.

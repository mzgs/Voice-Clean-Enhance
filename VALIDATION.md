# Validation — 2026-09-20

Environment: Apple Silicon macOS, Rust 1.93.1, existing FFmpeg installation.

- Release build completed with the embedded DeepFilterNet3 model.
- Executable approximately 22.2 MB (21.2 MiB); dynamically linked only to macOS
  system libraries. FFmpeg is a separate executable dependency.
- Integration tests cover mono silence, non-frame-aligned short audio, final
  samples after delay flushing, overwrite protection, direct MP4 creation,
  unchanged video bitstream, and preservation of a delayed audio start.
- Source test: `/Volumes/SSD/final cut pro/IN/finished/sabrin yolu.mp4`, 01:00–02:00.
- Exactly 2,880,000 stereo frames at 48 kHz; all values finite; sample peak 0.490096.
- Estimated alignment lag against the original: 0 samples.
- Correlation against the earlier Python gentle output: 0.998823; RMS sample
  difference 0.002389. This is a waveform comparison, not a perceptual score.
- Native model uses continuous frame state; Python used independent chunks.
  Runtime thresholds and backend differences can also affect output. Bit-exact
  equivalence and subjective quality parity are not claimed.
- Eight-second source-derived video test: input/output copied video bitstreams
  have identical SHA-256 `000c82e1bbb2cab1484d5280988462eceed5992002c6cf136d69f2b1f572c416`.
- Encoded-audio cross-correlation on that remux: 144 samples of offset, matching
  the input's 3 ms audio/container offset. Audio timestamps are explicitly applied
  after filtering to avoid FFmpeg resetting the raw-audio input offset.
- One-minute sample processing/model initialization measured about 12.5 seconds
  during concurrent compilation. This is not a controlled performance benchmark.
- No listening-based comparison with Final Cut Pro has been performed.
- The full 80-minute source has not been processed.

Scratch results are in ignored `work/`. The original files were not modified.

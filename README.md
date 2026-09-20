# Voice Clean Enhance

Local neural speech cleanup in **Rust**, using DeepFilterNet3 and Tract. The model
is embedded in the executable. No Python, PyTorch, cloud service, or runtime model
download is required. FFmpeg and ffprobe must be available on PATH.

## Use

```sh
./clean-voice input.mp4 out.mp4
./clean-voice input.mp4 out.mp4 --profile balanced
./clean-voice input.mp4 out.wav --start 60 --duration 30
```

Run `./build.sh` first to create the compiled `clean-voice` executable in the
project root. For distribution, copy that executable; it does not need this
project directory or Cargo.
FFmpeg/ffprobe are separate dependencies.

Supported output: MP4, MOV, MKV, or 24-bit 48 kHz WAV. Video is copied without
re-encoding; audio is AAC at 256 kb/s. The first video/audio streams are used;
additional tracks, subtitles, and attachments are omitted. Metadata and chapters
are copied where supported. Use WAV for time-range previews. Mono/stereo only.
Existing output files and reports are never replaced; original media is never edited.

## Build

```sh
./build.sh
```

Tested with Rust 1.93.1 on Apple Silicon macOS. Keep Cargo.lock: it pins transitive
versions (including kstring 2.0.2, compatible with that compiler).

## Settings and quality

- `--profile gentle` (default): 12 dB attenuation limit.
- `--profile balanced`: 18 dB.
- `--profile strong`: 30 dB, with more risk of voice artifacts.
- `--attenuation 9`: override the profile (1–60 dB).
- `--start 60 --duration 30`: optional audio preview range.
- `--work-dir /path`: choose scratch storage (default: OS temporary directory).

Attenuation settings are model controls, not guaranteed measured noise reduction
or equivalents to Apple's sliders. No postfilter, EQ, compressor, or loudness
normalization is used. If needed, one constant gain reduction leaves 1 dB of
sample-peak headroom; it does not guarantee AAC true-peak headroom.
Stereo channels have independent model states. Their spatial image may change.
This is a speech model: music and other desired sounds may be removed. It cannot
guarantee isolation of one speaker from other voices or match Final Cut's quality.

## Implementation

FFmpeg decodes to 48 kHz interleaved float audio on disk. Rust processes small
frames with continuous recurrent state, appends zero frames to flush model delay,
and trims the output to the exact decoded sample count. Memory does not scale
with recording length. The output remux preserves the original audio start offset
relative to the input container. Container precision and AAC priming still apply.

Temporary files scale with duration: two float stereo streams use about 3.7 GB
for 80 minutes. Allow additional space for the final media. Output is staged in
the destination directory and published by a no-clobber hard link; that directory
must use a filesystem supporting hard links (e.g. APFS). Normal completion and
error returns clean up staging files. Forced termination may leave temporary files.
A JSON report is written beside output (`out.mp4.json`).

This project vendors DeepFilterNet v0.5.6 under its original MIT/Apache-2.0 license
and includes its pretrained model. See `vendor/deepfilter/UPSTREAM.md` and the
upstream license files. The vendored silence shortcut is patched so zero input
advances state and flushes delayed speech instead of discarding it.

## Validation

```sh
cargo test --release --locked
```

Integration tests require FFmpeg. They check short, non-frame-aligned audio,
trailing audio through delay flushing, silence, overwrite protection, and exact
video bitstream copying. See `VALIDATION.md` for source-sample checks. Technical
checks do not substitute for listening to voice timbre, breaths, and quiet words.

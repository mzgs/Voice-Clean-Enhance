use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
fn ff(args: &[&str]) -> Output {
    let o = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error"])
        .args(args)
        .output()
        .expect("FFmpeg required");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    o
}
fn cli(input: &Path, output: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_clean-voice"))
        .arg(input)
        .arg(output)
        .output()
        .unwrap()
}
fn cli_dereverb(input: &Path, output: &Path, level: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_clean-voice"))
        .arg(input)
        .arg(output)
        .args(["--dereverb", level])
        .output()
        .unwrap()
}

#[test]
fn dereverb_stereo_silence_length_and_report() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("silence.wav");
    ff(&[
        "-f",
        "lavfi",
        "-i",
        "anullsrc=r=48000:cl=stereo:d=0.12345",
        "-c:a",
        "pcm_f32le",
        input.to_str().unwrap(),
    ]);
    let before = ff(&["-i", input.to_str().unwrap(), "-f", "f32le", "-"]).stdout;
    for level in ["gentle", "balanced", "strong"] {
        let output = dir.path().join(format!("{level}.wav"));
        let result = cli_dereverb(&input, &output, level);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let after = ff(&["-i", output.to_str().unwrap(), "-f", "f32le", "-"]).stdout;
        assert_eq!(before, after, "Silence/length changed");
        let report: serde_json::Value =
            serde_json::from_slice(&fs::read(output.with_extension("wav.json")).unwrap()).unwrap();
        assert_eq!(report["dereverb"], level);
        assert_eq!(report["dereverb_method"], "experimental-online-wpe");
        let saved = fs::read(&output).unwrap();
        assert!(!cli_dereverb(&input, &output, level).status.success());
        assert_eq!(saved, fs::read(output).unwrap());
    }
    let output = dir.path().join("invalid.wav");
    assert!(!cli_dereverb(&input, &output, "invalid").status.success());
    assert!(!output.exists());
    let output = dir.path().join("bare.wav");
    let result = Command::new(env!("CARGO_BIN_EXE_clean-voice"))
        .arg(&input)
        .arg(&output)
        .arg("--dereverb")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(output.with_extension("wav.json")).unwrap()).unwrap();
    assert_eq!(report["dereverb"], "gentle");
}
#[test]
fn short_audio_tail_silence_and_no_clobber() {
    let dir = tempfile::tempdir().unwrap();
    for (name, filter) in [
        (
            "tone",
            "sine=frequency=440:sample_rate=48000:duration=0.12345",
        ),
        ("silent", "anullsrc=r=48000:cl=mono:d=0.12345"),
    ] {
        let input = dir.path().join(format!("{name}.wav"));
        let output = dir.path().join(format!("{name}-clean.wav"));
        ff(&[
            "-f",
            "lavfi",
            "-i",
            filter,
            "-c:a",
            "pcm_f32le",
            input.to_str().unwrap(),
        ]);
        let result = cli(&input, &output);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let report: serde_json::Value =
            serde_json::from_slice(&fs::read(output.with_extension("wav.json")).unwrap()).unwrap();
        assert_eq!(report["dereverb"], "off");
        assert!(report["dereverb_method"].is_null());
        let before = ff(&["-i", input.to_str().unwrap(), "-f", "f32le", "-"]).stdout;
        let after = ff(&["-i", output.to_str().unwrap(), "-f", "f32le", "-"]).stdout;
        assert_eq!(before.len(), after.len());
        let samples: Vec<f32> = after
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        assert!(samples.iter().all(|x| x.is_finite() && x.abs() < 1.0));
        if name == "tone" {
            assert!(
                samples[samples.len() - 480..]
                    .iter()
                    .any(|x| x.abs() > 0.001),
                "Lost final audio during latency flush"
            );
        } else {
            assert!(samples.iter().all(|x| x.abs() < 0.00001));
        }
        let saved = fs::read(&output).unwrap();
        assert!(!cli(&input, &output).status.success());
        assert_eq!(saved, fs::read(&output).unwrap());
    }
}
#[test]
fn direct_video_copies_picture() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input.mp4");
    let output = dir.path().join("output.mp4");
    ff(&[
        "-f",
        "lavfi",
        "-i",
        "color=size=64x64:rate=25:duration=0.4",
        "-f",
        "lavfi",
        "-i",
        "sine=sample_rate=48000:duration=0.4",
        "-c:v",
        "mpeg4",
        "-c:a",
        "aac",
        input.to_str().unwrap(),
    ]);
    let result = cli_dereverb(&input, &output, "gentle");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let hash = |path: &Path| {
        ff(&[
            "-i",
            path.to_str().unwrap(),
            "-map",
            "0:v:0",
            "-c",
            "copy",
            "-f",
            "hash",
            "-",
        ])
        .stdout
    };
    assert_eq!(hash(&input), hash(&output));
    assert!(!output.with_extension("wav").exists());
}

#[test]
fn remux_preserves_audio_start_offset() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("offset.mov");
    let output = dir.path().join("clean.mov");
    ff(&[
        "-f",
        "lavfi",
        "-i",
        "color=size=64x64:rate=25:duration=1",
        "-itsoffset",
        "0.2",
        "-f",
        "lavfi",
        "-i",
        "sine=sample_rate=48000:duration=0.6",
        "-c:v",
        "mpeg4",
        "-c:a",
        "pcm_s16le",
        input.to_str().unwrap(),
    ]);
    let result = cli_dereverb(&input, &output, "balanced");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=start_time",
            "-of",
            "json",
        ])
        .arg(output)
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    let start: f64 = v["streams"][0]["start_time"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    // AAC priming can move the first encoded packet by one 1024-sample frame.
    assert!(
        (start - 0.2).abs() < 0.025,
        "Audio start offset lost: {start}"
    );
}

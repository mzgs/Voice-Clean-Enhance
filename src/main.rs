use anyhow::{ensure, Context, Result};
use clap::{Parser, ValueEnum};
use df::tract::{DfParams, DfTract, RuntimeParams};
use ndarray::Array2;
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::{BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Profile {
    Gentle,
    Balanced,
    Strong,
}
#[derive(Parser, Debug)]
#[command(
    version,
    about = "Local DeepFilterNet3 voice cleaning. No Python required."
)]
struct Args {
    input: PathBuf,
    /// Output .mp4, .mov, .mkv, or .wav (never overwritten)
    output: PathBuf,
    #[arg(long, value_enum, default_value = "gentle")]
    profile: Profile,
    /// Suppression limit, 1–60 dB; overrides the profile
    #[arg(long)]
    attenuation: Option<f32>,
    /// Audio-only preview start, in seconds
    #[arg(long, default_value_t = 0.0)]
    start: f64,
    /// Audio-only preview duration, in seconds
    #[arg(long)]
    duration: Option<f64>,
    /// Temporary files directory (defaults to the OS temporary directory)
    #[arg(long)]
    work_dir: Option<PathBuf>,
}
fn run(cmd: &mut Command) -> Result<()> {
    let result = cmd
        .output()
        .context("Could not launch FFmpeg; install it and put it on PATH")?;
    ensure!(
        result.status.success(),
        "FFmpeg failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(())
}
fn ffmpeg() -> Command {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-n"]);
    cmd
}
fn value_time(v: &Value) -> f64 {
    v.as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0)
}
fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}
fn main() -> Result<()> {
    let args = Args::parse();
    let limit = args.attenuation.unwrap_or(match args.profile {
        Profile::Gentle => 12.0,
        Profile::Balanced => 18.0,
        Profile::Strong => 30.0,
    });
    ensure!(
        limit.is_finite() && (1.0..=60.0).contains(&limit),
        "Attenuation must be between 1 and 60 dB"
    );
    ensure!(
        args.start.is_finite() && args.start >= 0.0,
        "Invalid start time"
    );
    if let Some(d) = args.duration {
        ensure!(d.is_finite() && d > 0.0, "Duration must be positive");
    }
    ensure!(
        args.input.is_file(),
        "Input does not exist: {}",
        args.input.display()
    );
    let ext = args
        .output
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let video = matches!(ext.as_str(), "mp4" | "mov" | "mkv");
    ensure!(
        video || ext == "wav",
        "Output must be .mp4, .mov, .mkv, or .wav"
    );
    ensure!(
        !video || (args.start == 0.0 && args.duration.is_none()),
        "Use .wav output for time-range previews"
    );
    let report = args.output.with_extension(format!("{ext}.json"));
    for path in [&args.output, &report] {
        ensure!(
            !path.try_exists()?,
            "Refusing to overwrite {}",
            path.display()
        );
    }
    fs::create_dir_all(parent(&args.output))?;
    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(&args.input)
        .output()
        .context("Install ffprobe (included with FFmpeg)")?;
    ensure!(
        probe.status.success(),
        "ffprobe failed: {}",
        String::from_utf8_lossy(&probe.stderr)
    );
    let info: Value = serde_json::from_slice(&probe.stdout)?;
    let streams = info["streams"].as_array().context("No streams found")?;
    let audio = streams
        .iter()
        .find(|s| s["codec_type"] == "audio")
        .context("No audio stream")?;
    let channels = audio["channels"].as_u64().unwrap_or(0) as usize;
    ensure!(
        (1..=2).contains(&channels),
        "Only mono and stereo audio are supported"
    );
    ensure!(
        !video || streams.iter().any(|s| s["codec_type"] == "video"),
        "No video stream"
    );
    let temp = if let Some(dir) = &args.work_dir {
        fs::create_dir_all(dir)?;
        tempfile::tempdir_in(dir)?
    } else {
        tempfile::tempdir()?
    };
    let raw = temp.path().join("input.f32");
    let cleaned = temp.path().join("clean.f32");
    let mut decode = ffmpeg();
    if args.start > 0.0 {
        decode.args(["-ss", &args.start.to_string()]);
    }
    decode.arg("-i").arg(&args.input);
    if let Some(d) = args.duration {
        decode.args(["-t", &d.to_string()]);
    }
    let mut filter = "aresample=48000".to_string();
    if let Some(d) = args.duration {
        filter.push_str(&format!(
            ",atrim=end_sample={}",
            (d * 48000.0).round() as u64
        ));
    }
    filter.push_str(",asetpts=PTS-STARTPTS");
    decode
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-af",
            &filter,
            "-ar",
            "48000",
            "-c:a",
            "pcm_f32le",
            "-f",
            "f32le",
        ])
        .arg(&raw);
    eprintln!("Decoding audio…");
    run(&mut decode)?;
    let frames = fs::metadata(&raw)?.len() as usize / (channels * 4);
    ensure!(frames > 0, "Selected range contains no audio");
    let clock = Instant::now();
    eprintln!("Loading embedded DeepFilterNet3 model…");
    // Separate recurrent state per channel, matching the Python baseline's independent channels.
    let params = RuntimeParams::default()
        .with_atten_lim(limit)
        .with_thresholds(-15.0, 35.0, 35.0);
    let mut models = (0..channels)
        .map(|_| DfTract::new(DfParams::default(), &params))
        .collect::<Result<Vec<_>>>()?;
    let hop = models[0].hop_size;
    let delay = models[0].fft_size - hop + models[0].lookahead * hop;
    ensure!(models[0].sr == 48000, "Unexpected model sample rate");
    let mut reader = BufReader::new(File::open(&raw)?);
    let mut writer = BufWriter::new(File::create(&cleaned)?);
    let mut input = Array2::<f32>::zeros((1, hop));
    let mut out = Array2::<f32>::zeros((1, hop));
    let mut bytes = vec![0u8; hop * channels * 4];
    let mut interleaved = vec![0f32; hop * channels];
    let mut peak = 0f32;
    let mut written = 0usize;
    // Flush delayed samples with zero input, then trim to the exact decoded length.
    let blocks = (frames + delay).div_ceil(hop);
    for block in 0..blocks {
        let position = block * hop;
        let count = frames.saturating_sub(position).min(hop);
        bytes.fill(0);
        reader.read_exact(&mut bytes[..count * channels * 4])?;
        for (ch, model) in models.iter_mut().enumerate() {
            for j in 0..hop {
                let i = (j * channels + ch) * 4;
                input[[0, j]] = f32::from_le_bytes(bytes[i..i + 4].try_into()?);
            }
            model.process(input.view(), out.view_mut())?;
            for j in 0..hop {
                interleaved[j * channels + ch] = out[[0, j]];
            }
        }
        for j in 0..hop {
            let t = position + j;
            if t < delay || t - delay >= frames {
                continue;
            }
            for ch in 0..channels {
                let sample = interleaved[j * channels + ch];
                ensure!(sample.is_finite(), "Model produced nonfinite audio");
                peak = peak.max(sample.abs());
                writer.write_all(&sample.to_le_bytes())?;
            }
            written += 1;
        }
        if block % 500 == 0 {
            eprint!(
                "\rCleaning {:.1}/{:.1} seconds",
                position.min(frames) as f64 / 48000.0,
                frames as f64 / 48000.0
            );
        }
    }
    writer.flush()?;
    ensure!(written == frames, "Internal sample-count mismatch");
    let gain = (10f32.powf(-1.0 / 20.0) / peak.max(1e-12)).min(1.0);
    eprintln!("\nWriting {}…", args.output.display());
    // Stage in the output directory and publish without clobbering existing files.
    let stage_dir = tempfile::tempdir_in(parent(&args.output))?;
    let staged = stage_dir.path().join(format!("result.{ext}"));
    let mut encode = ffmpeg();
    if video {
        encode.arg("-i").arg(&args.input);
    }
    encode
        .args([
            "-f",
            "f32le",
            "-ar",
            "48000",
            "-ac",
            &channels.to_string(),
            "-i",
        ])
        .arg(&cleaned);
    if video {
        encode.args([
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-map_metadata",
            "0",
            "-map_chapters",
            "0",
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-b:a",
            "256k",
        ]);
    } else {
        encode.args(["-c:a", "pcm_s24le"]);
    }
    let mut output_filter = format!("volume={gain}");
    if video {
        let offset = value_time(&audio["start_time"]) - value_time(&info["format"]["start_time"]);
        output_filter.push_str(&format!(",asetpts=PTS-STARTPTS+({offset})/TB"));
    }
    encode.args(["-af", &output_filter]);
    if ext == "wav" {
        encode.args(["-rf64", "auto"]);
    }
    encode.arg(&staged);
    run(&mut encode)?;
    fs::hard_link(&staged, &args.output)
        .context("Could not publish output (an existing file is never replaced)")?;
    let data = json!({"model":"DeepFilterNet3","engine":"DeepFilterNet Rust / Tract","input":args.input,"output":args.output,"attenuation_db":limit,"sample_rate":48000,"channels":channels,"frames":frames,"delay_compensated_samples":delay,"gain_db":20.0*gain.log10(),"elapsed_seconds":clock.elapsed().as_secs_f64(),"python_required":false});
    let mut report_file = File::options().write(true).create_new(true).open(&report)?;
    serde_json::to_writer_pretty(&mut report_file, &data)?;
    eprintln!("Saved {}", args.output.display());
    Ok(())
}

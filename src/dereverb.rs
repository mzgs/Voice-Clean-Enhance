//! Streaming, single-channel weighted prediction error (WPE) dereverberation.
//! Each channel learns a delayed spectral predictor with weighted RLS. This is
//! an experimental room-reverb reducer, not reference-based echo cancellation.
use anyhow::{ensure, Result};
use df::{Complex32 as C, DFState};
use std::{
    fs::File,
    io::{BufReader, BufWriter, Read, Write},
    path::Path,
};

const HOP: usize = 480;
const FFT: usize = 1920;
const DELAY: usize = 4; // 40 ms: prediction windows do not overlap the current window.
const TAPS: usize = 12;
const HISTORY: usize = DELAY + TAPS;
const ALPHA: f32 = 0.995;

struct Predictor {
    weights: [C; TAPS],
    inverse: [[C; TAPS]; TAPS],
    power: f32,
}

impl Predictor {
    fn new() -> Self {
        let mut inverse = [[C::default(); TAPS]; TAPS];
        for (i, row) in inverse.iter_mut().enumerate() {
            row[i].re = 1.0;
        }
        Self {
            weights: [C::default(); TAPS],
            inverse,
            power: 1e-8,
        }
    }

    fn process(&mut self, current: C, past: &[C; TAPS], amount: f32) -> C {
        let prediction: C = self
            .weights
            .iter()
            .zip(past)
            .map(|(w, x)| w.conj() * x)
            .sum();
        let residual = current - prediction;
        self.power = 0.9 * self.power + 0.1 * residual.norm_sqr();
        // Freeze during silence: forgetting with no observations makes the
        // inverse covariance grow indefinitely and destabilizes the next onset.
        if current.norm_sqr() + past.iter().map(|x| x.norm_sqr()).sum::<f32>() > 1e-12 {
            let mut px = [C::default(); TAPS];
            for (value, row) in px.iter_mut().zip(&self.inverse) {
                *value = row.iter().zip(past).map(|(p, x)| p * x).sum();
            }
            let denominator = ALPHA * self.power.max(1e-10)
                + past
                    .iter()
                    .zip(px)
                    .map(|(x, p)| (x.conj() * p).re)
                    .sum::<f32>()
                    .max(0.0);
            for i in 0..TAPS {
                let gain = px[i] / denominator;
                self.weights[i] += gain * residual.conj();
                for j in i..TAPS {
                    let mut value = (self.inverse[i][j] - gain * px[j].conj()) / ALPHA;
                    if i == j {
                        value.im = 0.0;
                    }
                    self.inverse[i][j] = value;
                    self.inverse[j][i] = value.conj();
                }
            }
            if self
                .weights
                .iter()
                .any(|w| !w.re.is_finite() || !w.im.is_finite())
                || (0..TAPS).any(|i| {
                    !self.inverse[i][i].re.is_finite()
                        || self.inverse[i][i].re <= 0.0
                        || self.inverse[i][i].re > 1e6
                })
            {
                *self = Self::new();
                return current;
            }
        }
        // Limit subtraction and never amplify a bin. A mismatched predictor
        // should not produce loud bursts or invent a tail after silence.
        let magnitude = current.norm();
        let prediction = prediction * (magnitude / prediction.norm().max(1e-20)).min(1.0);
        let output = current - amount * prediction;
        output * (magnitude / output.norm().max(1e-20)).min(1.0)
    }
}

struct Dereverb {
    stft: DFState,
    spectrum: Vec<C>,
    history: Vec<Vec<C>>,
    predictors: Vec<Predictor>,
    position: usize,
    amount: f32,
}

impl Dereverb {
    fn new(amount: f32) -> Self {
        let bins = FFT / 2 + 1;
        Self {
            stft: DFState::new(48000, FFT, HOP, 32, 2),
            spectrum: vec![C::default(); bins],
            history: vec![vec![C::default(); bins]; HISTORY],
            predictors: (0..bins).map(|_| Predictor::new()).collect(),
            position: 0,
            amount,
        }
    }

    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        self.stft.analysis(input, &mut self.spectrum);
        self.history[self.position].copy_from_slice(&self.spectrum);
        for (bin, value) in self.spectrum.iter_mut().enumerate() {
            let past = std::array::from_fn(|tap| {
                self.history[(self.position + HISTORY - DELAY - tap) % HISTORY][bin]
            });
            *value = self.predictors[bin].process(*value, &past, self.amount);
        }
        self.position = (self.position + 1) % HISTORY;
        self.stft.synthesis(&mut self.spectrum, output);
    }
}

/// Process decoded 48 kHz interleaved float PCM with bounded memory. Flush the
/// overlap-add delay and trim it so the output retains exact length and timing.
pub fn process_file(input: &Path, output: &Path, channels: usize, amount: f32) -> Result<()> {
    let file = File::open(input)?;
    let bytes_len = file.metadata()?.len() as usize;
    ensure!(
        (1..=2).contains(&channels),
        "Dereverb requires mono or stereo"
    );
    ensure!(bytes_len % (channels * 4) == 0, "Incomplete PCM frame");
    let frames = bytes_len / (channels * 4);
    let mut reader = BufReader::new(file);
    let mut writer = BufWriter::new(File::create(output)?);
    let mut states: Vec<_> = (0..channels).map(|_| Dereverb::new(amount)).collect();
    let delay = FFT - HOP;
    let mut bytes = vec![0; HOP * channels * 4];
    let mut input_frame = [0.0; HOP];
    let mut output_frame = [0.0; HOP];
    let mut interleaved = vec![0.0; HOP * channels];
    for block in 0..(frames + delay).div_ceil(HOP) {
        let position = block * HOP;
        let count = frames.saturating_sub(position).min(HOP);
        bytes.fill(0);
        reader.read_exact(&mut bytes[..count * channels * 4])?;
        for (ch, state) in states.iter_mut().enumerate() {
            for (j, x) in input_frame.iter_mut().enumerate() {
                let i = (j * channels + ch) * 4;
                *x = f32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
                ensure!(x.is_finite(), "Nonfinite input to dereverb");
            }
            state.process(&input_frame, &mut output_frame);
            for (j, &sample) in output_frame.iter().enumerate() {
                ensure!(sample.is_finite(), "Dereverb produced nonfinite audio");
                interleaved[j * channels + ch] = sample;
            }
        }
        let start = delay.saturating_sub(position).min(HOP);
        let end = (frames + delay).saturating_sub(position).min(HOP);
        for sample in &interleaved[start * channels..end * channels] {
            writer.write_all(&sample.to_le_bytes())?;
        }
        if block % 1000 == 0 {
            eprint!(
                "\rReducing room reverb {:.1}/{:.1} seconds",
                position.min(frames) as f64 / 48000.0,
                frames as f64 / 48000.0
            );
        }
    }
    writer.flush()?;
    eprintln!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(samples: &[f32], channels: usize, amount: f32) -> Vec<f32> {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.f32");
        let output = dir.path().join("out.f32");
        std::fs::write(
            &input,
            samples
                .iter()
                .flat_map(|x| x.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        process_file(&input, &output, channels, amount).unwrap();
        std::fs::read(output)
            .unwrap()
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect()
    }

    fn noise(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (*seed >> 8) as f32 / 8388608.0 - 1.0
    }

    #[test]
    fn reconstruction_preserves_alignment_edges_and_channels() {
        for frames in [1, 479, 480, 481, 5773] {
            let mut seed = 42;
            let samples: Vec<_> = (0..frames * 2).map(|_| noise(&mut seed) * 0.2).collect();
            let output = process(&samples, 2, 0.0);
            assert_eq!(output.len(), samples.len());
            let max_error = output
                .iter()
                .zip(&samples)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            assert!(max_error < 1e-6, "STFT reconstruction error: {max_error}");
        }
        // Separate impulses beyond the prediction history; a repeated impulse
        // inside that history is itself indistinguishable from a reflection.
        let mut impulse = vec![0.0; 10001 * 2];
        impulse[0] = 0.5;
        impulse[10000 * 2] = 0.5;
        let output = process(&impulse, 2, 0.85);
        assert_eq!(output.len(), impulse.len());
        assert!(
            output[0] > 0.45 && output[10000 * 2] > 0.45,
            "Damaged boundary impulses: {}, {}",
            output[0],
            output[10000 * 2]
        );
        assert!(
            output.iter().skip(1).step_by(2).all(|&x| x == 0.0),
            "Channel crosstalk"
        );
    }

    #[test]
    fn synthetic_room_tail_is_reduced() {
        // Repeated broadband bursts through a fixed decaying reflection path.
        // Independent innovations prevent success by simply predicting a tone.
        let mut seed = 728;
        let mut dry = vec![0.0; 48000 * 10];
        let mut reverberant = dry.clone();
        for i in 0..dry.len() {
            if i % 48000 < 14400 {
                dry[i] = 0.15 * noise(&mut seed);
            }
            reverberant[i] = dry[i];
            for (delay, gain) in [(3840, 0.55), (6240, 0.25)] {
                if i >= delay {
                    reverberant[i] += gain * reverberant[i - delay];
                }
            }
        }
        let output = process(&reverberant, 1, 0.6);
        assert_eq!(output.len(), dry.len());
        assert!(output.iter().all(|x| x.is_finite()));
        let mut before = 0.0;
        let mut after = 0.0;
        let mut onset_before = 0.0;
        let mut onset_after = 0.0;
        for i in 48000 * 5..dry.len() {
            let phase = i % 48000;
            if (16800..33600).contains(&phase) {
                before += reverberant[i].powi(2);
                after += output[i].powi(2);
            }
            if phase < 960 {
                onset_before += reverberant[i].powi(2);
                onset_after += output[i].powi(2);
            }
        }
        eprintln!(
            "Tail energy ratio: {}; onset ratio: {}",
            after / before,
            onset_after / onset_before
        );
        assert!(
            after / before < 0.8,
            "No meaningful reduction of the synthetic reverb tail"
        );
        assert!(
            onset_after / onset_before > 0.8,
            "Damaged direct sound onset"
        );
    }

    #[test]
    fn predictor_survives_long_silence_and_dry_signal() {
        let mut predictor = Predictor::new();
        let mut history = [C::default(); HISTORY];
        let mut seed = 67;
        let mut input_energy = 0.0;
        let mut output_energy = 0.0;
        for t in 0..100_000 {
            let current = if (1000..90_000).contains(&t) {
                C::default()
            } else {
                C::new(noise(&mut seed), noise(&mut seed)) * 0.01
            };
            let past = std::array::from_fn(|i| history[(t + HISTORY - DELAY - i) % HISTORY]);
            let output = predictor.process(current, &past, 0.85);
            history[t % HISTORY] = current;
            assert!(output.re.is_finite() && output.im.is_finite());
            assert!(output.norm() <= current.norm() + 1e-7);
            if t > 91_000 {
                input_energy += current.norm_sqr();
                output_energy += output.norm_sqr();
            }
        }
        assert!(
            output_energy / input_energy > 0.8,
            "Excessive suppression of uncorrelated dry input"
        );
    }
}

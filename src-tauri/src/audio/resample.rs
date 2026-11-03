//! Streaming sample-rate conversion to the engines' 16 kHz input rate.
//!
//! Runs on the audio pump task (never the CoreAudio real-time thread), so it
//! may allocate. The design is deliberately simple and dependency-free:
//!
//! 1. **Anti-alias low-pass** (downsampling only): a Blackman-windowed sinc
//!    FIR at the *input* rate with its cutoff just below the output Nyquist
//!    (`0.435 × out_rate`, ≈ 7 kHz for 16 kHz output). The tap count scales
//!    with the input rate so the transition band stays ~2 kHz wide.
//! 2. **Linear interpolation** at fractional input positions spaced
//!    `in_rate / out_rate` apart. Position is tracked in `f64` so there is no
//!    cumulative drift over long sessions.
//!
//! All state (FIR history, the last filtered sample, the fractional read
//! position) carries across calls, so feeding a signal in arbitrary chunk
//! sizes produces exactly the same output as feeding it in one go.

/// Width of the FIR transition band in Hz. Narrower = more taps.
const TRANSITION_HZ: f64 = 2_000.0;
/// Upper bound on FIR length so exotic 192 kHz devices stay cheap.
const MAX_TAPS: usize = 511;
/// Cutoff as a fraction of the *output* sample rate (Nyquist is 0.5).
const CUTOFF_FRACTION: f64 = 0.435;

/// Stateful mono resampler. Create one per capture session.
#[derive(Debug, Clone)]
pub struct Resampler {
    in_rate: u32,
    out_rate: u32,
    /// Input samples advanced per output sample.
    step: f64,
    /// FIR taps (empty when no low-pass is needed, i.e. upsampling/equal).
    taps: Vec<f32>,
    /// Last `taps.len() - 1` raw input samples (FIR delay line).
    history: Vec<f32>,
    /// Filtered samples not yet fully consumed by the interpolator.
    /// `pending[0]` is the sample at integer position 0 relative to `pos`.
    pending: Vec<f32>,
    /// Fractional read position into `pending`.
    pos: f64,
}

impl Resampler {
    /// Build a resampler from `in_rate` to `out_rate` (both in Hz, > 0).
    /// A zero rate is clamped to 1 Hz rather than panicking.
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        let in_rate = in_rate.max(1);
        let out_rate = out_rate.max(1);
        let taps = if in_rate > out_rate {
            design_lowpass(in_rate, out_rate)
        } else {
            Vec::new()
        };
        let history = vec![0.0; taps.len().saturating_sub(1)];
        Self {
            in_rate,
            out_rate,
            step: f64::from(in_rate) / f64::from(out_rate),
            taps,
            history,
            pending: Vec::new(),
            pos: 0.0,
        }
    }

    pub fn in_rate(&self) -> u32 {
        self.in_rate
    }

    pub fn out_rate(&self) -> u32 {
        self.out_rate
    }

    /// Resample `input`, appending the produced samples to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }
        if self.in_rate == self.out_rate {
            out.extend_from_slice(input);
            return;
        }

        // 1. Low-pass (or pass through) into `pending`.
        if self.taps.is_empty() {
            self.pending.extend_from_slice(input);
        } else {
            let hist_len = self.history.len();
            let mut line = Vec::with_capacity(hist_len + input.len());
            line.extend_from_slice(&self.history);
            line.extend_from_slice(input);
            self.pending.reserve(input.len());
            for window in line.windows(self.taps.len()) {
                // taps are symmetric, so convolution == correlation here.
                let y: f32 = window
                    .iter()
                    .zip(self.taps.iter())
                    .map(|(x, h)| x * h)
                    .sum();
                self.pending.push(y);
            }
            let keep_from = line.len() - hist_len;
            self.history.clear();
            self.history.extend_from_slice(&line[keep_from..]);
        }

        // 2. Linear interpolation at fractional positions.
        out.reserve((input.len() as f64 / self.step) as usize + 2);
        loop {
            let idx = self.pos.floor() as usize;
            if idx + 1 >= self.pending.len() {
                break;
            }
            let frac = (self.pos - idx as f64) as f32;
            let a = self.pending[idx];
            let b = self.pending[idx + 1];
            out.push(a + (b - a) * frac);
            self.pos += self.step;
        }

        // 3. Drop fully-consumed samples, keeping the one the next
        //    interpolation starts from.
        let consumed = (self.pos.floor() as usize).min(self.pending.len());
        if consumed > 0 {
            self.pending.drain(..consumed);
            self.pos -= consumed as f64;
        }
    }
}

/// Blackman-windowed sinc low-pass, normalised to unity DC gain.
fn design_lowpass(in_rate: u32, out_rate: u32) -> Vec<f32> {
    let fs = f64::from(in_rate);
    let mut n = (5.5 * fs / TRANSITION_HZ).ceil() as usize;
    n = n.clamp(15, MAX_TAPS);
    if n % 2 == 0 {
        n += 1;
    }
    let fc = CUTOFF_FRACTION * f64::from(out_rate) / fs; // cycles per sample
    let m = (n - 1) as f64 / 2.0;
    let mut taps: Vec<f64> = (0..n)
        .map(|i| {
            let x = i as f64 - m;
            let sinc = if x == 0.0 {
                2.0 * fc
            } else {
                (2.0 * std::f64::consts::PI * fc * x).sin() / (std::f64::consts::PI * x)
            };
            let w = 0.42 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos()
                + 0.08 * (4.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos();
            sinc * w
        })
        .collect();
    let sum: f64 = taps.iter().sum();
    if sum.abs() > f64::EPSILON {
        for t in &mut taps {
            *t /= sum;
        }
    }
    taps.into_iter().map(|t| t as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, rate: u32, secs: f64, amp: f32) -> Vec<f32> {
        let n = (f64::from(rate) * secs) as usize;
        (0..n)
            .map(|i| {
                amp * (2.0 * std::f64::consts::PI * freq * i as f64 / f64::from(rate)).sin() as f32
            })
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        if x.is_empty() {
            return 0.0;
        }
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
    }

    /// Frequency estimate from positive-going zero crossings.
    fn est_freq(x: &[f32], rate: u32) -> f64 {
        let crossings = x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        crossings as f64 * f64::from(rate) / x.len() as f64
    }

    fn run(r: &mut Resampler, input: &[f32]) -> Vec<f32> {
        let mut out = Vec::new();
        r.process(input, &mut out);
        out
    }

    #[test]
    fn equal_rates_pass_through() {
        let input = sine(440.0, 16_000, 0.1, 0.5);
        let mut r = Resampler::new(16_000, 16_000);
        assert_eq!(run(&mut r, &input), input);
    }

    #[test]
    fn output_length_matches_ratio() {
        for &rate in &[8_000_u32, 22_050, 24_000, 44_100, 48_000, 96_000] {
            let input = vec![0.1_f32; rate as usize]; // 1 s
            let mut r = Resampler::new(rate, 16_000);
            let out = run(&mut r, &input);
            let diff = (out.len() as i64 - 16_000).abs();
            assert!(diff <= 3, "rate {rate}: got {} samples", out.len());
        }
    }

    #[test]
    fn sine_survives_downsampling() {
        for &rate in &[44_100_u32, 48_000, 24_000] {
            let input = sine(1_000.0, rate, 1.0, 0.5);
            let mut r = Resampler::new(rate, 16_000);
            let out = run(&mut r, &input);
            // Skip the FIR warm-up.
            let body = &out[400..];
            let f = est_freq(body, 16_000);
            assert!((f - 1_000.0).abs() < 10.0, "rate {rate}: freq {f}");
            let expected = 0.5 / 2.0_f32.sqrt();
            let got = rms(body);
            assert!(
                (got - expected).abs() < 0.03,
                "rate {rate}: rms {got} vs {expected}"
            );
        }
    }

    #[test]
    fn sine_survives_upsampling() {
        let input = sine(500.0, 8_000, 1.0, 0.5);
        let mut r = Resampler::new(8_000, 16_000);
        let out = run(&mut r, &input);
        let f = est_freq(&out[100..], 16_000);
        assert!((f - 500.0).abs() < 10.0, "freq {f}");
    }

    #[test]
    fn content_above_output_nyquist_is_attenuated() {
        // 12 kHz would alias to 4 kHz at 16 kHz output without filtering.
        let input = sine(12_000.0, 48_000, 1.0, 0.5);
        let mut r = Resampler::new(48_000, 16_000);
        let out = run(&mut r, &input);
        let got = rms(&out[400..]);
        assert!(got < 0.01, "alias rms {got}");
    }

    #[test]
    fn chunked_processing_is_continuous() {
        for &rate in &[44_100_u32, 48_000, 8_000] {
            let input = sine(700.0, rate, 0.5, 0.4);
            let mut whole = Resampler::new(rate, 16_000);
            let expected = run(&mut whole, &input);

            let mut chunked = Resampler::new(rate, 16_000);
            let mut got = Vec::new();
            // Irregular chunk sizes, like real CoreAudio callbacks.
            let sizes = [1_usize, 7, 128, 441, 512, 1_000, 3];
            let mut i = 0;
            let mut k = 0;
            while i < input.len() {
                let n = sizes[k % sizes.len()].min(input.len() - i);
                chunked.process(&input[i..i + n], &mut got);
                i += n;
                k += 1;
            }
            assert_eq!(got.len(), expected.len(), "rate {rate}");
            for (a, b) in got.iter().zip(expected.iter()) {
                assert!((a - b).abs() < 1e-5, "rate {rate}: {a} vs {b}");
            }
        }
    }

    #[test]
    fn zero_rate_does_not_panic() {
        let mut r = Resampler::new(0, 16_000);
        let _ = run(&mut r, &[0.0; 10]);
    }
}

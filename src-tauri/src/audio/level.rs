//! Audio-level utilities.
//!
//! The pill's waveform is driven entirely by RMS amplitude — the engines do
//! their own VAD/feature extraction on the raw f32 samples. RMS is a cheap,
//! O(n) per buffer computation that we run inside the audio callback (no
//! allocations) before publishing it through a lock-free [`LevelMeter`].

use std::sync::atomic::{AtomicU32, Ordering};

/// Lock-free peak-hold level cell shared between the CoreAudio callback and
/// the 30 Hz level/VAD poller.
///
/// The callback calls [`LevelMeter::record`] once per buffer; the poller calls
/// [`LevelMeter::take`] once per tick, which returns the *maximum* level seen
/// since the previous tick and resets it to zero. That keeps syllable peaks
/// that land between ticks instead of only reporting the latest buffer.
///
/// Implementation note: for non-negative, non-NaN `f32`s the IEEE-754 bit
/// pattern orders the same way as the value, so `AtomicU32::fetch_max` on the
/// raw bits is a correct float max — no CAS loop, no lock, RT-safe.
#[derive(Debug, Default)]
pub struct LevelMeter(AtomicU32);

impl LevelMeter {
    pub fn new() -> Self {
        Self(AtomicU32::new(0))
    }

    /// Record a level (RT-safe). Negative / NaN / infinite values are ignored.
    pub fn record(&self, level: f32) {
        if level.is_finite() && level > 0.0 {
            self.0.fetch_max(level.to_bits(), Ordering::Relaxed);
        }
    }

    /// Return the peak recorded since the previous `take` and reset to 0.
    pub fn take(&self) -> f32 {
        f32::from_bits(self.0.swap(0, Ordering::Relaxed))
    }
}

/// Compute the root-mean-square amplitude of a slice of f32 samples.
///
/// Returns `0.0` for an empty slice rather than `NaN` so consumers never have
/// to special-case the empty case.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::{rms, LevelMeter};

    #[test]
    fn meter_holds_peak_until_taken() {
        let m = LevelMeter::new();
        m.record(0.2);
        m.record(0.5);
        m.record(0.3);
        assert_eq!(m.take(), 0.5);
        assert_eq!(m.take(), 0.0);
    }

    #[test]
    fn meter_ignores_invalid_values() {
        let m = LevelMeter::new();
        m.record(f32::NAN);
        m.record(-1.0);
        m.record(f32::INFINITY);
        assert_eq!(m.take(), 0.0);
        m.record(0.01);
        assert!((m.take() - 0.01).abs() < f32::EPSILON);
    }

    #[test]
    fn rms_empty_is_zero() {
        assert_eq!(rms(&[]), 0.0);
    }

    #[test]
    fn rms_constant_signal() {
        // A constant 0.5 amplitude signal has RMS = 0.5.
        let samples = vec![0.5_f32; 1024];
        let r = rms(&samples);
        assert!((r - 0.5).abs() < 1e-6, "expected ~0.5, got {r}");
    }

    #[test]
    fn rms_pure_sine_half_amplitude() {
        // A sine wave with amplitude 0.5 has RMS = 0.5 / sqrt(2) ≈ 0.3536.
        let samples: Vec<f32> = (0..2048)
            .map(|n| 0.5 * (2.0 * std::f32::consts::PI * n as f32 / 64.0).sin())
            .collect();
        let r = rms(&samples);
        let expected = 0.5_f32 / 2.0_f32.sqrt();
        assert!((r - expected).abs() < 1e-2, "expected ~{expected}, got {r}");
    }

    #[test]
    fn rms_silence_is_zero() {
        let samples = vec![0.0_f32; 512];
        assert_eq!(rms(&samples), 0.0);
    }
}

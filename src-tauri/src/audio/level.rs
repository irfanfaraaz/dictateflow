//! Audio-level utilities.
//!
//! The pill's waveform is driven entirely by RMS amplitude — the engines do
//! their own VAD/feature extraction on the raw f32 samples. RMS is a cheap,
//! O(n) per buffer computation that we run inside the audio callback (no
//! allocations) before publishing on a `tokio::sync::watch` channel.

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
    use super::rms;

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

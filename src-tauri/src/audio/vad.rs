//! Voice-activity detection (VAD).
//!
//! The pill's waveform drives the level display; this module uses the same RMS
//! samples to detect sustained silence and trigger early-stop of dictation.
//!
//! # Calibration note
//!
//! `VadConfig::rms_threshold` (default 0.005) is tuned for the existing audio
//! pipeline — post-decimation, post-channel-average 16-kHz mono f32 as
//! produced by `audio::capture`. If the capture pipeline changes its gain or
//! sample format, re-tune this threshold.

use std::time::{Duration, Instant};

// ---------- Public types ----------

/// Configuration for the silence detector.
///
/// Sane defaults: threshold is comfortably below normal speech but above
/// microphone noise floor on typical macOS hardware; silence window gives
/// the speaker a natural pause (800 ms) before we call it silence.
#[derive(Debug, Clone)]
pub struct VadConfig {
    /// RMS amplitude below which a sample is considered silence.
    pub rms_threshold: f32,
    /// How long (ms) silence must hold continuously before `SustainedSilence`
    /// fires. After it fires once it will not fire again until speech resumes
    /// and then a new silence run begins.
    pub silence_window_ms: u32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            rms_threshold: 0.005,
            silence_window_ms: 800,
        }
    }
}

/// Events returned by [`SilenceDetector::observe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SilenceEvent {
    /// RMS is above threshold — the user is speaking.
    Speaking,
    /// RMS is below threshold and silence hasn't held long enough yet.
    Silent,
    /// Silence has held continuously past `silence_window_ms`. Fires exactly
    /// once per silence run; subsequent calls return [`SilenceEvent::Silent`]
    /// until speech resumes and another silence run completes.
    SustainedSilence,
}

/// Stateful observer: feed RMS samples + wall-clock timestamps, receive
/// [`SilenceEvent`]s.
///
/// Create one per dictation session and drop it when the session ends.
pub struct SilenceDetector {
    config: VadConfig,
    /// When the current silence run started. `None` if the speaker is active.
    silence_started_at: Option<Instant>,
    /// True once `SustainedSilence` has been emitted for the current run, so
    /// we don't keep firing it on every subsequent silent sample.
    sustained_fired: bool,
}

impl SilenceDetector {
    pub fn new(config: VadConfig) -> Self {
        Self {
            config,
            silence_started_at: None,
            sustained_fired: false,
        }
    }

    /// Feed one RMS sample at wall-clock time `now`.
    ///
    /// Call this at the same cadence as the `watch::Receiver<f32>` emits
    /// (the level pump delivers values whenever cpal ticks, typically every
    /// ~10-20 ms on Apple Silicon hardware).
    pub fn observe(&mut self, rms: f32, now: Instant) -> SilenceEvent {
        if rms >= self.config.rms_threshold {
            // Speech detected — reset silence tracking unconditionally.
            self.silence_started_at = None;
            self.sustained_fired = false;
            return SilenceEvent::Speaking;
        }

        // Below threshold — silence.
        let started = self.silence_started_at.get_or_insert(now);
        let elapsed = now.duration_since(*started);
        let window = Duration::from_millis(self.config.silence_window_ms as u64);

        if elapsed >= window && !self.sustained_fired {
            self.sustained_fired = true;
            SilenceEvent::SustainedSilence
        } else {
            SilenceEvent::Silent
        }
    }
}

// ---------- Tests ----------

#[cfg(test)]
mod tests {
    use super::*;

    fn detector() -> SilenceDetector {
        SilenceDetector::new(VadConfig {
            rms_threshold: 0.005,
            silence_window_ms: 800,
        })
    }

    /// Helper: advance `now` by `ms` milliseconds.
    fn add_ms(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    // ------------------------------------------------------------------
    // Pure silence: first sample is Silent, escalates after >800 ms.
    // ------------------------------------------------------------------
    #[test]
    fn pure_silence_escalates_after_window() {
        let mut d = detector();
        let t0 = Instant::now();

        // First silent sample.
        assert_eq!(d.observe(0.0, t0), SilenceEvent::Silent);

        // 400 ms in — still within window.
        assert_eq!(d.observe(0.0, add_ms(t0, 400)), SilenceEvent::Silent);

        // Just past 800 ms — first time: SustainedSilence.
        assert_eq!(
            d.observe(0.0, add_ms(t0, 801)),
            SilenceEvent::SustainedSilence
        );

        // Subsequent silent samples after sustained fires → Silent (not repeated).
        assert_eq!(d.observe(0.0, add_ms(t0, 900)), SilenceEvent::Silent);
        assert_eq!(d.observe(0.0, add_ms(t0, 1_500)), SilenceEvent::Silent);
    }

    // ------------------------------------------------------------------
    // Speaking input never escalates.
    // ------------------------------------------------------------------
    #[test]
    fn speaking_never_escalates() {
        let mut d = detector();
        let t0 = Instant::now();

        for i in 0..50 {
            let ev = d.observe(0.1, add_ms(t0, i * 20));
            assert_eq!(ev, SilenceEvent::Speaking, "expected Speaking at tick {i}");
        }
    }

    // ------------------------------------------------------------------
    // Speaking → silence → speaking resets; no SustainedSilence.
    // ------------------------------------------------------------------
    #[test]
    fn speech_interrupts_silence_run() {
        let mut d = detector();
        let t0 = Instant::now();

        // Silence for 500 ms (below window).
        assert_eq!(d.observe(0.0, t0), SilenceEvent::Silent);
        assert_eq!(d.observe(0.0, add_ms(t0, 500)), SilenceEvent::Silent);

        // Speech resumes — resets the run.
        assert_eq!(d.observe(0.1, add_ms(t0, 600)), SilenceEvent::Speaking);

        // Silence run restarts from the first silent sample after speech.
        // That sample is at t=700ms; need 700 + 800 = 1500ms to fire.
        assert_eq!(d.observe(0.0, add_ms(t0, 700)), SilenceEvent::Silent);
        // Only 100 ms of new silence so far — must not fire.
        assert_eq!(d.observe(0.0, add_ms(t0, 800)), SilenceEvent::Silent);
        // 400 ms of new silence (since t=700) — still not fired.
        assert_eq!(d.observe(0.0, add_ms(t0, 1_100)), SilenceEvent::Silent);
        // Past new 800 ms window (700 + 800 = 1500 ms total).
        assert_eq!(
            d.observe(0.0, add_ms(t0, 1_501)),
            SilenceEvent::SustainedSilence
        );
    }

    // ------------------------------------------------------------------
    // sustained_fired flag resets when speech resumes.
    // ------------------------------------------------------------------
    #[test]
    fn new_silence_run_after_speech_can_fire_again() {
        let mut d = detector();
        let t0 = Instant::now();

        // First silence run fires.
        d.observe(0.0, t0);
        assert_eq!(
            d.observe(0.0, add_ms(t0, 801)),
            SilenceEvent::SustainedSilence
        );

        // Speech resets state.
        assert_eq!(d.observe(0.1, add_ms(t0, 900)), SilenceEvent::Speaking);

        // Second silence run — should be able to fire again.
        let t1 = add_ms(t0, 1_000);
        assert_eq!(d.observe(0.0, t1), SilenceEvent::Silent);
        assert_eq!(
            d.observe(0.0, t1 + Duration::from_millis(801)),
            SilenceEvent::SustainedSilence
        );
    }
}

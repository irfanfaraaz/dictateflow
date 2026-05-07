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
    /// True once we've seen at least one Speaking event. Pre-speech silence
    /// (mic warmup, BT HFP profile switch) doesn't count toward the silence
    /// window — otherwise the VAD fires before the user even starts talking.
    heard_speech: bool,
}

impl SilenceDetector {
    pub fn new(config: VadConfig) -> Self {
        Self {
            config,
            silence_started_at: None,
            sustained_fired: false,
            heard_speech: false,
        }
    }

    /// Feed one RMS sample at wall-clock time `now`.
    ///
    /// Call this at the same cadence as the `watch::Receiver<f32>` emits
    /// (the level pump delivers values whenever cpal ticks, typically every
    /// ~10-20 ms on Apple Silicon hardware).
    pub fn observe(&mut self, rms: f32, now: Instant) -> SilenceEvent {
        if rms >= self.config.rms_threshold {
            self.heard_speech = true;
            self.silence_started_at = None;
            self.sustained_fired = false;
            return SilenceEvent::Speaking;
        }

        // Below threshold — silence. But don't start the silence timer until
        // we've heard speech at least once. This prevents BT mic warmup delay,
        // HFP profile switching, or the user pausing before speaking from
        // triggering a premature auto-stop.
        if !self.heard_speech {
            return SilenceEvent::Silent;
        }

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
    // Pure silence (no prior speech) never escalates — prevents premature
    // auto-stop during mic warmup / BT HFP profile switch.
    // ------------------------------------------------------------------
    #[test]
    fn pure_silence_without_speech_never_escalates() {
        let mut d = detector();
        let t0 = Instant::now();

        assert_eq!(d.observe(0.0, t0), SilenceEvent::Silent);
        assert_eq!(d.observe(0.0, add_ms(t0, 400)), SilenceEvent::Silent);
        assert_eq!(d.observe(0.0, add_ms(t0, 801)), SilenceEvent::Silent);
        assert_eq!(d.observe(0.0, add_ms(t0, 2_000)), SilenceEvent::Silent);
    }

    // ------------------------------------------------------------------
    // Speech then silence escalates after window.
    // ------------------------------------------------------------------
    #[test]
    fn speech_then_silence_escalates_after_window() {
        let mut d = detector();
        let t0 = Instant::now();

        // Speech first.
        assert_eq!(d.observe(0.1, t0), SilenceEvent::Speaking);

        // Silence begins.
        assert_eq!(d.observe(0.0, add_ms(t0, 100)), SilenceEvent::Silent);
        assert_eq!(d.observe(0.0, add_ms(t0, 500)), SilenceEvent::Silent);

        // Past 800 ms of silence — fires.
        assert_eq!(
            d.observe(0.0, add_ms(t0, 901)),
            SilenceEvent::SustainedSilence
        );

        // Subsequent silent samples → Silent (not repeated).
        assert_eq!(d.observe(0.0, add_ms(t0, 1_000)), SilenceEvent::Silent);
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

        // Speech first (required to arm the VAD).
        assert_eq!(d.observe(0.1, t0), SilenceEvent::Speaking);

        // Silence for 500 ms (below window).
        assert_eq!(d.observe(0.0, add_ms(t0, 100)), SilenceEvent::Silent);
        assert_eq!(d.observe(0.0, add_ms(t0, 500)), SilenceEvent::Silent);

        // Speech resumes — resets the run.
        assert_eq!(d.observe(0.1, add_ms(t0, 600)), SilenceEvent::Speaking);

        // Silence run restarts from the first silent sample after speech.
        assert_eq!(d.observe(0.0, add_ms(t0, 700)), SilenceEvent::Silent);
        assert_eq!(d.observe(0.0, add_ms(t0, 800)), SilenceEvent::Silent);
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

        // Speech arms the VAD.
        assert_eq!(d.observe(0.1, t0), SilenceEvent::Speaking);

        // First silence run fires.
        assert_eq!(d.observe(0.0, add_ms(t0, 100)), SilenceEvent::Silent);
        assert_eq!(
            d.observe(0.0, add_ms(t0, 901)),
            SilenceEvent::SustainedSilence
        );

        // Speech resets state.
        assert_eq!(d.observe(0.1, add_ms(t0, 1_000)), SilenceEvent::Speaking);

        // Second silence run — should be able to fire again.
        let t1 = add_ms(t0, 1_100);
        assert_eq!(d.observe(0.0, t1), SilenceEvent::Silent);
        assert_eq!(
            d.observe(0.0, t1 + Duration::from_millis(801)),
            SilenceEvent::SustainedSilence
        );
    }
}

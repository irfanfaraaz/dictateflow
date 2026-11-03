//! Voice-activity detection (VAD).
//!
//! The pill's waveform drives the level display; this module uses the same RMS
//! levels to detect sustained silence and trigger early-stop of a
//! **toggle-mode** dictation (hold mode ends when the key is released, so it
//! never runs the VAD).
//!
//! # Calibration note
//!
//! `VadConfig::rms_threshold` (default 0.005) is tuned for the capture
//! pipeline's channel-averaged mono f32 RMS, as published by
//! `audio::capture` through `LevelMeter` and polled at ~30 Hz (each observed
//! value is the *peak* RMS over the ~33 ms poll window). If the capture
//! pipeline changes its gain or sample format, re-tune this threshold.

use std::time::{Duration, Instant};

// ---------- Public types ----------

/// Configuration for the silence detector.
///
/// Defaults: the threshold is comfortably below normal speech but above the
/// microphone noise floor on typical macOS hardware; the detector only arms
/// after `min_speech_ms` of *continuous* speech (so a click or cough can't
/// arm it), and a 2 s silence window gives the speaker a natural thinking
/// pause before we call it done.
#[derive(Debug, Clone)]
pub struct VadConfig {
    /// RMS amplitude below which a sample is considered silence.
    pub rms_threshold: f32,
    /// How long (ms) the level must stay continuously above the threshold
    /// before the detector arms. Pre-arm silence never counts.
    pub min_speech_ms: u32,
    /// How long (ms) silence must hold continuously before `SustainedSilence`
    /// fires. After it fires once it will not fire again until speech resumes
    /// and then a new silence run begins.
    pub silence_window_ms: u32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            rms_threshold: 0.005,
            min_speech_ms: 200,
            silence_window_ms: 2_000,
        }
    }
}

/// Events returned by [`SilenceDetector::observe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SilenceEvent {
    /// RMS is above threshold — the user is speaking.
    Speaking,
    /// RMS is below threshold and silence hasn't held long enough yet (or the
    /// detector hasn't armed).
    Silent,
    /// Silence has held continuously past `silence_window_ms`. Fires exactly
    /// once per silence run; subsequent calls return [`SilenceEvent::Silent`]
    /// until speech resumes and another silence run completes.
    SustainedSilence,
}

/// Stateful observer: feed RMS levels + wall-clock timestamps, receive
/// [`SilenceEvent`]s.
///
/// Create one per dictation session and drop it when the session ends.
pub struct SilenceDetector {
    config: VadConfig,
    /// When the current above-threshold run started (`None` while silent).
    speech_started_at: Option<Instant>,
    /// When the current silence run started. `None` if the speaker is active.
    silence_started_at: Option<Instant>,
    /// True once `SustainedSilence` has been emitted for the current run, so
    /// we don't keep firing it on every subsequent silent sample.
    sustained_fired: bool,
    /// True once we've seen `min_speech_ms` of continuous speech. Pre-speech
    /// silence (mic warmup, BT HFP profile switch) doesn't count toward the
    /// silence window — otherwise the VAD fires before the user even starts
    /// talking.
    armed: bool,
}

impl SilenceDetector {
    pub fn new(config: VadConfig) -> Self {
        Self {
            config,
            speech_started_at: None,
            silence_started_at: None,
            sustained_fired: false,
            armed: false,
        }
    }

    /// Feed one RMS level observed at wall-clock time `now`.
    ///
    /// Called by the dictation monitor at ~30 Hz with the peak level of each
    /// poll window.
    pub fn observe(&mut self, rms: f32, now: Instant) -> SilenceEvent {
        if rms >= self.config.rms_threshold {
            let started = *self.speech_started_at.get_or_insert(now);
            let min_speech = Duration::from_millis(u64::from(self.config.min_speech_ms));
            if now.duration_since(started) >= min_speech {
                self.armed = true;
            }
            self.silence_started_at = None;
            self.sustained_fired = false;
            return SilenceEvent::Speaking;
        }

        self.speech_started_at = None;

        // Below threshold — silence. Don't start the silence timer until the
        // detector has armed on real speech.
        if !self.armed {
            return SilenceEvent::Silent;
        }

        let started = self.silence_started_at.get_or_insert(now);
        let elapsed = now.duration_since(*started);
        let window = Duration::from_millis(u64::from(self.config.silence_window_ms));

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
            min_speech_ms: 200,
            silence_window_ms: 2_000,
        })
    }

    /// Helper: advance `now` by `ms` milliseconds.
    fn add_ms(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    /// Feed continuous speech at the 30 Hz poll cadence from `from_ms` for
    /// `dur_ms`, returning the timestamp of the last speech sample (ms).
    fn speak(d: &mut SilenceDetector, t0: Instant, from_ms: u64, dur_ms: u64) -> u64 {
        let mut t = from_ms;
        while t <= from_ms + dur_ms {
            assert_eq!(d.observe(0.1, add_ms(t0, t)), SilenceEvent::Speaking);
            t += 33;
        }
        t - 33
    }

    #[test]
    fn defaults_match_toggle_mode_tuning() {
        let c = VadConfig::default();
        assert_eq!(c.silence_window_ms, 2_000);
        assert_eq!(c.min_speech_ms, 200);
    }

    // Pure silence (no prior speech) never escalates — prevents premature
    // auto-stop during mic warmup / BT HFP profile switch.
    #[test]
    fn pure_silence_without_speech_never_escalates() {
        let mut d = detector();
        let t0 = Instant::now();
        for ms in [0, 400, 2_001, 5_000, 10_000] {
            assert_eq!(d.observe(0.0, add_ms(t0, ms)), SilenceEvent::Silent);
        }
    }

    // A single short spike (a click, a cough, a 10 ms glitch) must not arm
    // the detector — otherwise the silence that follows auto-stops before
    // the user has said anything.
    #[test]
    fn short_spike_does_not_arm() {
        let mut d = detector();
        let t0 = Instant::now();
        assert_eq!(d.observe(0.2, t0), SilenceEvent::Speaking);
        assert_eq!(d.observe(0.2, add_ms(t0, 33)), SilenceEvent::Speaking);
        for ms in (66..6_000).step_by(33) {
            assert_eq!(
                d.observe(0.0, add_ms(t0, ms)),
                SilenceEvent::Silent,
                "fired at {ms} ms"
            );
        }
    }

    // Speech then silence escalates only after the full window.
    #[test]
    fn speech_then_silence_escalates_after_window() {
        let mut d = detector();
        let t0 = Instant::now();
        let last = speak(&mut d, t0, 0, 300);

        let s0 = last + 33;
        assert_eq!(d.observe(0.0, add_ms(t0, s0)), SilenceEvent::Silent);
        assert_eq!(d.observe(0.0, add_ms(t0, s0 + 1_000)), SilenceEvent::Silent);
        assert_eq!(d.observe(0.0, add_ms(t0, s0 + 1_999)), SilenceEvent::Silent);
        assert_eq!(
            d.observe(0.0, add_ms(t0, s0 + 2_000)),
            SilenceEvent::SustainedSilence
        );
        // Not repeated.
        assert_eq!(d.observe(0.0, add_ms(t0, s0 + 2_100)), SilenceEvent::Silent);
    }

    // Speaking input never escalates.
    #[test]
    fn speaking_never_escalates() {
        let mut d = detector();
        let t0 = Instant::now();
        for i in 0..300 {
            let ev = d.observe(0.1, add_ms(t0, i * 33));
            assert_eq!(ev, SilenceEvent::Speaking, "expected Speaking at tick {i}");
        }
    }

    // Speaking → short pause → speaking resets the silence run.
    #[test]
    fn speech_interrupts_silence_run() {
        let mut d = detector();
        let t0 = Instant::now();
        let last = speak(&mut d, t0, 0, 300);

        // 1.5 s pause — below the window.
        assert_eq!(d.observe(0.0, add_ms(t0, last + 33)), SilenceEvent::Silent);
        assert_eq!(
            d.observe(0.0, add_ms(t0, last + 1_500)),
            SilenceEvent::Silent
        );

        // Speech resumes.
        assert_eq!(
            d.observe(0.1, add_ms(t0, last + 1_533)),
            SilenceEvent::Speaking
        );

        // The new silence run starts from its first silent sample.
        let s1 = last + 1_566;
        assert_eq!(d.observe(0.0, add_ms(t0, s1)), SilenceEvent::Silent);
        assert_eq!(d.observe(0.0, add_ms(t0, s1 + 1_999)), SilenceEvent::Silent);
        assert_eq!(
            d.observe(0.0, add_ms(t0, s1 + 2_000)),
            SilenceEvent::SustainedSilence
        );
    }

    // sustained_fired resets when speech resumes.
    #[test]
    fn new_silence_run_after_speech_can_fire_again() {
        let mut d = detector();
        let t0 = Instant::now();
        let last = speak(&mut d, t0, 0, 300);
        let s0 = last + 33;
        assert_eq!(d.observe(0.0, add_ms(t0, s0)), SilenceEvent::Silent);
        assert_eq!(
            d.observe(0.0, add_ms(t0, s0 + 2_000)),
            SilenceEvent::SustainedSilence
        );

        let s1 = s0 + 2_100;
        assert_eq!(d.observe(0.1, add_ms(t0, s1)), SilenceEvent::Speaking);
        assert_eq!(d.observe(0.0, add_ms(t0, s1 + 33)), SilenceEvent::Silent);
        assert_eq!(
            d.observe(0.0, add_ms(t0, s1 + 2_033)),
            SilenceEvent::SustainedSilence
        );
    }
}

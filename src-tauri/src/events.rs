//! Event names + payloads emitted Rust → frontend.
//!
//! Web analogue: think Server-Sent Events. The frontend `listen('name', cb)`s;
//! Rust calls `app.emit('name', payload)`. Payloads serialize via serde and
//! land in JS as plain objects validated by zod schemas in
//! `src/schemas/events.ts`.
//!
//! All payloads MUST be cheap to serialize — they're emitted at up to 30 Hz
//! during dictation (audio level events) and any allocation-heavy work would
//! show up as input latency.

#![allow(dead_code)] // surfaced by frontend in Task 1.7

use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};
use tracing::{debug, warn};

use crate::app::DictationSession;
use crate::audio::capture;
use crate::window;

// ---------- Event names ----------

pub const EV_DICTATION_START: &str = "dictation:start";
pub const EV_DICTATION_AUDIO_LEVEL: &str = "dictation:audio_level";
pub const EV_DICTATION_INJECTED: &str = "dictation:injected";
pub const EV_DICTATION_ERROR: &str = "dictation:error";

// ---------- Payloads ----------

#[derive(Debug, Serialize, Clone)]
pub struct DictationStartPayload {
    pub trigger: &'static str, // "hold" | "toggle" — Phase 8 wires toggle
}

#[derive(Debug, Serialize, Clone)]
pub struct AudioLevelPayload {
    /// Root-mean-square amplitude in [0, ~1]. Unsmoothed; the pill applies
    /// its own visual easing.
    pub rms: f32,
}

#[derive(Debug, Serialize, Clone)]
pub struct InjectedPayload {
    pub text: String,
    pub restored_clipboard: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct ErrorPayload {
    pub code: &'static str,
    pub message: String,
    pub recovery_hint: Option<String>,
}

// ---------- Hot-path orchestration ----------

/// Cadence of `dictation:audio_level` event emission. 30 Hz keeps the pill
/// waveform visually smooth while staying well below IPC saturation; the
/// audio callback updates the watch channel at ~50–100 Hz, but we only
/// forward at 30 Hz.
const LEVEL_EVENT_HZ: u64 = 30;

/// Pill fade-out delay after stop. Matches the design spec's ~250 ms cross-
/// fade so the window is gone exactly when the user perceives the pill to
/// have "let go."
const PILL_HIDE_DELAY: Duration = Duration::from_millis(250);

/// Begin a dictation: show the pill, start capture, spawn the level pump.
///
/// Returns the live session — caller stores it in the
/// [`DictationController`] until stop.
pub async fn start_dictation<R: Runtime>(app: &AppHandle<R>) -> Result<DictationSession> {
    // 1. Audio first — if the mic is denied, we want to fail before the
    //    pill flashes on screen.
    let (capture_session, _consumer, level_rx) = capture::start().context("start audio capture")?;

    // 2. Show the pill (must hop to the macOS main thread).
    show_pill_async(app);

    // 3. Tell the frontend.
    if let Err(e) = app.emit(
        EV_DICTATION_START,
        DictationStartPayload { trigger: "hold" },
    ) {
        warn!(?e, "emit dictation:start failed");
    }

    // 4. Spawn the 30 Hz level pump.
    let app_for_pump = app.clone();
    let level_pump = tokio::spawn(level_pump(app_for_pump, level_rx));

    Ok(DictationSession::new(capture_session, level_pump))
}

/// End an in-flight dictation: stop capture, await the pump, hide the pill,
/// emit the (placeholder) injected event.
pub async fn stop_dictation<R: Runtime>(app: &AppHandle<R>, session: DictationSession) {
    // Drop the capture guard first — this stops the mic. The level pump
    // will fall out of its loop when the watch sender is dropped.
    let DictationSession {
        capture,
        level_pump,
    } = session.into_parts();
    drop(capture);
    level_pump.abort();

    // Phase 1 has no transcription. Emit a placeholder `dictation:injected`
    // so the frontend can run its fade-out path; later phases will emit
    // this only after real injection.
    if let Err(e) = app.emit(
        EV_DICTATION_INJECTED,
        InjectedPayload {
            text: String::new(),
            restored_clipboard: false,
        },
    ) {
        warn!(?e, "emit dictation:injected failed");
    }

    // Wait briefly so the frontend can play its fade-out before the window
    // actually disappears.
    tokio::time::sleep(PILL_HIDE_DELAY).await;
    hide_pill_async(app);
}

/// Pump RMS values from the watch channel out as `dictation:audio_level`
/// events at most `LEVEL_EVENT_HZ` times per second.
async fn level_pump<R: Runtime>(app: AppHandle<R>, mut rx: tokio::sync::watch::Receiver<f32>) {
    let interval = Duration::from_millis(1000 / LEVEL_EVENT_HZ);
    let mut last_emit = Instant::now() - interval;

    // `changed()` returns Err when the sender is dropped (capture stopped).
    while rx.changed().await.is_ok() {
        let now = Instant::now();
        if now.duration_since(last_emit) < interval {
            continue;
        }
        last_emit = now;
        let level = *rx.borrow_and_update();
        if let Err(e) = app.emit(EV_DICTATION_AUDIO_LEVEL, AudioLevelPayload { rms: level }) {
            debug!(?e, "emit audio_level failed");
        }
    }
}

fn show_pill_async<R: Runtime>(app: &AppHandle<R>) {
    let app2 = app.clone();
    if let Err(e) = app.run_on_main_thread(move || {
        if let Err(e) = window::show_at_cursor(&app2) {
            warn!(?e, "show_at_cursor failed");
        }
    }) {
        warn!(?e, "run_on_main_thread (show) dispatch failed");
    }
}

fn hide_pill_async<R: Runtime>(app: &AppHandle<R>) {
    let app2 = app.clone();
    if let Err(e) = app.run_on_main_thread(move || {
        if let Err(e) = window::hide(&app2) {
            warn!(?e, "hide pill failed");
        }
    }) {
        warn!(?e, "run_on_main_thread (hide) dispatch failed");
    }
}

// Type alias used in the "no-op start failure" test in later phases.
#[allow(dead_code)]
pub type EventResult<T> = std::result::Result<T, anyhow::Error>;

// Keep `anyhow!` reachable for any caller that imports from this module.
#[allow(dead_code)]
fn _force_anyhow_in_scope() -> anyhow::Error {
    anyhow!("placeholder")
}

#[cfg(test)]
mod tests {
    #[test]
    fn level_event_cadence_under_60hz() {
        // 30 Hz target → 33 ms interval; sanity-check the constant.
        let interval_ms = 1000 / super::LEVEL_EVENT_HZ;
        assert!((30..=40).contains(&interval_ms));
    }
}

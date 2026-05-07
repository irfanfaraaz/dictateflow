//! Event names + payloads emitted Rust → frontend, plus the
//! start/stop-dictation orchestration that wires audio capture, the
//! inference engine, and the pill window together.
//!
//! Web analogue: think Server-Sent Events. The frontend `listen('name', cb)`s;
//! Rust calls `app.emit('name', payload)`. Payloads serialize via serde and
//! land in JS as plain objects validated by zod schemas in
//! `src/schemas/events.ts`.
//!
//! All payloads MUST be cheap to serialize — they're emitted at up to 30 Hz
//! during dictation (audio level events) and any allocation-heavy work would
//! show up as input latency.

#![allow(dead_code)] // surfaced by frontend incrementally

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use futures::StreamExt;
use ringbuf::traits::{Consumer, Observer};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::app::{AppState, DictationSession};
use crate::audio::capture;
use crate::engine::{local::LocalEngine, AudioChunk, Engine, EngineError};
use crate::inject;
use crate::models;
use crate::window;

// ---------- Event names ----------

pub const EV_DICTATION_START: &str = "dictation:start";
pub const EV_DICTATION_AUDIO_LEVEL: &str = "dictation:audio_level";
pub const EV_DICTATION_INJECTED: &str = "dictation:injected";
pub const EV_DICTATION_ERROR: &str = "dictation:error";
/// PHASE 2 DEBUG ONLY — emitted with the raw transcript so the pill can
/// briefly flash it. Phase 3 deletes both this event and the pill
/// subscription; instead the controller invokes the clipboard injector.
pub const EV_DICTATION_FINAL: &str = "dictation:final";

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

#[derive(Debug, Serialize, Clone)]
pub struct FinalPayload {
    pub text: String,
}

// ---------- Hot-path orchestration ----------

/// Cadence of `dictation:audio_level` event emission. 30 Hz keeps the pill
/// waveform visually smooth while staying well below IPC saturation; the
/// audio callback updates the watch channel at ~50–100 Hz, but we only
/// forward at 30 Hz.
const LEVEL_EVENT_HZ: u64 = 30;

/// Pill fade-out delay after stop. Matches the design spec's ~250 ms cross-
/// fade so the window is gone exactly when the user perceives the pill to
/// have "let go." Phase 3 trimmed this back from the Phase 2 debug value
/// (1.7 s) now that the pill no longer flashes the transcript text — the
/// 250 ms also overlaps the "pasting…" beat the pill renders on
/// `dictation:injected`.
const PILL_HIDE_DELAY: Duration = Duration::from_millis(250);

/// Cadence at which the audio drain task copies samples out of the cpal
/// ringbuf into the engine's mpsc. 100 ms is a comfortable mid-point — far
/// above the 5 s ringbuf capacity so we never overflow under realistic
/// scheduling jitter, but tight enough that hold-to-talk releases finalize
/// promptly.
const AUDIO_PUMP_INTERVAL: Duration = Duration::from_millis(100);

/// Bounded mpsc capacity between the audio drain task and the engine.
/// Each chunk is ~1600 f32 samples (= 6.4 KiB). 64 chunks ≈ 6.4 s of audio
/// in flight; the engine's batch loop drains immediately so the queue
/// stays near-empty in practice.
const ENGINE_CHANNEL_CAPACITY: usize = 64;

/// Begin a dictation: load the engine if needed, show the pill, start
/// capture, and spawn the level / audio / engine pumps.
///
/// Returns the live session — caller stores it in the
/// [`DictationController`] until stop. On any setup failure we surface a
/// `dictation:error` event and bail out without showing the pill.
pub async fn start_dictation<R: Runtime>(app: &AppHandle<R>) -> Result<DictationSession> {
    // 0. Resolve / load the engine first. Loading the model is a one-time
    //    cost the first dictation pays; subsequent presses reuse the
    //    cached `Arc<dyn Engine>` from `AppState`.
    let engine = ensure_engine_loaded(app).context("load engine")?;

    // 1. Audio next — if the mic is denied, fail before the pill flashes.
    let (capture_session, mut consumer, level_rx) =
        capture::start().context("start audio capture")?;

    // 2. Show the pill (must hop to the macOS main thread).
    show_pill_async(app);

    // 3. Tell the frontend we've begun.
    if let Err(e) = app.emit(
        EV_DICTATION_START,
        DictationStartPayload { trigger: "hold" },
    ) {
        warn!(?e, "emit dictation:start failed");
    }

    // 4. Spawn the 30 Hz level pump (drives the pill waveform).
    let app_for_levels = app.clone();
    let level_pump = tokio::spawn(level_pump(app_for_levels, level_rx));

    // 5. Build the engine pipeline: audio_pump → mpsc → engine.stream → emit.
    let cancel = CancellationToken::new();
    let audio_stop = CancellationToken::new();
    let (audio_tx, audio_rx) = mpsc::channel::<AudioChunk>(ENGINE_CHANNEL_CAPACITY);

    // Audio pump: copy from cpal ringbuf into the mpsc every
    // AUDIO_PUMP_INTERVAL. Exits when `audio_stop` fires (after one final
    // drain of any residual samples). The captured `audio_tx` is dropped
    // when this task returns, which closes the engine's input channel and
    // triggers normal end-of-stream finalisation.
    let audio_stop_for_pump = audio_stop.clone();
    let audio_pump = tokio::spawn(async move {
        // Reuse a single scratch buffer; no per-tick allocation.
        // 1600 samples = 100 ms at 16 kHz.
        let mut scratch = vec![0.0_f32; 16_000 / 10];
        let mut ticker = tokio::time::interval(AUDIO_PUMP_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                biased;
                _ = audio_stop_for_pump.cancelled() => {
                    // One last drain so we don't lose the tail of the
                    // hold-to-talk audio that arrived between the previous
                    // tick and the stop signal.
                    drain_consumer(&mut consumer, &mut scratch, &audio_tx).await;
                    return;
                }
                _ = ticker.tick() => {
                    drain_consumer(&mut consumer, &mut scratch, &audio_tx).await;
                }
            }
        }
    });

    // Engine pump: drive the partial stream and emit `dictation:final` on
    // the terminating `is_final = true` partial.
    let app_for_engine = app.clone();
    let engine_for_task = engine.clone();
    let cancel_for_engine = cancel.clone();
    let engine_pump = tokio::spawn(async move {
        let stream_result = engine_for_task.stream(audio_rx, cancel_for_engine).await;
        let mut stream = match stream_result {
            Ok(s) => s,
            Err(e) => {
                warn!(?e, "engine.stream() failed");
                emit_error(&app_for_engine, &e);
                return;
            }
        };

        while let Some(item) = stream.next().await {
            match item {
                Ok(p) => {
                    debug!(text = %p.text, is_final = p.is_final, "engine partial");
                    if p.is_final {
                        // `dictation:final` still fires for downstream
                        // consumers (Phase 9 may surface it as a tray
                        // history). The pill no longer subscribes — see
                        // shape brief §3.
                        if let Err(e) = app_for_engine.emit(
                            EV_DICTATION_FINAL,
                            FinalPayload {
                                text: p.text.clone(),
                            },
                        ) {
                            warn!(?e, "emit dictation:final failed");
                        }

                        let trimmed = p.text.trim();
                        if trimmed.is_empty() {
                            // Whisper sometimes returns "" or
                            // "[BLANK_AUDIO]" on silence. Skip injection
                            // and let the pill fade quietly.
                            info!("skipping injection: empty transcript");
                        } else {
                            match inject::paste(&p.text).await {
                                Ok(()) => {
                                    if let Err(e) = app_for_engine.emit(
                                        EV_DICTATION_INJECTED,
                                        InjectedPayload {
                                            text: p.text,
                                            restored_clipboard: true,
                                        },
                                    ) {
                                        warn!(?e, "emit dictation:injected failed");
                                    }
                                }
                                Err(e) => {
                                    warn!(?e, "inject::paste failed");
                                    let payload = ErrorPayload {
                                        code: "inject_failed",
                                        message: e.to_string(),
                                        recovery_hint: Some(
                                            "Grant Accessibility permission in System \
                                             Settings → Privacy & Security → Accessibility."
                                                .into(),
                                        ),
                                    };
                                    if let Err(e2) =
                                        app_for_engine.emit(EV_DICTATION_ERROR, payload)
                                    {
                                        warn!(?e2, "emit dictation:error failed");
                                    }
                                }
                            }
                        }
                    }
                    // Phase 6 will wire intermediate (is_final=false)
                    // partials into Phase 9's silence-detection heuristic.
                }
                Err(EngineError::Cancelled) => {
                    info!("dictation cancelled");
                }
                Err(e) => {
                    warn!(?e, "engine yielded error");
                    emit_error(&app_for_engine, &e);
                }
            }
        }
    });

    Ok(DictationSession::new(
        capture_session,
        level_pump,
        audio_pump,
        engine_pump,
        audio_stop,
        cancel,
    ))
}

/// Pop everything currently in the cpal consumer and forward it to the
/// engine's mpsc as one or more chunks. Returns immediately if the consumer
/// is empty. Bails on the first send failure (engine receiver dropped).
async fn drain_consumer(
    consumer: &mut ringbuf::HeapCons<f32>,
    scratch: &mut [f32],
    audio_tx: &mpsc::Sender<AudioChunk>,
) {
    let occupied = consumer.occupied_len();
    if occupied == 0 {
        return;
    }
    let mut total_sent = 0usize;
    while total_sent < occupied {
        let popped = consumer.pop_slice(scratch);
        if popped == 0 {
            return;
        }
        let chunk = AudioChunk {
            samples: scratch[..popped].to_vec(),
        };
        if audio_tx.send(chunk).await.is_err() {
            return;
        }
        total_sent += popped;
    }
}

/// End an in-flight dictation: stop capture, await the engine pump, hide
/// the pill, emit the (placeholder) injected event.
pub async fn stop_dictation<R: Runtime>(app: &AppHandle<R>, session: DictationSession) {
    let DictationSession {
        capture,
        level_pump,
        audio_pump,
        engine_pump,
        audio_stop,
        cancel,
    } = session;

    // 1. Stop the mic. The audio_pump's `audio_stop` will catch the residual
    //    ringbuf contents on its way out.
    drop(capture);

    // 2. Stop the level event emitter — the watch channel is dead now.
    level_pump.abort();

    // 3. Give the cpal callback a tick to land its final buffer in the
    //    ringbuf, then signal the audio pump to drain + exit. The pump
    //    drops `audio_tx` on return, closing the engine's input channel,
    //    which causes the batch LocalEngine to finalise.
    tokio::time::sleep(AUDIO_PUMP_INTERVAL).await;
    audio_stop.cancel();
    if let Err(e) = audio_pump.await {
        warn!(?e, "audio pump did not exit cleanly");
    }

    // 4. Wait for the engine to emit its final partial. We do NOT trigger
    //    `cancel` here — cancellation is reserved for cancel-on-restart
    //    (Phase 9). The 15 s cap protects against a stuck engine without
    //    silently dropping a real transcript.
    if let Err(e) = tokio::time::timeout(Duration::from_secs(15), engine_pump).await {
        warn!(?e, "engine pump did not complete in 15s; continuing anyway");
    }

    // 5. Drop the unused cancellation token; nothing else holds it.
    drop(cancel);

    // `dictation:injected` is emitted by the engine pump once the paste
    // actually lands; we don't fire a placeholder here. If the engine
    // pump exited without injecting (silence transcript, error, timeout),
    // the pill simply falls through its idle state and fades.

    // Brief overlap with the pill's "pasting…" beat before fade-out.
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

/// Load the engine via `AppState`'s cache, or build it on first use.
///
/// Phase 7 introduces a settings-driven factory that swaps engines on
/// `settings:updated`; Phase 2 hard-codes `LocalEngine` against the
/// `small` model.
fn ensure_engine_loaded<R: Runtime>(app: &AppHandle<R>) -> Result<Arc<dyn Engine>> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| anyhow!("AppState missing from Tauri state"))?;

    if let Some(engine) = state.engine() {
        return Ok(engine);
    }

    let model_path =
        models::resolve_path(app, "small").with_context(|| "resolve ggml-small model path")?;
    let engine = LocalEngine::load(&model_path)
        .with_context(|| format!("load LocalEngine from {}", model_path.display()))?;
    let engine: Arc<dyn Engine> = Arc::new(engine);
    state.set_engine(engine.clone());
    info!(model = %model_path.display(), "engine loaded");
    Ok(engine)
}

fn emit_error<R: Runtime>(app: &AppHandle<R>, err: &EngineError) {
    let code: &'static str = match err {
        EngineError::MissingApiKey => "missing_api_key",
        EngineError::ModelNotInstalled(_) => "model_not_installed",
        EngineError::Audio(_) => "audio",
        EngineError::Inference(_) => "inference",
        EngineError::Cancelled => "cancelled",
        EngineError::Io(_) => "io",
        EngineError::WebSocket(_) => "websocket",
    };
    let payload = ErrorPayload {
        code,
        message: err.to_string(),
        recovery_hint: None,
    };
    if let Err(e) = app.emit(EV_DICTATION_ERROR, payload) {
        warn!(?e, "emit dictation:error failed");
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

#[cfg(test)]
mod tests {
    #[test]
    fn level_event_cadence_under_60hz() {
        // 30 Hz target → 33 ms interval; sanity-check the constant.
        let interval_ms = 1000 / super::LEVEL_EVENT_HZ;
        assert!((30..=40).contains(&interval_ms));
    }

    #[test]
    fn engine_channel_capacity_reasonable() {
        // 64 chunks * 100 ms ≈ 6.4 s — comfortably above the audio pump
        // tick; reduce only if memory becomes a concern.
        assert!((16..=256).contains(&super::ENGINE_CHANNEL_CAPACITY));
    }
}

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

use std::sync::atomic::{AtomicU64, Ordering};
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
use crate::audio::vad::{SilenceDetector, VadConfig};
use crate::engine::{factory, AudioChunk, Engine, EngineError};
use crate::inject;
use crate::settings::SettingsStore;
use crate::sounds;
use crate::tray::{self, TrayState};
use crate::window;

// ---------- Event names ----------

pub const EV_DICTATION_START: &str = "dictation:start";
pub const EV_DICTATION_AUDIO_LEVEL: &str = "dictation:audio_level";
/// Emitted the moment capture stops (hotkey released / VAD / cap) so the pill
/// can switch from the live waveform to its "transcribing" animation.
pub const EV_DICTATION_STOP: &str = "dictation:stop";
/// Emitted just before the pill window hides so the pill can fade out
/// instead of vanishing abruptly.
pub const EV_DICTATION_END: &str = "dictation:end";
pub const EV_DICTATION_INJECTED: &str = "dictation:injected";
pub const EV_DICTATION_ERROR: &str = "dictation:error";
/// PHASE 2 DEBUG ONLY — emitted with the raw transcript so the pill can
/// briefly flash it. Phase 3 deletes both this event and the pill
/// subscription; instead the controller invokes the clipboard injector.
pub const EV_DICTATION_FINAL: &str = "dictation:final";
/// Emitted after engine construction so the settings UI can show a health dot.
pub const EV_ENGINE_STATUS: &str = "engine:status";
/// Emitted by the cap timer when the session has reached its maximum duration.
/// Payload carries the cap that was enforced so the pill can show a warning.
pub const EV_DICTATION_CAP_REACHED: &str = "dictation:cap_reached";

// ---------- Model download event names (Phase 5) ----------

/// Emitted periodically during a model download.
pub const EV_MODEL_DOWNLOAD_PROGRESS: &str = "model:download_progress";
/// Emitted when a model download completes successfully.
pub const EV_MODEL_DOWNLOAD_COMPLETE: &str = "model:download_complete";
/// Emitted when a model download fails.
pub const EV_MODEL_DOWNLOAD_ERROR: &str = "model:download_error";

// ---------- Payloads ----------

#[derive(Debug, Serialize, Clone)]
pub struct DictationStartPayload {
    pub trigger: &'static str, // "hold" | "toggle"
    /// Engine-aware cap in seconds. Pill uses this to compute the 80% warning
    /// threshold and to display `mm:ss / cap` without an extra event.
    pub cap_seconds: u32,
}

#[derive(Debug, Serialize, Clone)]
pub struct CapReachedPayload {
    pub cap_seconds: u32,
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

#[derive(Debug, Serialize, Clone)]
pub struct EngineStatusPayload {
    pub engine: String,
    pub healthy: bool,
    pub last_error: Option<String>,
}

// ---------- Model download payloads (Phase 5) ----------

#[derive(Debug, Serialize, Clone)]
pub struct ModelDownloadProgressPayload {
    pub id: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug, Serialize, Clone)]
pub struct ModelDownloadCompletePayload {
    pub id: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct ModelDownloadErrorPayload {
    pub id: String,
    pub message: String,
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

/// How long the error pill stays up after a failed start (pill fades itself
/// at ~2.2 s; the window is hidden just after).
const START_ERROR_LINGER: Duration = Duration::from_millis(2500);

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
/// capture, and spawn the level / audio / engine / cap pumps.
///
/// Returns the live session — caller stores it in the
/// [`DictationController`] until stop. On any setup failure we surface a
/// `dictation:error` event and bail out without showing the pill.
pub async fn start_dictation<R: Runtime>(app: &AppHandle<R>) -> Result<DictationSession> {
    use crate::settings::schema::EngineKind;

    // 0a. Guard: local engine with no installed models → emit error chip and bail.
    //
    // `LocalEngine::load` would return `ModelNotInstalled` anyway, but we
    // short-circuit here so the pill shows the error chip without needing to
    // start capture. The pill auto-shows on `dictation:error` (see Pill.tsx).
    let (engine_kind, cap_seconds) = {
        let store = app.state::<SettingsStore>();
        let settings = store.get();
        if settings.engine == EngineKind::Local && settings.local.installed_models.is_empty() {
            warn!("start_dictation: local engine selected but no models installed");
            let payload = ErrorPayload {
                code: "model_not_installed",
                message: "No model installed. Download a model in Settings → Models.".into(),
                recovery_hint: Some("open settings → models to download a model".into()),
            };
            if let Err(e) = app.emit(EV_DICTATION_ERROR, payload) {
                warn!(?e, "emit dictation:error (no model) failed");
            }
            if let Err(e) = tray::set_state(app, TrayState::Error) {
                warn!(?e, "tray set_state Error (no model) failed");
            }
            return Err(anyhow!("no model installed"));
        }
        let cap = match settings.engine {
            EngineKind::Local => settings.limits.local_max_seconds,
            EngineKind::Scribe => settings.limits.scribe_max_seconds,
        };
        (settings.engine, cap)
    };

    // 0. Resolve / load the engine first. Building the engine is a one-time
    //    cost per kind; subsequent presses reuse the cached `Arc<dyn Engine>`
    //    from `AppState` (invalidated automatically when the kind changes).
    let engine = ensure_engine_loaded(app).await.context("load engine")?;
    let _ = engine_kind; // recorded above; engine loaded by kind

    // 1. Audio next — if the mic is denied, fail before the pill flashes.
    // Read the configured input device from settings (falls back to default if not set or not found).
    let input_device_id = {
        let store = app.state::<SettingsStore>();
        store.get().audio.input_device_id.clone()
    };
    let (capture_session, mut consumer, level_rx, effective_rate) =
        capture::start_with_device(input_device_id.as_deref()).context("start audio capture")?;

    // 2. Show the pill (must hop to the macOS main thread).
    show_pill_async(app);

    // 2a. Tray → Listening + optional start sound.
    if let Err(e) = tray::set_state(app, TrayState::Listening) {
        warn!(?e, "tray set_state Listening failed");
    }
    {
        let store = app.state::<SettingsStore>();
        if store.get().sounds.start_enabled {
            sounds::play_start(app);
        }
    }

    // 3. Tell the frontend we've begun. Include the cap so the pill can
    //    display `mm:ss / cap` and compute the 80% warning threshold.
    //
    //    Read mode from settings to emit the correct trigger string.
    let trigger = app
        .try_state::<SettingsStore>()
        .map(|s| match s.get().hotkey.mode {
            crate::settings::schema::HotkeyMode::Hold => "hold",
            crate::settings::schema::HotkeyMode::Toggle => "toggle",
        })
        .unwrap_or("hold");

    if let Err(e) = app.emit(
        EV_DICTATION_START,
        DictationStartPayload {
            trigger,
            cap_seconds,
        },
    ) {
        warn!(?e, "emit dictation:start failed");
    }

    // 4a. Spawn the 30 Hz level pump (drives the pill waveform).
    let app_for_levels = app.clone();
    // Clone level_rx before moving into level_pump so the VAD task can also
    // observe RMS values independently. Both clones see every published value.
    let vad_level_rx = level_rx.clone();
    let level_pump = tokio::spawn(level_pump(app_for_levels, level_rx));

    // 4b. Spawn the VAD task. Watches RMS from the same watch channel and calls
    //     `audio_stop.cancel()` when sustained silence is detected. This
    //     triggers the same clean teardown path as the user releasing the
    //     hotkey: audio_pump drains + drops audio_tx → engine sees EOF →
    //     finalises and emits is_final=true.
    //
    //     If the user releases the hotkey before VAD fires, `stop_dictation`
    //     aborts the task (idempotent: audio_stop is already cancelled or will
    //     be cancelled by stop_dictation's own path).

    // 5. Build the engine pipeline: audio_pump → mpsc → engine.stream → emit.
    let cancel = CancellationToken::new();
    let audio_stop = CancellationToken::new();
    let (audio_tx, audio_rx) = mpsc::channel::<AudioChunk>(ENGINE_CHANNEL_CAPACITY);

    // 4c. Engine-aware cap timer. Fires `audio_stop` after `cap_seconds` —
    //     same path as the VAD early-stop and the user releasing the hotkey.
    //     Uses `tokio::select!` against `audio_stop` so it exits immediately
    //     when dictation already ended normally.
    let audio_stop_for_cap = audio_stop.clone();
    let app_for_cap = app.clone();
    let cap_task = tokio::spawn(async move {
        let cap_dur = std::time::Duration::from_secs(u64::from(cap_seconds));
        tokio::select! {
            _ = tokio::time::sleep(cap_dur) => {
                // Emit the warning event, then trigger clean stop.
                if let Err(e) = app_for_cap.emit(
                    EV_DICTATION_CAP_REACHED,
                    CapReachedPayload { cap_seconds },
                ) {
                    debug!(?e, "emit dictation:cap_reached failed");
                }
                info!(cap_seconds, "cap reached — triggering auto-stop");
                audio_stop_for_cap.cancel();
                request_auto_stop(&app_for_cap);
            }
            _ = audio_stop_for_cap.cancelled() => {
                // Dictation ended before the cap — nothing to do.
            }
        }
    });

    let audio_stop_for_vad = audio_stop.clone();
    let app_for_vad = app.clone();
    let vad_task = tokio::spawn(async move {
        let mut detector = SilenceDetector::new(VadConfig::default());
        let mut rx = vad_level_rx;
        // `changed()` returns Err when the sender (capture) is dropped —
        // means audio stopped, so we exit cleanly.
        while rx.changed().await.is_ok() {
            let rms = *rx.borrow_and_update();
            let event = detector.observe(rms, std::time::Instant::now());
            if event == crate::audio::vad::SilenceEvent::SustainedSilence {
                debug!("VAD: sustained silence detected — triggering early stop");
                audio_stop_for_vad.cancel();
                request_auto_stop(&app_for_vad);
                return;
            }
        }
    });

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
                    drain_consumer(&mut consumer, &mut scratch, &audio_tx, effective_rate).await;
                    return;
                }
                _ = ticker.tick() => {
                    drain_consumer(&mut consumer, &mut scratch, &audio_tx, effective_rate).await;
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
                            // TODO(integrator): pass the pid captured at dictation start.
                            match inject::paste(&p.text, None).await {
                                Ok(outcome) => {
                                    // Play stop sound on successful injection.
                                    {
                                        let store = app_for_engine.try_state::<SettingsStore>();
                                        if store
                                            .as_ref()
                                            .map(|s| s.get().sounds.stop_enabled)
                                            .unwrap_or(false)
                                        {
                                            sounds::play_stop(&app_for_engine);
                                        }
                                    }
                                    if let Err(e) = app_for_engine.emit(
                                        EV_DICTATION_INJECTED,
                                        InjectedPayload {
                                            text: p.text,
                                            restored_clipboard: outcome.restored_clipboard,
                                        },
                                    ) {
                                        warn!(?e, "emit dictation:injected failed");
                                    }
                                }
                                Err(e) => {
                                    warn!(?e, "inject::paste failed");
                                    if let Err(te) =
                                        tray::set_state(&app_for_engine, TrayState::Error)
                                    {
                                        warn!(?te, "tray set_state Error (inject fail) failed");
                                    }
                                    let payload = ErrorPayload {
                                        code: e.code(),
                                        message: e.to_string(),
                                        recovery_hint: e.recovery_hint().map(Into::into),
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
                    if let Err(te) = tray::set_state(&app_for_engine, TrayState::Error) {
                        warn!(?te, "tray set_state Error (engine error) failed");
                    }
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
        vad_task,
        cap_task,
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
    sample_rate: u32,
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
            sample_rate,
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
        vad_task,
        cap_task,
        audio_stop,
        cancel,
    } = session;
    let gen = pill_gen();

    // 1. Stop the mic. The audio_pump's `audio_stop` will catch the residual
    //    ringbuf contents on its way out.
    drop(capture);

    // 2. Stop the level event emitter, the VAD task, and the cap timer —
    //    all abort gracefully. The cap timer also exits via `audio_stop`
    //    cancellation (select! arm), but abort is the fast path on normal stop.
    level_pump.abort();
    vad_task.abort();
    cap_task.abort();

    // Tell the pill we're now transcribing (no more level events will come).
    if let Err(e) = app.emit(EV_DICTATION_STOP, ()) {
        debug!(?e, "emit dictation:stop failed");
    }

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

    // Fade the pill out, then hide the window (skipped if a new dictation
    // has already re-shown the pill).
    fade_and_hide_pill(app, gen).await;

    // Revert tray to Idle. The error auto-revert path already does this for
    // error states; this covers the normal stop path.
    if let Err(e) = tray::set_state(app, TrayState::Idle) {
        warn!(?e, "tray set_state Idle (stop) failed");
    }
}

/// Pump RMS values from the watch channel out as `dictation:audio_level`
/// events at most `LEVEL_EVENT_HZ` times per second.
async fn level_pump<R: Runtime>(app: AppHandle<R>, mut rx: tokio::sync::watch::Receiver<f32>) {
    let interval = Duration::from_millis(1000 / LEVEL_EVENT_HZ);
    let mut last_emit = Instant::now() - interval;

    // Peak-hold across the throttle window: the cpal callback updates ~every
    // 10 ms, but we emit every ~33 ms. Sending only the *latest* value threw
    // away 2 of 3 readings — including syllable peaks — so the waveform
    // jittered instead of tracking the voice.
    let mut peak = 0.0_f32;

    // `changed()` returns Err when the sender is dropped (capture stopped).
    while rx.changed().await.is_ok() {
        peak = peak.max(*rx.borrow_and_update());
        let now = Instant::now();
        if now.duration_since(last_emit) < interval {
            continue;
        }
        last_emit = now;
        let level = std::mem::take(&mut peak);
        if let Err(e) = app.emit(EV_DICTATION_AUDIO_LEVEL, AudioLevelPayload { rms: level }) {
            debug!(?e, "emit audio_level failed");
        }
    }
}

/// Load the engine for the current settings, using `AppState` as a cache.
///
/// Cache key is `EngineKind`: if the user switches local→scribe in Settings
/// the old entry is evicted and the new engine is built fresh. On build
/// failure we reset the cache to `None` so the next dictation retries.
///
/// After a successful build we run `health_check()` and emit `engine:status`
/// so the Settings UI can show a green/red dot without blocking dictation.
async fn ensure_engine_loaded<R: Runtime>(app: &AppHandle<R>) -> Result<Arc<dyn Engine>> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| anyhow!("AppState missing from Tauri state"))?;
    let settings_store = app
        .try_state::<SettingsStore>()
        .ok_or_else(|| anyhow!("SettingsStore missing from Tauri state"))?;

    // Read settings without holding the lock across any await.
    let settings = settings_store.get();
    let kind = settings.engine;

    if let Some(engine) = state.engine_for_kind(kind) {
        return Ok(engine);
    }

    // Build fresh — this is the expensive path (model file I/O or first
    // keychain lookup). Errors surface as `dictation:error` to the frontend.
    let engine: Arc<dyn Engine> = factory::build(app, &settings).map_err(|e| {
        state.clear_engine();
        anyhow::Error::from(e)
    })?;

    state.set_engine(kind, engine.clone());
    info!(?kind, "engine built and cached");

    // Health-check is best-effort: we don't fail the dictation on a bad
    // health result, just surface the status to the frontend.
    let engine_for_health = engine.clone();
    let app_for_health = app.clone();
    let kind_str = format!("{kind:?}").to_lowercase();
    tokio::spawn(async move {
        let (healthy, last_error) = match engine_for_health.health_check().await {
            Ok(()) => (true, None),
            Err(e) => (false, Some(e.to_string())),
        };
        let payload = EngineStatusPayload {
            engine: kind_str,
            healthy,
            last_error,
        };
        if let Err(e) = app_for_health.emit(EV_ENGINE_STATUS, payload) {
            warn!(?e, "emit engine:status failed");
        }
    });

    Ok(engine)
}

fn emit_error<R: Runtime>(app: &AppHandle<R>, err: &EngineError) {
    if let Err(e) = tray::set_state(app, TrayState::Error) {
        warn!(?e, "tray set_state Error failed");
    }
    let code: &'static str = match err {
        EngineError::MissingApiKey => "missing_api_key",
        EngineError::ModelNotInstalled(_) => "model_not_installed",
        EngineError::Audio(_) => "audio",
        EngineError::Inference(_) => "inference",
        EngineError::Cancelled => "cancelled",
        EngineError::Io(_) => "io",
        EngineError::WebSocket(_) => "websocket",
    };
    let recovery_hint: Option<String> = match err {
        EngineError::MissingApiKey => {
            Some("open settings → engine to add your elevenlabs key".into())
        }
        _ => None,
    };
    let payload = ErrorPayload {
        code,
        message: err.to_string(),
        recovery_hint,
    };
    if let Err(e) = app.emit(EV_DICTATION_ERROR, payload) {
        warn!(?e, "emit dictation:error failed");
    }
}

/// Bumped every time the pill is shown. Delayed hides capture the value and
/// skip if it changed — otherwise a quick re-press inside the 250 ms fade
/// window would hide the *new* dictation's pill.
static PILL_GEN: AtomicU64 = AtomicU64::new(0);

fn pill_gen() -> u64 {
    PILL_GEN.load(Ordering::SeqCst)
}

/// Fade the pill out (frontend) then hide the window — unless a newer
/// dictation has shown the pill in the meantime.
async fn fade_and_hide_pill<R: Runtime>(app: &AppHandle<R>, gen: u64) {
    if pill_gen() != gen {
        return;
    }
    if let Err(e) = app.emit(EV_DICTATION_END, ()) {
        debug!(?e, "emit dictation:end failed");
    }
    tokio::time::sleep(PILL_HIDE_DELAY).await;
    if pill_gen() == gen {
        hide_pill_async(app);
    }
}

/// Route a VAD / cap auto-stop through the controller so the full stop path
/// runs: mic closes, pill switches to "transcribing", then fades out, and
/// toggle state resets. Previously only `audio_stop` was cancelled, leaving
/// the pill stuck in "listening" with a live mic until the next keypress.
fn request_auto_stop<R: Runtime>(app: &AppHandle<R>) {
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let ctrl = app2
            .try_state::<Arc<crate::app::DictationController>>()
            .map(|s| Arc::clone(s.inner()));
        if let Some(ctrl) = ctrl {
            ctrl.stop(app2.clone()).await;
        }
    });
}

/// Start failed (no API key, mic denied, engine load error…). Briefly show
/// the pill in its error state so the keypress isn't silently ignored.
pub fn flash_start_error<R: Runtime>(app: &AppHandle<R>, err: &anyhow::Error) {
    show_pill_async(app);
    let gen = pill_gen();
    let payload = ErrorPayload {
        code: "start_failed",
        message: format!("{err:#}"),
        recovery_hint: None,
    };
    if let Err(e) = app.emit(EV_DICTATION_ERROR, payload) {
        warn!(?e, "emit dictation:error (start failed) failed");
    }
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        // Pill self-dismisses errors after ~2.2 s; hide the window after.
        tokio::time::sleep(START_ERROR_LINGER).await;
        if pill_gen() == gen {
            hide_pill_async(&app2);
        }
    });
}

fn show_pill_async<R: Runtime>(app: &AppHandle<R>) {
    PILL_GEN.fetch_add(1, Ordering::SeqCst);
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

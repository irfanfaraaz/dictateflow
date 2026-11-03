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
//!
//! # Session lifecycle
//!
//! ```text
//! start_dictation ─► capture + monitor (30 Hz level / VAD / mic health)
//!                 ─► audio pump (ringbuf → resample 16 kHz → mpsc)
//!                 ─► engine pump (engine.stream → paste → events)
//! stop_dictation  ─► mic off, audio pump drained (≤ 1 s), returns
//!                 └► finish_session (spawned): await engine pump (≤ 15 s,
//!                    then cancel + abort), fade / linger, hide pill
//! ```
//!
//! A finishing session never blocks the next dictation, and is never
//! cancelled by it. Its UI effects (tray, pill events) are gated on the pill
//! generation so they can't land on a newer session's pill, and pastes go
//! through an ordered chain + global lock so transcripts land in the order
//! they were spoken.

#![allow(dead_code)] // surfaced by frontend incrementally

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use ringbuf::traits::Consumer;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::app::{AppState, DictationSession};
use crate::audio::capture::{self, CaptureHealth};
use crate::audio::level::LevelMeter;
use crate::audio::resample::Resampler;
use crate::audio::vad::{SilenceDetector, SilenceEvent, VadConfig};
use crate::audio::{AudioError, TARGET_SAMPLE_RATE};
use crate::engine::transcript::clean_transcript;
use crate::engine::{factory, AudioChunk, Engine, EngineError};
use crate::inject::{self, InjectError};
use crate::settings::schema::{EngineKind, HotkeyMode, Settings};
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
/// Emitted with the raw final transcript for downstream consumers (e.g. a
/// future tray history). The pill does not subscribe.
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
    /// Peak root-mean-square amplitude over the last ~33 ms, in [0, ~1].
    /// Unsmoothed; the pill applies its own visual easing.
    pub rms: f32,
}

#[derive(Debug, Serialize, Clone)]
pub struct InjectedPayload {
    pub text: String,
    pub restored_clipboard: bool,
}

/// `message` is rendered inside the pill: keep it short (≤ ~60 chars), human
/// and actionable. Longer guidance goes in `recovery_hint`.
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

// ---------- Tunables ----------

/// Cadence of `dictation:audio_level` emission and of the VAD / mic-health
/// poll. 30 Hz keeps the pill waveform visually smooth while staying well
/// below IPC saturation.
const LEVEL_EVENT_HZ: u64 = 30;

/// Pill fade-out delay after stop. Matches the design spec's ~250 ms cross-
/// fade so the window is gone exactly when the user perceives the pill to
/// have "let go"; it also overlaps the "pasting…" beat the pill renders on
/// `dictation:injected`.
const PILL_HIDE_DELAY: Duration = Duration::from_millis(250);

/// How long an error pill stays up (the pill self-dismisses errors at
/// ~2.2 s; the window is hidden just after).
const START_ERROR_LINGER: Duration = Duration::from_millis(2500);

/// Cadence at which the audio pump moves samples from the cpal ringbuf
/// through the resampler into the engine's mpsc.
const AUDIO_PUMP_INTERVAL: Duration = Duration::from_millis(100);

/// Bounded mpsc capacity between the audio pump and the engine. One chunk
/// per pump tick (~100 ms), so 256 chunks ≈ 25 s — enough to buffer audio
/// while a slow Scribe handshake (≤ 8 s) completes.
const ENGINE_CHANNEL_CAPACITY: usize = 256;

/// How long `stop_dictation` waits for the audio pump's final drain before
/// aborting it (it can only stall if the engine stopped consuming).
const AUDIO_PUMP_EXIT_TIMEOUT: Duration = Duration::from_secs(1);

/// How long a stopped session's engine may take to produce its transcript
/// (and paste) before it is cancelled and aborted.
const TRANSCRIPTION_TIMEOUT: Duration = Duration::from_secs(15);

/// If the mic has delivered only digital silence for this long, report it.
const MIC_SILENT_AFTER: Duration = Duration::from_secs(2);

// ---------- Errors ----------

/// Typed start failure: carries the exact payload to show, so the pill gets
/// the specific code / message / hint (and exactly one error event).
#[derive(Debug, Clone)]
pub struct StartError {
    pub payload: ErrorPayload,
    /// Technical detail for logs.
    pub detail: String,
}

impl StartError {
    fn new(payload: ErrorPayload, detail: impl Into<String>) -> Self {
        Self {
            payload,
            detail: detail.into(),
        }
    }

    fn internal(detail: impl Into<String>) -> Self {
        let detail = detail.into();
        Self::new(
            payload(
                "start_failed",
                "Couldn't start dictation",
                Some("Restart DictateFlow. If it keeps happening, check the log file."),
            ),
            detail,
        )
    }

    fn from_engine(err: &EngineError) -> Self {
        Self::new(engine_error_payload(err), err.to_string())
    }

    fn from_audio(err: &AudioError) -> Self {
        Self::new(audio_error_payload(err), err.to_string())
    }
}

fn payload(code: &'static str, message: &str, hint: Option<&str>) -> ErrorPayload {
    ErrorPayload {
        code,
        message: message.to_owned(),
        recovery_hint: hint.map(str::to_owned),
    }
}

/// Map an engine error to a short pill message + longer hint.
fn engine_error_payload(err: &EngineError) -> ErrorPayload {
    match err {
        EngineError::MissingApiKey => payload(
            "missing_api_key",
            "Add your ElevenLabs API key in Settings",
            Some("Settings → Engine → paste your ElevenLabs API key."),
        ),
        EngineError::ModelNotInstalled(_) => payload(
            "model_not_installed",
            "No speech model installed",
            Some("Download a model in Settings → Models."),
        ),
        EngineError::Audio(a) => audio_error_payload(a),
        EngineError::Inference(d) => ErrorPayload {
            code: "inference",
            message: "Transcription failed".into(),
            recovery_hint: Some(format!("Try again. Details: {d}")),
        },
        EngineError::Cancelled => payload("cancelled", "Dictation cancelled", None),
        EngineError::Io(e) => ErrorPayload {
            code: "io",
            message: "Couldn't read a required file".into(),
            recovery_hint: Some(format!("Re-download the model in Settings → Models. ({e})")),
        },
        EngineError::Connect(d) => ErrorPayload {
            code: "network",
            message: "No internet — couldn't reach ElevenLabs".into(),
            recovery_hint: Some(format!(
                "Check your connection, or switch to the local engine in Settings. ({d})"
            )),
        },
        EngineError::WebSocket(d) => websocket_error_payload(d),
    }
}

fn websocket_error_payload(detail: &str) -> ErrorPayload {
    let lower = detail.to_ascii_lowercase();
    let (code, message, hint): (&'static str, &str, &str) =
        if lower.contains("auth") || lower.contains("401") || lower.contains("403") {
            (
                "auth_failed",
                "ElevenLabs rejected your API key",
                "Check or replace the key in Settings → Engine.",
            )
        } else if lower.contains("quota") {
            (
                "quota_exceeded",
                "ElevenLabs quota used up",
                "Top up your ElevenLabs plan or switch to the local engine.",
            )
        } else if lower.contains("rate") || lower.contains("throttl") || lower.contains("429") {
            (
                "rate_limited",
                "ElevenLabs is busy — try again shortly",
                "Too many requests right now; wait a few seconds.",
            )
        } else if lower.contains("no transcript") {
            (
                "no_transcript",
                "No transcript came back",
                "ElevenLabs didn't return any text. Try again.",
            )
        } else {
            (
                "websocket",
                "Lost connection to ElevenLabs",
                "Check your connection and try again.",
            )
        };
    ErrorPayload {
        code,
        message: message.to_owned(),
        recovery_hint: Some(format!("{hint} ({detail})")),
    }
}

fn audio_error_payload(err: &AudioError) -> ErrorPayload {
    match err {
        AudioError::NoDefaultDevice => payload(
            "no_microphone",
            "No microphone found",
            Some("Connect a microphone or choose an input in Settings → Audio."),
        ),
        other => ErrorPayload {
            code: "mic_unavailable",
            message: "Microphone unavailable".into(),
            recovery_hint: Some(format!(
                "Allow DictateFlow in System Settings → Privacy & Security → Microphone, \
                 or pick another input in Settings. ({other})"
            )),
        },
    }
}

/// Map a paste failure to a pill error.
///
/// Map a paste failure to a short pill message. In every case the
/// transcript is left on the clipboard so nothing is lost.
fn inject_error_payload(err: &InjectError) -> ErrorPayload {
    match err {
        InjectError::Clipboard(e) => ErrorPayload {
            code: "inject_failed",
            message: "Couldn't use the clipboard to paste".into(),
            recovery_hint: Some(format!("Try again. ({e})")),
        },
        InjectError::Cgevent(e) => ErrorPayload {
            code: "inject_failed",
            message: "Couldn't paste the transcript".into(),
            recovery_hint: Some(format!(
                "Grant Accessibility in System Settings → Privacy & Security → \
                 Accessibility, then relaunch. ({e})"
            )),
        },
        // Code and hint come from the inject layer, so the two can't drift.
        InjectError::FocusChanged => payload(
            err.code(),
            "Focus changed — transcript copied, press ⌘V to paste",
            err.recovery_hint(),
        ),
        InjectError::AccessibilityDenied => payload(
            err.code(),
            "Re-enable Accessibility to paste — text copied",
            err.recovery_hint(),
        ),
    }
}

fn no_speech_payload() -> ErrorPayload {
    payload(
        "no_speech",
        "Nothing heard",
        Some("Start speaking after the pill appears, and check the input in Settings → Audio."),
    )
}

fn no_transcript_payload() -> ErrorPayload {
    payload(
        "no_transcript",
        "No transcript came back",
        Some("The engine ended without a result. Try again."),
    )
}

fn transcription_timeout_payload() -> ErrorPayload {
    payload(
        "transcription_timeout",
        "Transcription took too long",
        Some("Try a shorter dictation, or switch engines in Settings."),
    )
}

fn mic_silent_payload() -> ErrorPayload {
    payload(
        "mic_silent",
        "Microphone is silent",
        Some(
            "Allow DictateFlow in System Settings → Privacy & Security → Microphone, \
             unmute the input, or pick another one in Settings → Audio.",
        ),
    )
}

fn device_lost_payload() -> ErrorPayload {
    payload(
        "device_lost",
        "Microphone disconnected",
        Some("Reconnect it, or choose another input in Settings → Audio."),
    )
}

// ---------- Per-session context ----------

/// What the session's background tasks need to report back.
struct SessionCtx<R: Runtime> {
    app: AppHandle<R>,
    /// Pill generation of this session.
    gen: u64,
    /// Set whenever this session surfaces a `dictation:error`.
    errored: Arc<AtomicBool>,
}

// Manual impl: `#[derive(Clone)]` would wrongly require `R: Clone`.
impl<R: Runtime> Clone for SessionCtx<R> {
    fn clone(&self) -> Self {
        Self {
            app: self.app.clone(),
            gen: self.gen,
            errored: self.errored.clone(),
        }
    }
}

impl<R: Runtime> SessionCtx<R> {
    /// True while this session still owns the pill (no newer dictation or
    /// start-error flash has re-shown it).
    fn is_current(&self) -> bool {
        pill_gen() == self.gen
    }

    /// Record + surface an error for this session. Suppressed (logged only)
    /// once a newer session owns the pill.
    fn error(&self, payload: ErrorPayload) {
        self.errored.store(true, Ordering::SeqCst);
        if !self.is_current() {
            warn!(
                code = payload.code,
                "suppressing error from a stale dictation session"
            );
            return;
        }
        if let Err(e) = tray::set_state(&self.app, TrayState::Error) {
            warn!(?e, "tray set_state Error failed");
        }
        if let Err(e) = self.app.emit(EV_DICTATION_ERROR, payload) {
            warn!(?e, "emit dictation:error failed");
        }
    }

    fn has_errored(&self) -> bool {
        self.errored.load(Ordering::SeqCst)
    }
}

// ---------- Paste ordering ----------

/// Serialises the clipboard-write + ⌘V + restore sequence across sessions.
static PASTE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Receiver that resolves when the most recently started session's engine
/// pump has finished (pasted or given up). Each new session swaps in its own
/// and waits on the previous one before pasting, so transcripts land in the
/// order they were dictated even if a later engine finishes first.
static PASTE_CHAIN: std::sync::Mutex<Option<oneshot::Receiver<()>>> = std::sync::Mutex::new(None);

/// Join the paste chain: returns the previous session's "done" receiver (if
/// any) and this session's "done" sender (dropping it signals completion).
fn join_paste_chain() -> (Option<oneshot::Receiver<()>>, oneshot::Sender<()>) {
    let (tx, rx) = oneshot::channel();
    let prev = match PASTE_CHAIN.lock() {
        Ok(mut g) => g.replace(rx),
        Err(p) => p.into_inner().replace(rx),
    };
    (prev, tx)
}

/// Stand-in for the upcoming `inject::PasteOutcome`.
///
/// Paste `text` into the app that was frontmost when the dictation started.
/// If focus moved elsewhere meanwhile, the inject layer refuses to paste
/// and leaves the transcript on the clipboard ([`InjectError::FocusChanged`]).
async fn paste_transcript(
    text: &str,
    target_pid: Option<i32>,
) -> Result<inject::PasteOutcome, InjectError> {
    inject::paste(text, target_pid).await
}

// ---------- Start ----------

/// Begin a dictation: load the engine if needed, start capture, show the
/// pill, and spawn the monitor / audio / engine / cap tasks.
///
/// Returns the live session — the caller stores it in the
/// `DictationController` until stop. On failure nothing is emitted here; the
/// caller shows the returned [`StartError`] via [`flash_start_error`] so
/// exactly one `dictation:error` fires.
pub async fn start_dictation<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<DictationSession, StartError> {
    let settings = app
        .try_state::<SettingsStore>()
        .ok_or_else(|| StartError::internal("SettingsStore missing from Tauri state"))?
        .get();

    // Guard: local engine with no installed models. `LocalEngine::load`
    // would fail too, but this gives the specific chip without touching disk.
    if settings.engine == EngineKind::Local && settings.local.installed_models.is_empty() {
        return Err(StartError::from_engine(&EngineError::ModelNotInstalled(
            "no models installed".into(),
        )));
    }
    let cap_seconds = match settings.engine {
        EngineKind::Local => settings.limits.local_max_seconds,
        EngineKind::Scribe => settings.limits.scribe_max_seconds,
    };
    let mode = settings.hotkey.mode;

    // 1. Resolve / load the engine (cached per kind; built off the async
    //    workers).
    let engine = ensure_engine_loaded(app, &settings).await?;

    // 2. Audio next — if the mic is unavailable, fail before the pill flashes.
    //    Opening CoreAudio blocks; `block_in_place` keeps it off the async
    //    scheduler while staying on this thread (cpal's Stream is !Send on
    //    macOS, so it must not hop threads via spawn_blocking).
    let device = settings.audio.input_device_id.clone();
    let cap = tokio::task::block_in_place(|| capture::start_with_device(device.as_deref()))
        .map_err(|e| StartError::from_audio(&e))?;

    // Remember which app we're dictating into. Must run before the pill is
    // shown (the pill never takes focus, but be safe).
    let target_pid = inject::frontmost_app_pid();

    // 3. Show the pill (must hop to the macOS main thread). Its generation
    //    identifies this session from here on.
    show_pill_async(app);
    let gen = pill_gen();

    if let Err(e) = tray::set_state(app, TrayState::Listening) {
        warn!(?e, "tray set_state Listening failed");
    }
    if settings.sounds.start_enabled {
        sounds::play_start(app);
    }

    let trigger = match mode {
        HotkeyMode::Hold => "hold",
        HotkeyMode::Toggle => "toggle",
    };
    if let Err(e) = app.emit(
        EV_DICTATION_START,
        DictationStartPayload {
            trigger,
            cap_seconds,
        },
    ) {
        warn!(?e, "emit dictation:start failed");
    }

    let ctx = SessionCtx {
        app: app.clone(),
        gen,
        errored: Arc::new(AtomicBool::new(false)),
    };
    let cancel = CancellationToken::new();
    let audio_stop = CancellationToken::new();
    let (audio_tx, audio_rx) = mpsc::channel::<AudioChunk>(ENGINE_CHANNEL_CAPACITY);

    // 4a. Monitor: level events, toggle-mode VAD, mic health. Hold mode ends
    //     on key release, so it never auto-stops on silence.
    let vad =
        matches!(mode, HotkeyMode::Toggle).then(|| SilenceDetector::new(VadConfig::default()));
    let monitor = tokio::spawn(monitor_pump(ctx.clone(), cap.level, cap.health, vad));

    // 4b. Engine-aware cap timer — same stop path as VAD / key release.
    let cap_ctx = ctx.clone();
    let audio_stop_for_cap = audio_stop.clone();
    let cap_task = tokio::spawn(async move {
        let cap_dur = Duration::from_secs(u64::from(cap_seconds));
        tokio::select! {
            _ = tokio::time::sleep(cap_dur) => {
                if cap_ctx.is_current() {
                    if let Err(e) = cap_ctx.app.emit(
                        EV_DICTATION_CAP_REACHED,
                        CapReachedPayload { cap_seconds },
                    ) {
                        debug!(?e, "emit dictation:cap_reached failed");
                    }
                }
                info!(cap_seconds, "cap reached — triggering auto-stop");
                request_auto_stop(&cap_ctx.app, cap_ctx.gen);
            }
            _ = audio_stop_for_cap.cancelled() => {}
        }
    });

    // 4c. Audio pump: ringbuf → 16 kHz resampler → engine mpsc.
    let audio_pump = tokio::spawn(run_audio_pump(
        cap.consumer,
        cap.sample_rate,
        audio_tx,
        audio_stop.clone(),
    ));

    // 4d. Engine pump.
    let (prev_paste, paste_done) = join_paste_chain();
    let engine_pump = tokio::spawn(run_engine_pump(
        ctx.clone(),
        engine,
        audio_rx,
        cancel.clone(),
        target_pid,
        prev_paste,
        paste_done,
    ));

    Ok(DictationSession {
        capture: cap.session,
        monitor,
        audio_pump,
        engine_pump,
        cap_task,
        audio_stop,
        cancel,
        gen,
        errored: ctx.errored,
    })
}

// ---------- Audio pump ----------

/// Move audio from the cpal ringbuf to the engine every
/// `AUDIO_PUMP_INTERVAL`, resampled to 16 kHz. On `audio_stop` it does one
/// final drain and returns, dropping `audio_tx` (engine sees end-of-stream).
///
/// Sends use `reserve()` raced against `audio_stop`, so a full channel (an
/// engine that hasn't started consuming) can never keep the pump from
/// noticing the stop; the pending chunk is folded into the final send.
async fn run_audio_pump(
    mut consumer: ringbuf::HeapCons<f32>,
    in_rate: u32,
    audio_tx: mpsc::Sender<AudioChunk>,
    audio_stop: CancellationToken,
) {
    let mut resampler = Resampler::new(in_rate, TARGET_SAMPLE_RATE);
    let mut scratch = vec![0.0_f32; 4096];
    let mut ticker = tokio::time::interval(AUDIO_PUMP_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut pending: Vec<f32> = Vec::new();

    loop {
        tokio::select! {
            biased;
            _ = audio_stop.cancelled() => break,
            _ = ticker.tick() => {
                let samples = drain_resampled(&mut consumer, &mut scratch, &mut resampler);
                if samples.is_empty() {
                    continue;
                }
                tokio::select! {
                    biased;
                    permit = audio_tx.reserve() => match permit {
                        Ok(p) => p.send(AudioChunk { samples, sample_rate: TARGET_SAMPLE_RATE }),
                        // Engine receiver gone — nothing left to feed.
                        Err(_) => return,
                    },
                    _ = audio_stop.cancelled() => {
                        pending = samples;
                        break;
                    }
                }
            }
        }
    }

    pending.extend(drain_resampled(&mut consumer, &mut scratch, &mut resampler));
    if !pending.is_empty() {
        // Bounded by stop_dictation's AUDIO_PUMP_EXIT_TIMEOUT.
        let _ = audio_tx
            .send(AudioChunk {
                samples: pending,
                sample_rate: TARGET_SAMPLE_RATE,
            })
            .await;
    }
}

/// Pop everything currently in the ringbuf and resample it.
fn drain_resampled(
    consumer: &mut ringbuf::HeapCons<f32>,
    scratch: &mut [f32],
    resampler: &mut Resampler,
) -> Vec<f32> {
    let mut out = Vec::new();
    loop {
        let popped = consumer.pop_slice(scratch);
        if popped == 0 {
            break;
        }
        resampler.process(&scratch[..popped], &mut out);
    }
    out
}

// ---------- Engine pump ----------

/// Drive the engine's partial stream to a final transcript and paste it.
///
/// - Tracks the latest interim text; if the stream ends (or errors) without
///   a final result, that text is pasted instead of being lost.
/// - Errors request an auto-stop so the mic doesn't stay live.
/// - `paste_done` is dropped on return, releasing the next session's paste.
#[allow(clippy::too_many_arguments)]
async fn run_engine_pump<R: Runtime>(
    ctx: SessionCtx<R>,
    engine: Arc<dyn Engine>,
    audio_rx: mpsc::Receiver<AudioChunk>,
    cancel: CancellationToken,
    target_pid: Option<i32>,
    prev_paste: Option<oneshot::Receiver<()>>,
    paste_done: oneshot::Sender<()>,
) {
    let _paste_done = paste_done;

    let mut stream = match engine.stream(audio_rx, cancel).await {
        Ok(s) => s,
        Err(EngineError::Cancelled) => {
            info!("engine.stream() cancelled before start");
            return;
        }
        Err(e) => {
            warn!(?e, "engine.stream() failed");
            ctx.error(engine_error_payload(&e));
            request_auto_stop(&ctx.app, ctx.gen);
            return;
        }
    };

    let mut final_text: Option<String> = None;
    let mut last_partial = String::new();
    let mut last_err: Option<EngineError> = None;

    while let Some(item) = stream.next().await {
        match item {
            Ok(p) if p.is_final => {
                debug!(text_len = p.text.len(), "engine final");
                final_text = Some(p.text);
                break;
            }
            Ok(p) => {
                last_partial = p.text;
            }
            Err(EngineError::Cancelled) => {
                info!("dictation cancelled");
                return;
            }
            Err(e) => {
                warn!(?e, "engine yielded error");
                // Stop the mic now; the recording can't be transcribed.
                request_auto_stop(&ctx.app, ctx.gen);
                last_err = Some(e);
                break;
            }
        }
    }
    drop(stream);

    let text = match final_text {
        Some(t) => {
            if let Err(e) = ctx
                .app
                .emit(EV_DICTATION_FINAL, FinalPayload { text: t.clone() })
            {
                warn!(?e, "emit dictation:final failed");
            }
            t
        }
        None if !last_partial.trim().is_empty() => {
            warn!(
                had_error = last_err.is_some(),
                "engine ended without a final transcript — pasting last interim text"
            );
            last_partial
        }
        None => {
            match last_err {
                Some(e) => ctx.error(engine_error_payload(&e)),
                None => {
                    warn!("engine stream ended without a final transcript");
                    ctx.error(no_transcript_payload());
                }
            }
            return;
        }
    };

    deliver_transcript(&ctx, &text, target_pid, prev_paste).await;
}

/// Filter non-speech, wait for earlier sessions' pastes, then paste.
async fn deliver_transcript<R: Runtime>(
    ctx: &SessionCtx<R>,
    raw: &str,
    target_pid: Option<i32>,
    prev_paste: Option<oneshot::Receiver<()>>,
) {
    let Some(text) = clean_transcript(raw) else {
        info!("skipping injection: no speech in transcript");
        // A mic failure already explained why nothing was heard.
        if !ctx.has_errored() {
            ctx.error(no_speech_payload());
        }
        return;
    };

    if let Some(prev) = prev_paste {
        // Resolves (Err) when the previous session's engine pump finishes.
        let _ = prev.await;
    }
    let _paste_guard = PASTE_LOCK.lock().await;

    match paste_transcript(&text, target_pid).await {
        Ok(outcome) => {
            if !outcome.pasted {
                debug!("paste reported not pasted");
            }
            let play_sound = ctx
                .app
                .try_state::<SettingsStore>()
                .map(|s| s.get().sounds.stop_enabled)
                .unwrap_or(false);
            if play_sound {
                sounds::play_stop(&ctx.app);
            }
            if ctx.is_current() {
                if let Err(e) = ctx.app.emit(
                    EV_DICTATION_INJECTED,
                    InjectedPayload {
                        text,
                        restored_clipboard: outcome.restored_clipboard,
                    },
                ) {
                    warn!(?e, "emit dictation:injected failed");
                }
            }
        }
        Err(e) => {
            warn!(?e, "paste failed");
            ctx.error(inject_error_payload(&e));
        }
    }
}

// ---------- Stop ----------

/// End an in-flight dictation: stop capture and drain the audio pump, then
/// hand the rest of the teardown (awaiting the transcript, hiding the pill)
/// to a background task so the next dictation can start immediately.
pub async fn stop_dictation<R: Runtime>(app: &AppHandle<R>, session: DictationSession) {
    let DictationSession {
        capture,
        monitor,
        mut audio_pump,
        engine_pump,
        cap_task,
        audio_stop,
        cancel,
        gen,
        errored,
    } = session;

    // 1. Stop the mic and the per-session timers.
    drop(capture);
    monitor.abort();
    cap_task.abort();

    // Tell the pill we're now transcribing (no more level events will come).
    if pill_gen() == gen {
        if let Err(e) = app.emit(EV_DICTATION_STOP, ()) {
            debug!(?e, "emit dictation:stop failed");
        }
    }

    // 2. Drain residual samples into the engine and close its input. Bounded:
    //    if the engine isn't consuming, abort the pump (dropping audio_tx
    //    still closes the channel).
    audio_stop.cancel();
    match tokio::time::timeout(AUDIO_PUMP_EXIT_TIMEOUT, &mut audio_pump).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => warn!(?e, "audio pump did not exit cleanly"),
        Err(_) => {
            warn!("audio pump stuck on final drain — aborting");
            audio_pump.abort();
        }
    }

    // 3. Await the transcript in the background.
    tauri::async_runtime::spawn(finish_session(
        app.clone(),
        gen,
        engine_pump,
        cancel,
        errored,
    ));
}

/// Wait (bounded) for the engine pump, then fade or linger and hide the pill.
async fn finish_session<R: Runtime>(
    app: AppHandle<R>,
    gen: u64,
    mut engine_pump: tokio::task::JoinHandle<()>,
    cancel: CancellationToken,
    errored: Arc<AtomicBool>,
) {
    let ctx = SessionCtx { app, gen, errored };

    match tokio::time::timeout(TRANSCRIPTION_TIMEOUT, &mut engine_pump).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => warn!(?e, "engine pump did not exit cleanly"),
        Err(_) => {
            warn!(
                timeout_s = TRANSCRIPTION_TIMEOUT.as_secs(),
                "engine pump timed out — cancelling"
            );
            // Abort so a late result can never paste minutes from now.
            cancel.cancel();
            engine_pump.abort();
            ctx.error(transcription_timeout_payload());
        }
    }

    if ctx.has_errored() {
        // Let the pill show its error (self-dismisses at ~2.2 s) instead of
        // fading it out after 250 ms. The tray reverts from Error itself.
        if !ctx.is_current() {
            return;
        }
        tokio::time::sleep(START_ERROR_LINGER).await;
        if ctx.is_current() {
            hide_pill_async(&ctx.app);
        }
        return;
    }

    // `dictation:injected` was emitted by the engine pump once the paste
    // landed. Fade the pill out, then hide the window (skipped if a new
    // dictation has re-shown the pill).
    fade_and_hide_pill(&ctx.app, gen).await;

    if ctx.is_current() {
        if let Err(e) = tray::set_state(&ctx.app, TrayState::Idle) {
            warn!(?e, "tray set_state Idle (stop) failed");
        }
    }
}

// ---------- Monitor ----------

/// 30 Hz poller: forwards the peak RMS as `dictation:audio_level`, runs the
/// (toggle-mode) VAD, and turns mic failures into errors + auto-stop.
async fn monitor_pump<R: Runtime>(
    ctx: SessionCtx<R>,
    level: Arc<LevelMeter>,
    health: Arc<CaptureHealth>,
    mut vad: Option<SilenceDetector>,
) {
    let mut ticker = tokio::time::interval(Duration::from_millis(1000 / LEVEL_EVENT_HZ));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let started = Instant::now();

    loop {
        ticker.tick().await;
        // Peak since the previous tick (see LevelMeter) — keeps syllable
        // peaks that land between ticks.
        let rms = level.take();
        if let Err(e) = ctx
            .app
            .emit(EV_DICTATION_AUDIO_LEVEL, AudioLevelPayload { rms })
        {
            debug!(?e, "emit audio_level failed");
        }

        if health.device_lost() {
            warn!("input device lost mid-dictation — stopping");
            ctx.error(device_lost_payload());
            request_auto_stop(&ctx.app, ctx.gen);
            return;
        }
        if !health.saw_signal() && started.elapsed() >= MIC_SILENT_AFTER {
            warn!(
                "audio capture: only digital silence after {} s — mic permission denied, \
                 device muted, or a silent virtual device",
                MIC_SILENT_AFTER.as_secs()
            );
            ctx.error(mic_silent_payload());
            request_auto_stop(&ctx.app, ctx.gen);
            return;
        }

        if let Some(detector) = vad.as_mut() {
            if detector.observe(rms, Instant::now()) == SilenceEvent::SustainedSilence {
                debug!("VAD: sustained silence detected — triggering early stop");
                request_auto_stop(&ctx.app, ctx.gen);
                return;
            }
        }
    }
}

// ---------- Engine cache ----------

/// Load the engine for the current settings, using `AppState` as a cache.
///
/// Cache key is `EngineKind`: if the user switches local→scribe in Settings
/// the old entry is evicted and the new engine is built fresh. On build
/// failure we reset the cache to `None` so the next dictation retries.
///
/// The build (model load / Keychain read) runs on the blocking pool. After a
/// successful build we run `health_check()` and emit `engine:status` so the
/// Settings UI can show a green/red dot without blocking dictation.
async fn ensure_engine_loaded<R: Runtime>(
    app: &AppHandle<R>,
    settings: &Settings,
) -> Result<Arc<dyn Engine>, StartError> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| StartError::internal("AppState missing from Tauri state"))?;
    let kind = settings.engine;

    if let Some(engine) = state.engine_for_kind(kind) {
        return Ok(engine);
    }

    let app_for_build = app.clone();
    let settings_for_build = settings.clone();
    let built =
        tokio::task::spawn_blocking(move || factory::build(&app_for_build, &settings_for_build))
            .await;
    let engine: Arc<dyn Engine> = match built {
        Ok(Ok(engine)) => engine,
        Ok(Err(e)) => {
            state.clear_engine();
            return Err(StartError::from_engine(&e));
        }
        Err(e) => {
            state.clear_engine();
            return Err(StartError::internal(format!(
                "engine build task failed: {e}"
            )));
        }
    };

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

// ---------- Pill helpers ----------

/// Bumped every time the pill is shown. Delayed hides and session effects
/// capture the value and skip if it changed — otherwise a quick re-press
/// would let the old session hide or overwrite the *new* dictation's pill.
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

/// Route a VAD / cap / error auto-stop through the controller so the full
/// stop path runs: mic closes, pill switches to "transcribing", then fades
/// out. `gen` scopes the request to the session that raised it, so a late
/// request can't stop a newer dictation.
fn request_auto_stop<R: Runtime>(app: &AppHandle<R>, gen: u64) {
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let ctrl = app2
            .try_state::<Arc<crate::app::DictationController>>()
            .map(|s| Arc::clone(s.inner()));
        if let Some(ctrl) = ctrl {
            ctrl.stop_session(&app2, gen).await;
        }
    });
}

/// Start failed (no API key, mic denied, engine load error…). Briefly show
/// the pill in its error state so the keypress isn't silently ignored.
pub fn flash_start_error<R: Runtime>(app: &AppHandle<R>, err: &StartError) {
    show_pill_async(app);
    let gen = pill_gen();
    if let Err(e) = tray::set_state(app, TrayState::Error) {
        warn!(?e, "tray set_state Error (start failed) failed");
    }
    if let Err(e) = app.emit(EV_DICTATION_ERROR, err.payload.clone()) {
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
    use super::*;

    #[test]
    fn level_event_cadence_under_60hz() {
        // 30 Hz target → 33 ms interval; sanity-check the constant.
        let interval_ms = 1000 / LEVEL_EVENT_HZ;
        assert!((30..=40).contains(&interval_ms));
    }

    #[test]
    fn engine_channel_capacity_reasonable() {
        // One ~100 ms chunk per pump tick; must cover the Scribe connect
        // timeout (8 s) without blocking the pump.
        assert!((16..=256).contains(&ENGINE_CHANNEL_CAPACITY));
        assert!(ENGINE_CHANNEL_CAPACITY as u64 * AUDIO_PUMP_INTERVAL.as_millis() as u64 >= 8_000);
    }

    fn all_payloads() -> Vec<ErrorPayload> {
        let mut v = vec![
            engine_error_payload(&EngineError::MissingApiKey),
            engine_error_payload(&EngineError::ModelNotInstalled("x".into())),
            engine_error_payload(&EngineError::Audio(AudioError::NoDefaultDevice)),
            engine_error_payload(&EngineError::Audio(AudioError::BuildStream("x".into()))),
            engine_error_payload(&EngineError::Inference("x".into())),
            engine_error_payload(&EngineError::Cancelled),
            engine_error_payload(&EngineError::Io(std::io::Error::other("x"))),
            engine_error_payload(&EngineError::Connect("dns".into())),
            inject_error_payload(&InjectError::Cgevent("x".into())),
            no_speech_payload(),
            no_transcript_payload(),
            transcription_timeout_payload(),
            mic_silent_payload(),
            device_lost_payload(),
            StartError::internal("x").payload,
        ];
        for d in [
            "scribe_auth_error: bad key",
            "auth rejected (HTTP 401)",
            "scribe_quota_exceeded_error: x",
            "scribe_rate_limited_error: x",
            "no transcript received from ElevenLabs",
            "connection closed before final transcript (code 1006 )",
        ] {
            v.push(engine_error_payload(&EngineError::WebSocket(d.into())));
        }
        v
    }

    #[test]
    fn error_messages_are_short_for_the_pill() {
        for p in all_payloads() {
            let n = p.message.chars().count();
            assert!(n <= 60, "{}: message too long ({n}): {}", p.code, p.message);
            assert!(!p.message.is_empty(), "{}: empty message", p.code);
        }
    }

    #[test]
    fn websocket_errors_are_classified() {
        let code = |d: &str| engine_error_payload(&EngineError::WebSocket(d.into())).code;
        assert_eq!(code("scribe_auth_error: invalid"), "auth_failed");
        assert_eq!(code("handshake failed (HTTP 403)"), "auth_failed");
        assert_eq!(code("scribe_quota_exceeded_error: x"), "quota_exceeded");
        assert_eq!(code("scribe_throttled_error: x"), "rate_limited");
        assert_eq!(code("connection reset"), "websocket");
        assert_eq!(
            engine_error_payload(&EngineError::Connect("timed out".into())).code,
            "network"
        );
    }

    #[test]
    fn paste_chain_orders_sessions() {
        // Second session waits on the first one's sender.
        let (_prev0, done1) = join_paste_chain();
        let (prev1, done2) = join_paste_chain();
        let mut prev1 = prev1.expect("second session sees the first");
        assert!(prev1.try_recv().is_err()); // first still running
        drop(done1);
        assert!(matches!(
            prev1.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ));
        drop(done2);
    }
}

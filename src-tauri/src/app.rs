//! AppState + DictationController.
//!
//! `AppState` is the shared container Tauri threads through `tauri::State`.
//! `DictationController` owns the start/stop state machine for a dictation.
//! Hotkey events reach it through the single dispatcher task in
//! `hotkey::listener`; VAD / cap / error auto-stops reach it through
//! `events::request_auto_stop`.
//!
//! Every transition (start, stop, toggle) holds an async `transition` mutex
//! for its whole duration, so a stop that arrives while `start_dictation` is
//! still building the session waits for the session to be stored and then
//! stops it — the mic can never be left live by a lost stop. The inner
//! `Mutex<Option<Session>>` is a plain std mutex that is only locked for a
//! `take()` / `replace()` and never across an `.await`.

#![allow(dead_code)] // wired into commands as later phases land

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Runtime};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::audio::capture::CaptureSession;
use crate::engine::Engine;
use crate::settings::schema::EngineKind;

/// Lazily-initialised in `lib::run::setup`.
///
/// Holds the last-built engine keyed by its `EngineKind` so that switching
/// local→scribe (or vice-versa) in Settings invalidates the cache on the
/// next dictation without requiring an app restart.
pub struct AppState {
    /// `(kind, engine)` — None if no engine has been built yet or the last
    /// build failed. On failure we reset to None so the next F5 press retries.
    engine: Mutex<Option<(EngineKind, Arc<dyn Engine>)>>,
    /// Active download cancellation tokens keyed by model id.
    ///
    /// Inserted BEFORE spawning the download task; removed on completion or
    /// error. `cancel_model_download` looks up the token here and calls
    /// `.cancel()` so the streaming loop exits cleanly.
    pub download_tokens: Mutex<HashMap<String, CancellationToken>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            engine: Mutex::new(None),
            download_tokens: Mutex::new(HashMap::new()),
        }
    }
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the cached engine iff its `EngineKind` matches `kind`.
    /// A mismatched kind means the user switched engines in Settings; we
    /// return `None` so `ensure_engine_loaded` rebuilds.
    pub fn engine_for_kind(&self, kind: EngineKind) -> Option<Arc<dyn Engine>> {
        match self.engine.lock() {
            Ok(g) => g
                .as_ref()
                .and_then(|(k, e)| (*k == kind).then(|| e.clone())),
            Err(p) => p
                .into_inner()
                .as_ref()
                .and_then(|(k, e)| (*k == kind).then(|| e.clone())),
        }
    }

    /// Install the engine handle, tagged with its kind.
    pub fn set_engine(&self, kind: EngineKind, engine: Arc<dyn Engine>) {
        let mut guard = match self.engine.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        *guard = Some((kind, engine));
    }

    /// Forget the cached engine (used when load fails so the next dictation
    /// retries, or when the API key is rotated/cleared).
    pub fn clear_engine(&self) {
        let mut guard = match self.engine.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        *guard = None;
    }
}

/// Holds whatever a single in-flight dictation owns.
///
/// Public to the crate so `events.rs` can construct + destructure it; never
/// expose this beyond the crate boundary.
pub struct DictationSession {
    pub(crate) capture: CaptureSession,
    /// 30 Hz poller: emits `dictation:audio_level`, runs the toggle-mode VAD,
    /// and watches mic health (silent / disconnected device).
    pub(crate) monitor: JoinHandle<()>,
    /// Pumps samples from the cpal ringbuf through the resampler into the
    /// engine's mpsc. Exits when `audio_stop` fires; on exit the captured
    /// `audio_tx` is dropped, which closes the engine's input channel.
    pub(crate) audio_pump: JoinHandle<()>,
    /// Drains the engine's partial stream, pastes the transcript and emits
    /// Tauri events.
    pub(crate) engine_pump: JoinHandle<()>,
    /// Engine-aware cap timer. Requests an auto-stop when the session hits
    /// `local_max_seconds` or `scribe_max_seconds`. Aborted on normal stop so
    /// a short dictation doesn't wait for the full cap.
    pub(crate) cap_task: JoinHandle<()>,
    /// Signals the audio pump to drain residual samples and exit. Distinct
    /// from `cancel` because end-of-dictation is a *clean* stream-close, not
    /// an abort.
    pub(crate) audio_stop: CancellationToken,
    /// Aborts in-flight inference / the WebSocket. Fired only when the
    /// engine misses the stop timeout — a newer dictation never cancels an
    /// older one that is still finishing (its transcript would be lost).
    pub(crate) cancel: CancellationToken,
    /// Pill generation this session showed. Effects of a finishing session
    /// (tray, pill events) are suppressed once a newer session owns the pill.
    pub(crate) gen: u64,
    /// Set by the engine pump / monitor when they surface a `dictation:error`
    /// so the stop path lets the error linger instead of fading immediately.
    pub(crate) errored: Arc<AtomicBool>,
}

/// Coordinates "hotkey down → start" and "hotkey up → stop".
///
/// See the module docs for the locking discipline. Toggle mode needs no
/// separate flag: "is a dictation in flight?" is answered by `inflight`
/// under the transition lock.
pub struct DictationController {
    /// Serialises start / stop / toggle. Held across `.await` on purpose.
    transition: tokio::sync::Mutex<()>,
    inflight: Mutex<Option<DictationSession>>,
}

impl Default for DictationController {
    fn default() -> Self {
        Self {
            transition: tokio::sync::Mutex::new(()),
            inflight: Mutex::new(None),
        }
    }
}

impl DictationController {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn is_active(&self) -> bool {
        match self.inflight.lock() {
            Ok(g) => g.is_some(),
            Err(p) => p.into_inner().is_some(),
        }
    }

    /// Toggle-mode handler: start if idle, stop if a dictation is running.
    pub async fn toggle<R: Runtime>(&self, app: &AppHandle<R>) {
        let _transition = self.transition.lock().await;
        if self.is_active() {
            self.stop_locked(app, None).await;
        } else {
            self.start_locked(app).await;
        }
    }

    /// Begin a dictation. No-op if one is already in flight.
    ///
    /// A previous dictation that is still finishing (transcribing / pasting)
    /// is left alone: it pastes before this one does (see the paste chain in
    /// `events.rs`).
    pub async fn start<R: Runtime>(&self, app: &AppHandle<R>) {
        let _transition = self.transition.lock().await;
        self.start_locked(app).await;
    }

    /// End the in-flight dictation. No-op if none is running.
    pub async fn stop<R: Runtime>(&self, app: &AppHandle<R>) {
        let _transition = self.transition.lock().await;
        self.stop_locked(app, None).await;
    }

    /// Stop the in-flight dictation only if it is the session with pill
    /// generation `gen`. Used by auto-stops (VAD, cap, engine error, mic
    /// failure) so a late request from an old session can't stop a new one.
    pub async fn stop_session<R: Runtime>(&self, app: &AppHandle<R>, gen: u64) {
        let _transition = self.transition.lock().await;
        self.stop_locked(app, Some(gen)).await;
    }

    async fn start_locked<R: Runtime>(&self, app: &AppHandle<R>) {
        if self.is_active() {
            info!("start ignored: dictation already in flight");
            return;
        }
        match crate::events::start_dictation(app).await {
            Ok(session) => {
                let mut guard = match self.inflight.lock() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                *guard = Some(session);
            }
            Err(e) => {
                warn!(code = e.payload.code, detail = %e.detail, "start_dictation failed");
                crate::events::flash_start_error(app, &e);
            }
        }
    }

    async fn stop_locked<R: Runtime>(&self, app: &AppHandle<R>, only_gen: Option<u64>) {
        let session = {
            let mut guard = match self.inflight.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            match only_gen {
                Some(gen) if guard.as_ref().map(|s| s.gen) != Some(gen) => {
                    debug!(gen, "auto-stop ignored: that session already ended");
                    None
                }
                _ => guard.take(),
            }
        };
        if let Some(session) = session {
            crate::events::stop_dictation(app, session).await;
        }
    }
}

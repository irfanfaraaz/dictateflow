//! AppState + DictationController.
//!
//! `AppState` is the shared container Tauri threads through `tauri::State`.
//! `DictationController` owns the cross-task wiring: hotkey down spawns it,
//! hotkey up tells it to stop. Phase 1 wired audio + the pill window;
//! Phase 2 adds the engine pipeline (LocalEngine via whisper-rs).
//!
//! We avoid `Arc<Mutex<…>>` for cross-task state where possible — the
//! controller uses an internal `Mutex<Option<Session>>` only at start/stop
//! transitions, and never holds the lock across `.await`.

#![allow(dead_code)] // wired into commands as later phases land

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Runtime};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

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
    pub(crate) level_pump: JoinHandle<()>,
    /// Pumps samples from the cpal ringbuf into the engine's mpsc.
    /// Exits when `audio_stop` fires; on exit the captured `audio_tx` is
    /// dropped, which closes the engine's input channel.
    pub(crate) audio_pump: JoinHandle<()>,
    /// Drains the engine's partial stream and emits Tauri events.
    pub(crate) engine_pump: JoinHandle<()>,
    /// Watches RMS levels and triggers early-stop on sustained silence (Phase 6).
    /// Calls `audio_stop.cancel()` when `SilenceDetector` fires `SustainedSilence`.
    pub(crate) vad_task: JoinHandle<()>,
    /// Engine-aware cap timer (Phase 8). Calls `audio_stop.cancel()` when the
    /// session hits `local_max_seconds` or `scribe_max_seconds`. Aborted on
    /// normal stop so a short dictation doesn't wait for the full cap.
    pub(crate) cap_task: JoinHandle<()>,
    /// Signals the audio pump to drain residual samples and exit. Distinct
    /// from `cancel` because end-of-dictation is a *clean* stream-close, not
    /// an abort.
    pub(crate) audio_stop: CancellationToken,
    /// Fired only on cancel-on-restart (Phase 9). Aborts in-flight inference.
    pub(crate) cancel: CancellationToken,
}

impl DictationSession {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        capture: CaptureSession,
        level_pump: JoinHandle<()>,
        audio_pump: JoinHandle<()>,
        engine_pump: JoinHandle<()>,
        vad_task: JoinHandle<()>,
        cap_task: JoinHandle<()>,
        audio_stop: CancellationToken,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            capture,
            level_pump,
            audio_pump,
            engine_pump,
            vad_task,
            cap_task,
            audio_stop,
            cancel,
        }
    }
}

/// Coordinates "hotkey down → start" and "hotkey up → stop". Holds the
/// in-flight dictation behind a Mutex; we only ever lock for the duration
/// of a `take()` or `replace()` and never across an await point.
///
/// `toggle_active` tracks whether a toggle-mode dictation is in progress so
/// the second keypress knows to stop rather than start. It mirrors the logical
/// state of `inflight`: set when we successfully start, cleared when we stop.
/// An `AtomicBool` is sufficient — no await involved.
///
/// # Cancel-on-restart (Phase 9)
/// `cancel_token` stores the `CancellationToken` from the most recently
/// started session. When a new hotkey-down arrives while the prior session is
/// still finalizing (i.e., `stop_dictation` is awaiting its `engine_pump`),
/// we fire the token so the old inference task aborts immediately rather than
/// blocking the new dictation. The token is replaced atomically under the same
/// Mutex lock, so there is no window where two live tokens can co-exist.
pub struct DictationController {
    inflight: Mutex<Option<DictationSession>>,
    /// The cancel token of the most recent session, kept alive so a new
    /// start can abort stale finalization.
    last_cancel: Mutex<Option<CancellationToken>>,
    /// Toggle-mode active flag. `true` = dictation is running.
    toggle_active: AtomicBool,
}

impl Default for DictationController {
    fn default() -> Self {
        Self {
            inflight: Mutex::new(None),
            last_cancel: Mutex::new(None),
            toggle_active: AtomicBool::new(false),
        }
    }
}

impl DictationController {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Toggle-mode handler: start if not active, stop if active.
    pub async fn toggle<R: Runtime>(self: Arc<Self>, app: AppHandle<R>) {
        // swap returns the *previous* value; if it was false we should start.
        let was_active = self.toggle_active.swap(true, Ordering::SeqCst);
        if was_active {
            // Already running — stop.
            self.toggle_active.store(false, Ordering::SeqCst);
            Arc::clone(&self).stop(app).await;
        } else {
            // Not running — start.
            Arc::clone(&self).start(app).await;
            // If start failed, reset the flag.
            let guard = match self.inflight.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            if guard.is_none() {
                self.toggle_active.store(false, Ordering::SeqCst);
            }
        }
    }

    /// Begin a dictation. No-op if one is already in flight.
    ///
    /// Cancel-on-restart: if a prior session is still finalizing (i.e., the
    /// inflight slot is empty but `last_cancel` is live because `stop()` took
    /// the session but `stop_dictation` hasn't returned yet), fire the old
    /// cancel token so the stale inference aborts and the new dictation starts
    /// cleanly without risking a double-paste.
    pub async fn start<R: Runtime>(self: Arc<Self>, app: AppHandle<R>) {
        // Grab and fire any stale cancel token from a prior session that is
        // still finalizing. Also check `inflight` in the same lock scope.
        let already_running = {
            let guard = match self.inflight.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            guard.is_some()
        };

        if already_running {
            info!("start ignored: dictation already in flight");
            return;
        }

        // Fire stale cancel token from a prior session that is in teardown.
        {
            let guard = match self.last_cancel.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            if let Some(token) = guard.as_ref() {
                if !token.is_cancelled() {
                    info!("cancel-on-restart: firing stale session cancel token");
                    token.cancel();
                }
            }
        }

        match crate::events::start_dictation(&app).await {
            Ok(session) => {
                // Store cancel token for potential future cancel-on-restart.
                {
                    let mut cancel_guard = match self.last_cancel.lock() {
                        Ok(g) => g,
                        Err(p) => p.into_inner(),
                    };
                    *cancel_guard = Some(session.cancel.clone());
                }
                let mut guard = match self.inflight.lock() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                *guard = Some(session);
            }
            Err(e) => {
                warn!(?e, "start_dictation failed");
            }
        }
    }

    /// End the in-flight dictation. No-op if none is running.
    pub async fn stop<R: Runtime>(self: Arc<Self>, app: AppHandle<R>) {
        let session = {
            let mut guard = match self.inflight.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            guard.take()
        };
        // Clear toggle flag regardless of mode so a mode-switch mid-session
        // doesn't leave toggle_active stale.
        self.toggle_active.store(false, Ordering::SeqCst);
        if let Some(session) = session {
            crate::events::stop_dictation(&app, session).await;
        }
    }
}

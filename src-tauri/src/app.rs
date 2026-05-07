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
#[derive(Default)]
pub struct AppState {
    /// `(kind, engine)` — None if no engine has been built yet or the last
    /// build failed. On failure we reset to None so the next F5 press retries.
    engine: Mutex<Option<(EngineKind, Arc<dyn Engine>)>>,
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
    /// Signals the audio pump to drain residual samples and exit. Distinct
    /// from `cancel` because end-of-dictation is a *clean* stream-close, not
    /// an abort.
    pub(crate) audio_stop: CancellationToken,
    /// Fired only on cancel-on-restart (Phase 9). Aborts in-flight inference.
    pub(crate) cancel: CancellationToken,
}

impl DictationSession {
    pub fn new(
        capture: CaptureSession,
        level_pump: JoinHandle<()>,
        audio_pump: JoinHandle<()>,
        engine_pump: JoinHandle<()>,
        audio_stop: CancellationToken,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            capture,
            level_pump,
            audio_pump,
            engine_pump,
            audio_stop,
            cancel,
        }
    }
}

/// Coordinates "hotkey down → start" and "hotkey up → stop". Holds the
/// in-flight dictation behind a Mutex; we only ever lock for the duration
/// of a `take()` or `replace()` and never across an await point.
#[derive(Default)]
pub struct DictationController {
    inflight: Mutex<Option<DictationSession>>,
}

impl DictationController {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Begin a dictation. No-op if one is already in flight.
    pub async fn start<R: Runtime>(self: Arc<Self>, app: AppHandle<R>) {
        // Quick check: already running? Lock scope is tight; no `.await`
        // inside.
        {
            let guard = match self.inflight.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            if guard.is_some() {
                info!("start ignored: dictation already in flight");
                return;
            }
        }

        match crate::events::start_dictation(&app).await {
            Ok(session) => {
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
        if let Some(session) = session {
            crate::events::stop_dictation(&app, session).await;
        }
    }
}

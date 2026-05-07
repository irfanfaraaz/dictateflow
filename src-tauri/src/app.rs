//! AppState + DictationController.
//!
//! `AppState` is the shared container Tauri threads through `tauri::State`.
//! `DictationController` owns the cross-task wiring: hotkey down spawns it,
//! hotkey up tells it to stop. Phase 1 wires it to audio + the pill window;
//! later phases add the engine + injection layers behind the same surface.
//!
//! We avoid `Arc<Mutex<…>>` for cross-task state where possible — the
//! controller uses an internal `Mutex<Option<Session>>` only at start/stop
//! transitions, and never holds the lock across `.await`.

#![allow(dead_code)] // wired into commands as later phases land

use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Runtime};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::audio::capture::CaptureSession;

/// Lazily-initialised in `lib::run::setup`.
#[derive(Default)]
pub struct AppState {
    // Future fields (engine, settings) land here as they're built.
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Holds whatever a single in-flight dictation owns.
///
/// Public to the crate so `events.rs` can construct + destructure it; never
/// expose this beyond the crate boundary.
pub struct DictationSession {
    pub(crate) capture: CaptureSession,
    pub(crate) level_pump: JoinHandle<()>,
}

impl DictationSession {
    pub fn new(capture: CaptureSession, level_pump: JoinHandle<()>) -> Self {
        Self {
            capture,
            level_pump,
        }
    }

    /// Move the inner pieces out for clean shutdown.
    pub fn into_parts(self) -> Self {
        self
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

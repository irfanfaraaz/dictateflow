//! Speech-recognition engine abstraction.
//!
//! All engines (LocalEngine on whisper.cpp today, ScribeEngine over WebSocket
//! in Phase 7) implement the same trait so the dictation controller can swap
//! between them at runtime without knowing the implementation.
//!
//! # Pipeline shape
//!
//! ```text
//!   cpal callback ─► HeapRb ─► drain task ─► mpsc::Sender<AudioChunk>
//!                                                        │
//!                                                        ▼
//!                                          Engine::stream(rx, cancel)
//!                                                        │
//!                                                        ▼
//!                                          Stream<Result<Partial, …>>
//!                                                        │
//!                                                        ▼
//!                                            dictation:final emit
//! ```
//!
//! # Thread-safety contract
//!
//! Engines are `Send + Sync` and held behind an `Arc` for the lifetime of
//! the app. Any CPU-heavy / blocking work (Whisper inference) MUST run inside
//! `tokio::task::spawn_blocking` so the async runtime stays responsive.

#![allow(dead_code)] // surfaced incrementally across Phase 2.x

pub mod factory;
pub mod local;
pub mod scribe;
pub mod transcript;

use async_trait::async_trait;
use futures::stream::BoxStream;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::audio::AudioError;

/// A chunk of mono f32 PCM samples at a known sample rate.
///
/// The audio capture layer pumps these through an `mpsc::Sender<AudioChunk>`
/// to whichever engine is active. The dictation audio pump resamples to
/// exactly 16 kHz before sending, so `sample_rate` is always 16 000 in the
/// app; engines still check it (and resample or reject) so a future caller
/// can't silently feed them the wrong rate.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

/// A single transcription result, possibly intermediate.
///
/// `LocalEngine` emits a single `is_final = true` result per session;
/// `ScribeEngine` also streams `is_final = false` interim text (committed
/// segments so far + the current partial).
#[derive(Debug, Clone)]
pub struct Partial {
    pub text: String,
    pub is_final: bool,
    pub confidence: Option<f32>,
}

/// Errors any engine can raise.
///
/// Per the project's hard rules: `thiserror` for library-layer enums,
/// `anyhow` only at the Tauri command boundary. `EngineError` converts
/// freely into `anyhow::Error` via `?`.
#[derive(Debug, Error)]
pub enum EngineError {
    #[error("missing API key — set ELEVENLABS_API_KEY in settings")]
    MissingApiKey,

    #[error("model not installed: {0}")]
    ModelNotInstalled(String),

    #[error("audio capture failed: {0}")]
    Audio(#[from] AudioError),

    #[error("inference failed: {0}")]
    Inference(String),

    #[error("cancelled")]
    Cancelled,

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// WebSocket transport failure (handshake, frame send/recv, close).
    /// Used by [`scribe::ScribeEngine`].
    #[error("websocket: {0}")]
    WebSocket(String),

    /// Could not reach the remote service at all (DNS, TCP/TLS failure,
    /// connect timeout) — usually "no internet".
    #[error("connect: {0}")]
    Connect(String),
}

/// Convenience alias used in implementations.
pub type EngineResult<T> = std::result::Result<T, EngineError>;

// async_trait adds #[must_use] to methods returning a boxed future, which
// newer clippy flags as double_must_use.
#[allow(clippy::double_must_use)]
#[async_trait]
pub trait Engine: Send + Sync {
    /// Begin a transcription session.
    ///
    /// The engine takes ownership of the audio receiver and runs until either
    /// the sender is dropped (end-of-stream) or `cancel` is fired. The
    /// returned stream yields `Partial`s and ends when the engine finalises.
    async fn stream(
        &self,
        audio: mpsc::Receiver<AudioChunk>,
        cancel: CancellationToken,
    ) -> EngineResult<BoxStream<'static, EngineResult<Partial>>>;

    /// Lightweight readiness check — model loaded, API key present, etc.
    /// Phase 7+ will surface this in the settings UI.
    async fn health_check(&self) -> EngineResult<()>;
}

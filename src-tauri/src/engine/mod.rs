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

pub mod local;
pub mod scribe;

use async_trait::async_trait;
use futures::stream::BoxStream;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::audio::AudioError;

/// A chunk of 16 kHz mono f32 PCM samples.
///
/// The audio capture layer pumps these through an `mpsc::Sender<AudioChunk>`
/// to whichever engine is active. Phase 2 batches them all and runs Whisper
/// once at end-of-stream; Phase 6 will consume them as they arrive.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub samples: Vec<f32>,
}

/// A single transcription result, possibly intermediate.
///
/// Phase 2 only ever emits `is_final = true` (batch mode). Phase 6's streaming
/// implementation will emit `is_final = false` partials every ~1 s.
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
}

/// Convenience alias used in implementations.
pub type EngineResult<T> = std::result::Result<T, EngineError>;

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

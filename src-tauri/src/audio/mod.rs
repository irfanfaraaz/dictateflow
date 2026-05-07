//! Audio capture pipeline.
//!
//! Web analogue: this module is the equivalent of `getUserMedia({ audio: true })`
//! plumbed into a `MediaRecorder` — except we have to talk directly to CoreAudio
//! via `cpal`, and the audio thread is a real-time priority OS thread that
//! cannot allocate, lock, or log without risking dropped frames.
//!
//! The contract:
//! - Output format is **16 kHz mono f32** (whisper.cpp's native input).
//! - Samples flow through a lock-free `ringbuf::HeapRb<f32>` to async consumers.
//! - The latest RMS level is published on a `tokio::sync::watch` channel.
//! - The cpal callback never allocates, locks, or logs.

#![allow(dead_code)] // wired into AppState in Task 1.6

pub mod capture;
pub mod level;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("no default input device available")]
    NoDefaultDevice,
    #[error("device offered no supported input config: {0}")]
    UnsupportedConfig(String),
    #[error("failed to build cpal stream: {0}")]
    BuildStream(String),
    #[error("failed to start cpal stream: {0}")]
    PlayStream(String),
}

/// Target sample rate for downstream consumers (whisper.cpp expects 16k).
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

//! Audio capture pipeline.
//!
//! Web analogue: this module is the equivalent of `getUserMedia({ audio: true })`
//! plumbed into a `MediaRecorder` — except we have to talk directly to CoreAudio
//! via `cpal`, and the audio thread is a real-time priority OS thread that
//! cannot allocate, lock, or log without risking dropped frames.
//!
//! The contract:
//! - `capture` produces **mono f32 at the device's native rate** into a
//!   lock-free `ringbuf::HeapRb<f32>`.
//! - `resample` converts that to **16 kHz** (whisper.cpp's and Scribe's
//!   input rate) off the RT thread, in the dictation audio pump.
//! - The peak RMS level is published through a lock-free `level::LevelMeter`.
//! - The cpal callback never allocates, locks, or logs.

#![allow(dead_code)] // some helpers are only used by tests / later phases

pub mod capture;
pub mod level;
pub mod resample;
pub mod vad;

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

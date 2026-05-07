//! cpal-based microphone capture.
//!
//! # Threading model
//!
//! The cpal stream callback runs on a dedicated CoreAudio real-time thread.
//! It MUST NOT:
//! - allocate (no `Vec::push`, no `Box::new`, no `format!`)
//! - lock (no `Mutex`, no `RwLock`, no blocking sync primitive)
//! - log (no `tracing::*`, no `println!` — all of those allocate or lock)
//!
//! Web analogue: imagine if `requestAnimationFrame` could miss frames and the
//! user would *hear* the glitch. That's what happens when we miss a deadline
//! in the audio thread. So we just `try_send`, drop on overflow, and move on.
//!
//! # Sample-rate conversion
//!
//! cpal exposes whatever rate the device prefers (44.1k or 48k on most macs).
//! whisper.cpp wants 16k mono f32. We do a 3-tap nearest-neighbour decimation
//! inside the callback — cheap, deterministic, no allocation, "good enough"
//! for speech recognition (whisper itself oversamples internally). A higher-
//! quality resampler (e.g. `dasp_signal::interpolate` or `samplerate`) is
//! deferred until measurable WER regression shows up.

// SAFETY: cpal's `Stream` is `!Send` because the underlying CoreAudio
// AudioUnit is bound to its creating thread. We mark our owning wrapper
// `Send + Sync` only because we never actually access the stream from any
// thread other than the one that built it — the Mutex is just an owner
// that we drop in place. All real cross-thread communication happens via
// the lock-free ringbuf and the watch channel.
#![allow(unsafe_code)]

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    BuildStreamError, Sample, SampleFormat, Stream, StreamConfig,
};
use ringbuf::{
    traits::{Producer, Split},
    HeapCons, HeapProd, HeapRb,
};
use tokio::sync::watch;
use tracing::{debug, warn};

use super::{AudioError, TARGET_SAMPLE_RATE};

/// Capacity of the sample ring buffer in 16 kHz f32 frames.
///
/// 5 seconds at 16k = 80_000 samples; well above the consumer's 100 ms
/// poll cadence so transient stalls (GC pauses, IPC congestion) don't drop
/// audio.
const RING_CAPACITY_FRAMES: usize = TARGET_SAMPLE_RATE as usize * 5;

/// Handle returned by [`AudioCapture::start`].
///
/// Holding this keeps the stream alive. Dropping it stops capture.
pub struct CaptureSession {
    // Stream is `!Send` on some backends. We never move it across threads;
    // the Mutex is purely to satisfy the borrow checker — we only ever lock
    // it from the owning task to call `pause()` on stop.
    _stream: StreamHandle,
    stopped: Arc<AtomicBool>,
}

/// Thin wrapper so `Stream` (which is `!Send` on macOS) lives behind a guard
/// that we can drop in the same task that built it. We never actually share
/// the inner stream across threads.
struct StreamHandle(Mutex<Option<Stream>>);

// SAFETY: cpal's `Stream` is `!Send` because the underlying CoreAudio
// AudioUnit is bound to its creating thread. We only use the Mutex as an
// owner that we drop from the same task that built the capture. We never
// access the stream from another thread.
unsafe impl Send for StreamHandle {}
unsafe impl Sync for StreamHandle {}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Ok(mut guard) = self._stream.0.lock() {
            if let Some(stream) = guard.take() {
                // Best effort — pause may fail on already-closed device.
                let _ = stream.pause();
            }
        }
    }
}

/// Starts the default input device and returns a consumer + RMS receiver.
///
/// The session handle MUST be kept alive for the duration of capture; dropping
/// it stops the stream.
pub fn start() -> Result<(CaptureSession, HeapCons<f32>, watch::Receiver<f32>), AudioError> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or(AudioError::NoDefaultDevice)?;

    let supported = device
        .default_input_config()
        .map_err(|e| AudioError::UnsupportedConfig(e.to_string()))?;

    let device_sample_rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.config();

    // Decimation factor (device → 16k). Round to nearest integer; if the
    // device is already at 16k, the factor is 1 and we copy verbatim.
    let decim = (device_sample_rate as f32 / TARGET_SAMPLE_RATE as f32).round() as usize;
    let decim = decim.max(1);

    debug!(
        device_rate = device_sample_rate,
        channels,
        format = ?sample_format,
        decim,
        "audio capture starting"
    );

    let rb = HeapRb::<f32>::new(RING_CAPACITY_FRAMES);
    let (producer, consumer) = rb.split();
    let (level_tx, level_rx) = watch::channel(0.0_f32);

    let stopped = Arc::new(AtomicBool::new(false));
    let stopped_cb = stopped.clone();

    let stream = build_stream(
        &device,
        &config,
        sample_format,
        channels,
        decim,
        producer,
        level_tx,
        stopped_cb,
    )?;

    stream
        .play()
        .map_err(|e| AudioError::PlayStream(e.to_string()))?;

    let session = CaptureSession {
        _stream: StreamHandle(Mutex::new(Some(stream))),
        stopped,
    };

    Ok((session, consumer, level_rx))
}

#[allow(clippy::too_many_arguments)]
fn build_stream(
    device: &cpal::Device,
    config: &StreamConfig,
    fmt: SampleFormat,
    channels: usize,
    decim: usize,
    producer: HeapProd<f32>,
    level_tx: watch::Sender<f32>,
    stopped: Arc<AtomicBool>,
) -> Result<Stream, AudioError> {
    let err_cb = |err| {
        // Stream errors land on a separate callback; we can't `?` from inside
        // the data callback, but we can at least log here (this runs on
        // cpal's error thread, NOT the RT audio thread).
        warn!(?err, "cpal stream error");
    };

    let result: Result<Stream, BuildStreamError> = match fmt {
        SampleFormat::F32 => device.build_input_stream(
            config,
            make_callback::<f32>(channels, decim, producer, level_tx, stopped),
            err_cb,
            None,
        ),
        SampleFormat::I16 => device.build_input_stream(
            config,
            make_callback::<i16>(channels, decim, producer, level_tx, stopped),
            err_cb,
            None,
        ),
        SampleFormat::U16 => device.build_input_stream(
            config,
            make_callback::<u16>(channels, decim, producer, level_tx, stopped),
            err_cb,
            None,
        ),
        other => {
            return Err(AudioError::UnsupportedConfig(format!(
                "sample format {other:?} not supported"
            )));
        }
    };

    result.map_err(|e| AudioError::BuildStream(e.to_string()))
}

/// Build the per-buffer audio callback.
///
/// Generic over the device's native sample type so we convert to f32 once
/// at the source. The callback is FnMut + Send + 'static — no captures may
/// require allocation or locking on the hot path.
fn make_callback<S>(
    channels: usize,
    decim: usize,
    mut producer: HeapProd<f32>,
    level_tx: watch::Sender<f32>,
    stopped: Arc<AtomicBool>,
) -> impl FnMut(&[S], &cpal::InputCallbackInfo) + Send + 'static
where
    S: Sample + cpal::SizedSample,
    f32: cpal::FromSample<S>,
{
    // Pre-size a stack-friendly scratch using a const-cap array.
    // We never grow this — it's a fixed-size local buffer.
    const SCRATCH: usize = 2048;
    let mut scratch = [0.0_f32; SCRATCH];

    move |input: &[S], _info: &cpal::InputCallbackInfo| {
        if stopped.load(Ordering::Acquire) {
            return;
        }

        // Step 1: deinterleave to mono + decimate to ~16k, into the local
        // stack scratch. No allocation, no lock.
        //
        // Frames per channel-tick = channels samples; we average channels
        // into a single mono sample, then take 1 of every `decim` frames.
        let frames = input.len() / channels.max(1);
        let mut written = 0usize;
        let mut sum_sq = 0.0_f32;

        let mut frame_idx = 0;
        while frame_idx < frames && written < SCRATCH {
            // Decimate by keeping only every Nth frame (nearest-neighbour).
            if frame_idx % decim == 0 {
                let base = frame_idx * channels;
                let mut acc = 0.0_f32;
                for ch in 0..channels {
                    let s: f32 = input[base + ch].to_sample::<f32>();
                    acc += s;
                }
                let mono = acc / channels as f32;
                scratch[written] = mono;
                sum_sq += mono * mono;
                written += 1;
            }
            frame_idx += 1;
        }

        if written == 0 {
            return;
        }

        // Step 2: push slice to the lock-free ringbuf. On overflow we drop
        // — better than blocking the RT thread.
        let pushed = producer.push_slice(&scratch[..written]);
        if pushed < written {
            // Overflow happens when the consumer task is too slow. Bookkeep
            // silently — logging here would allocate.
        }

        // Step 3: publish RMS on the watch channel. send() returns Err only
        // when all receivers are dropped, which is fine during teardown.
        let level = (sum_sq / written as f32).sqrt();
        let _ = level_tx.send(level);
    }
}

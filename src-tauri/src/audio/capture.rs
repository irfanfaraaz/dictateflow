//! cpal-based microphone capture.
//!
//! # Threading model
//!
//! The cpal stream callback runs on a dedicated CoreAudio real-time thread.
//! It MUST NOT:
//! - allocate (no `Vec::push`, no `Box::new`, no `format!`)
//! - lock (no `Mutex`, no `RwLock`, no channel `send` that takes a lock)
//! - log (no `tracing::*`, no `println!` — all of those allocate or lock)
//!
//! Web analogue: imagine if `requestAnimationFrame` could miss frames and the
//! user would *hear* the glitch. That's what happens when we miss a deadline
//! in the audio thread. So the callback only downmixes, pushes into a
//! lock-free ringbuf (dropping on overflow), and publishes its RMS through an
//! atomic [`LevelMeter`].
//!
//! # Sample rate
//!
//! The callback emits mono f32 at the device's *native* rate (44.1k / 48k on
//! most Macs, 24k or 16k/8k for Bluetooth headsets). Conversion to the
//! engines' 16 kHz happens off the RT thread in the audio pump, via
//! [`super::resample::Resampler`] (anti-alias low-pass + linear
//! interpolation with state carried across chunks).

// SAFETY: cpal's `Stream` is `!Send` because the underlying CoreAudio
// AudioUnit is bound to its creating thread. We mark our owning wrapper
// `Send + Sync` only because we never actually access the stream from any
// thread other than to pause + drop it — the Mutex is just an owner. All real
// cross-thread communication happens via the lock-free ringbuf and atomics.
#![allow(unsafe_code)]

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    BuildStreamError, Sample, SampleFormat, Stream, StreamConfig, StreamError,
};
use ringbuf::{
    traits::{Producer, Split},
    HeapCons, HeapProd, HeapRb,
};
use tracing::{info, warn};

use super::level::LevelMeter;
use super::AudioError;

/// Seconds of native-rate mono audio the ring buffer can hold. Far above the
/// consumer's 100 ms poll cadence so transient stalls don't drop audio.
const RING_SECONDS: usize = 5;

/// Mono frames converted per inner-loop pass in the RT callback. The
/// callback loops over arbitrarily large device buffers in pieces of this
/// size, so nothing is ever truncated.
const CALLBACK_SCRATCH_FRAMES: usize = 1024;

/// Handle returned by [`start_with_device`].
///
/// Holding this keeps the stream alive. Dropping it stops capture.
pub struct CaptureSession {
    _stream: StreamHandle,
    stopped: Arc<AtomicBool>,
}

/// Thin wrapper so `Stream` (which is `!Send` on macOS) can live inside the
/// session. We never touch the inner stream except to pause + drop it.
struct StreamHandle(Mutex<Option<Stream>>);

// SAFETY: see the module-level note. The stream is only paused and dropped
// through this wrapper; it is never used concurrently.
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

/// Lock-free health flags shared between cpal's callbacks and the dictation
/// monitor (which turns them into user-facing errors).
#[derive(Debug, Default)]
pub struct CaptureHealth {
    /// Set once any non-zero sample arrives. macOS delivers pure digital
    /// silence when mic permission is missing or the device is muted.
    saw_signal: AtomicBool,
    /// Set by the error callback when the device disappears (unplugged
    /// headset, Bluetooth disconnect).
    device_lost: AtomicBool,
}

impl CaptureHealth {
    pub fn saw_signal(&self) -> bool {
        self.saw_signal.load(Ordering::Acquire)
    }

    pub fn device_lost(&self) -> bool {
        self.device_lost.load(Ordering::Acquire)
    }
}

/// Everything a started capture hands to the dictation pipeline.
pub struct CaptureStart {
    /// Keep alive for the duration of capture; dropping it stops the stream.
    pub session: CaptureSession,
    /// Mono f32 samples at `sample_rate`.
    pub consumer: HeapCons<f32>,
    /// Peak-hold RMS level (see [`LevelMeter`]).
    pub level: Arc<LevelMeter>,
    pub health: Arc<CaptureHealth>,
    /// Native device rate of the samples in `consumer`.
    pub sample_rate: u32,
}

/// Starts the default input device.
pub fn start() -> Result<CaptureStart, AudioError> {
    start_with_device(None)
}

/// Starts audio capture on a specific device (or default if `device_id` is
/// `None`).
///
/// `device_id` is a device name as returned by `list_input_devices`. If no
/// device with that name exists we log a warning and fall back to the
/// system default.
///
/// This opens CoreAudio synchronously (can take tens of ms); call it from a
/// blocking context (`tokio::task::block_in_place`) rather than directly on
/// an async worker.
pub fn start_with_device(device_id: Option<&str>) -> Result<CaptureStart, AudioError> {
    let host = cpal::default_host();
    let device = match device_id {
        None => host
            .default_input_device()
            .ok_or(AudioError::NoDefaultDevice)?,
        Some(target_name) => {
            let found = host
                .input_devices()
                .map_err(|e| AudioError::BuildStream(format!("enumerate devices: {e}")))?
                .find(|d| d.name().map(|n| n == target_name).unwrap_or(false));

            match found {
                Some(d) => d,
                None => {
                    warn!(
                        requested_device = %target_name,
                        "requested input device not found; falling back to default"
                    );
                    host.default_input_device()
                        .ok_or(AudioError::NoDefaultDevice)?
                }
            }
        }
    };

    let supported = device
        .default_input_config()
        .map_err(|e| AudioError::UnsupportedConfig(e.to_string()))?;

    let device_name = device.name().unwrap_or_else(|_| "<unknown>".into());
    let sample_rate = supported.sample_rate().0;
    let channels = usize::from(supported.channels());
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.config();

    info!(
        device = %device_name,
        sample_rate,
        channels,
        format = ?sample_format,
        "audio capture starting"
    );

    let capacity = (sample_rate as usize).max(8_000) * RING_SECONDS;
    let rb = HeapRb::<f32>::new(capacity);
    let (producer, consumer) = rb.split();

    let level = Arc::new(LevelMeter::new());
    let health = Arc::new(CaptureHealth::default());
    let stopped = Arc::new(AtomicBool::new(false));

    let stream = build_stream(
        &device,
        &config,
        sample_format,
        channels,
        CallbackShared {
            producer,
            level: level.clone(),
            health: health.clone(),
            stopped: stopped.clone(),
        },
    )?;

    stream
        .play()
        .map_err(|e| AudioError::PlayStream(e.to_string()))?;

    let session = CaptureSession {
        _stream: StreamHandle(Mutex::new(Some(stream))),
        stopped,
    };

    Ok(CaptureStart {
        session,
        consumer,
        level,
        health,
        sample_rate,
    })
}

/// State moved into the RT callback.
struct CallbackShared {
    producer: HeapProd<f32>,
    level: Arc<LevelMeter>,
    health: Arc<CaptureHealth>,
    stopped: Arc<AtomicBool>,
}

fn build_stream(
    device: &cpal::Device,
    config: &StreamConfig,
    fmt: SampleFormat,
    channels: usize,
    shared: CallbackShared,
) -> Result<Stream, AudioError> {
    let health_for_err = shared.health.clone();
    // Runs on cpal's error thread, NOT the RT audio thread, so logging is ok.
    let err_cb = move |err: StreamError| match err {
        StreamError::DeviceNotAvailable => {
            warn!("cpal stream error: input device no longer available");
            health_for_err.device_lost.store(true, Ordering::Release);
        }
        other => warn!(?other, "cpal stream error"),
    };

    let result: Result<Stream, BuildStreamError> = match fmt {
        SampleFormat::F32 => {
            device.build_input_stream(config, make_callback::<f32>(channels, shared), err_cb, None)
        }
        SampleFormat::I16 => {
            device.build_input_stream(config, make_callback::<i16>(channels, shared), err_cb, None)
        }
        SampleFormat::U16 => {
            device.build_input_stream(config, make_callback::<u16>(channels, shared), err_cb, None)
        }
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
/// at the source. No captures allocate or lock on the hot path.
fn make_callback<S>(
    channels: usize,
    shared: CallbackShared,
) -> impl FnMut(&[S], &cpal::InputCallbackInfo) + Send + 'static
where
    S: Sample + cpal::SizedSample,
    f32: cpal::FromSample<S>,
{
    let CallbackShared {
        mut producer,
        level,
        health,
        stopped,
    } = shared;
    let channels = channels.max(1);
    let mut scratch = [0.0_f32; CALLBACK_SCRATCH_FRAMES];

    move |input: &[S], _info: &cpal::InputCallbackInfo| {
        if stopped.load(Ordering::Acquire) {
            return;
        }

        let mut sum_sq = 0.0_f32;
        let mut frames_total = 0usize;
        let mut nonzero = false;

        // Downmix to mono in scratch-sized pieces; push each piece.
        for block in input.chunks(CALLBACK_SCRATCH_FRAMES * channels) {
            let mut n = 0usize;
            for (slot, frame) in scratch.iter_mut().zip(block.chunks_exact(channels)) {
                let mut acc = 0.0_f32;
                for &s in frame {
                    acc += s.to_sample::<f32>();
                }
                let mono = acc / channels as f32;
                *slot = mono;
                sum_sq += mono * mono;
                nonzero |= mono != 0.0;
                n += 1;
            }
            // On overflow push_slice drops the excess — better than blocking
            // the RT thread. (Bookkept silently; logging would allocate.)
            let _ = producer.push_slice(&scratch[..n]);
            frames_total += n;
        }

        if frames_total == 0 {
            return;
        }
        if nonzero && !health.saw_signal.load(Ordering::Relaxed) {
            health.saw_signal.store(true, Ordering::Release);
        }
        level.record((sum_sq / frames_total as f32).sqrt());
    }
}

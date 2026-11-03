//! `LocalEngine` — whisper.cpp via whisper-rs (Metal-accelerated on Apple Silicon).
//!
//! Batch per dictation: the engine accumulates all audio for the session and
//! runs **one** inference when the audio channel closes (hotkey released,
//! VAD early-stop, or cap). An earlier version re-transcribed the whole
//! growing buffer every second for interim partials; nothing consumed them,
//! and the final pass had to wait for the in-flight interim pass, so they
//! only added latency and battery drain.
//!
//! - Inference runs on `tokio::task::spawn_blocking` (non-negotiable — see
//!   CLAUDE.md hard rules) and is raced against `cancel`, so a stop timeout
//!   ends the session immediately instead of waiting for whisper.
//! - Input chunks are expected at 16 kHz (the audio pump resamples). Chunks
//!   at any other rate are resampled here rather than fed to whisper at the
//!   wrong speed.
//!
//! # The WhisperContext-once pattern
//!
//! Loading a GGML model is expensive (file I/O, GGUF parsing, Metal shader
//! init). whisper.cpp's recommended pattern is to load the *context* exactly
//! once per process and create a fresh *state* per inference. We expose that
//! by holding `Arc<WhisperContext>` inside `LocalEngine` and calling
//! `ctx.create_state()` inside each `spawn_blocking` task.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use futures::channel::mpsc as futures_mpsc;
use futures::stream::BoxStream;
use futures::SinkExt as _;
use futures::StreamExt as _;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use super::{AudioChunk, Engine, EngineError, EngineResult, Partial};
use crate::audio::resample::Resampler;
use crate::audio::TARGET_SAMPLE_RATE;

/// Probability above which whisper treats a segment as "no speech".
const NO_SPEECH_THRESHOLD: f32 = 0.6;

/// Loaded whisper.cpp context. Cheap to clone (it's an `Arc` internally)
/// and safe to share across dictations.
pub struct LocalEngine {
    ctx: Arc<WhisperContext>,
    model_path: PathBuf,
    /// Whisper language code (`"en"`, `"de"`, …); `None` = autodetect.
    language: Option<String>,
}

impl LocalEngine {
    /// Load a GGML model from disk. The on-disk file is mmap'd by whisper.cpp,
    /// so this is fast (~tens of ms) once the OS page cache is warm. First
    /// call after boot may take a second or so as the file is paged in.
    ///
    /// Synchronous: call from a blocking context.
    pub fn load(model_path: impl AsRef<Path>) -> EngineResult<Self> {
        let model_path = model_path.as_ref().to_path_buf();
        if !model_path.exists() {
            return Err(EngineError::ModelNotInstalled(
                model_path.display().to_string(),
            ));
        }

        let model_str = model_path
            .to_str()
            .ok_or_else(|| EngineError::Inference("model path is not valid UTF-8".into()))?;

        // Metal acceleration is enabled by the `metal` feature flag; we still
        // toggle `use_gpu` explicitly so the choice is visible at the call
        // site (and so we can flip it off in tests / on non-Metal builds).
        let mut ctx_params = WhisperContextParameters::default();
        ctx_params.use_gpu(true);

        info!(model = %model_path.display(), "loading whisper context");
        let ctx = WhisperContext::new_with_params(model_str, ctx_params)
            .map_err(|e| EngineError::Inference(format!("WhisperContext::new: {e}")))?;

        Ok(Self {
            ctx: Arc::new(ctx),
            model_path,
            language: None,
        })
    }

    /// Force a transcription language (`None` = autodetect).
    pub fn with_language(mut self, language: Option<String>) -> Self {
        self.language = language;
        self
    }

    pub fn model_path(&self) -> &Path {
        &self.model_path
    }
}

/// Build the per-inference parameter struct.
///
/// Pulled into a free function so we can unit-test the construction without
/// loading a real context. Defaults match the design spec §7:
/// - language from settings (`None` → autodetect)
/// - no translation, no carry-over context (fresh per dictation)
/// - blank / non-speech tokens suppressed so silence doesn't transcribe as
///   `[BLANK_AUDIO]` or `(music)` (the events layer filters any that slip
///   through)
/// - quiet stdout (no progress / realtime / special tokens)
fn build_full_params(language: Option<&str>) -> FullParams<'_, '_> {
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(language);
    params.set_translate(false);
    params.set_no_context(true);
    params.set_single_segment(false);
    params.set_suppress_blank(true);
    params.set_suppress_non_speech_tokens(true);
    params.set_no_speech_thold(NO_SPEECH_THRESHOLD);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params
}

#[async_trait]
impl Engine for LocalEngine {
    async fn stream(
        &self,
        mut audio: mpsc::Receiver<AudioChunk>,
        cancel: CancellationToken,
    ) -> EngineResult<BoxStream<'static, EngineResult<Partial>>> {
        let ctx = self.ctx.clone();
        let language = self.language.clone();

        // A single final result (or error) flows out; capacity 2 is plenty.
        let (mut tx, rx) = futures_mpsc::channel::<EngineResult<Partial>>(2);

        tokio::spawn(async move {
            // Accumulate all audio for the session (16 kHz mono).
            let mut buffer: Vec<f32> = Vec::with_capacity(16_000 * 30);
            let mut resampler: Option<Resampler> = None;

            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        let _ = tx.send(Err(EngineError::Cancelled)).await;
                        return;
                    }
                    maybe_chunk = audio.recv() => match maybe_chunk {
                        Some(chunk) => append_16k(&mut buffer, &mut resampler, &chunk),
                        // Sender dropped → clean end-of-stream.
                        None => break,
                    },
                }
            }

            if buffer.is_empty() {
                // Nothing to transcribe — emit a final empty Partial so the
                // controller can still advance its state machine.
                let _ = tx
                    .send(Ok(Partial {
                        text: String::new(),
                        is_final: true,
                        confidence: None,
                    }))
                    .await;
                return;
            }

            debug!(
                samples = buffer.len(),
                seconds = buffer.len() as f32 / TARGET_SAMPLE_RATE as f32,
                "local engine: running final inference"
            );

            let ctx_final = ctx.clone();
            let handle = tokio::task::spawn_blocking(move || {
                run_window(&ctx_final, &buffer, language.as_deref())
            });

            // Race inference against cancel. On cancel the blocking task
            // keeps running to completion in the pool, but its result is
            // dropped and the stream ends now.
            let result = tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(EngineError::Cancelled),
                joined = handle => match joined {
                    Ok(r) => r,
                    Err(e) => Err(EngineError::Inference(format!("blocking task: {e}"))),
                },
            };
            let _ = tx.send(result).await;
        });

        Ok(rx.boxed())
    }

    async fn health_check(&self) -> EngineResult<()> {
        // The context is non-null because `load()` would have errored;
        // a deeper health check (run a dummy 1-sample inference) is overkill.
        Ok(())
    }
}

/// Append `chunk` to the 16 kHz session buffer, resampling if the chunk
/// arrives at any other rate.
fn append_16k(buffer: &mut Vec<f32>, resampler: &mut Option<Resampler>, chunk: &AudioChunk) {
    if chunk.sample_rate == TARGET_SAMPLE_RATE {
        buffer.extend_from_slice(&chunk.samples);
        return;
    }
    let needs_new = resampler
        .as_ref()
        .is_none_or(|r| r.in_rate() != chunk.sample_rate);
    if needs_new {
        warn!(
            sample_rate = chunk.sample_rate,
            "local engine: resampling non-16 kHz input"
        );
        *resampler = Some(Resampler::new(chunk.sample_rate, TARGET_SAMPLE_RATE));
    }
    if let Some(r) = resampler.as_mut() {
        r.process(&chunk.samples, buffer);
    }
}

/// Run whisper inference on the 16 kHz `samples` and return the final
/// `Partial`.
///
/// Caller must run this inside `tokio::task::spawn_blocking` — it is
/// synchronous and CPU/GPU-heavy. Never call from the async runtime.
fn run_window(
    ctx: &WhisperContext,
    samples: &[f32],
    language: Option<&str>,
) -> EngineResult<Partial> {
    let start = Instant::now();
    let mut state = ctx
        .create_state()
        .map_err(|e| EngineError::Inference(format!("create_state: {e}")))?;

    let params = build_full_params(language);
    state
        .full(params, samples)
        .map_err(|e| EngineError::Inference(format!("state.full: {e}")))?;

    let n_segments = state
        .full_n_segments()
        .map_err(|e| EngineError::Inference(format!("full_n_segments: {e}")))?;

    let mut text = String::new();
    for i in 0..n_segments {
        match state.full_get_segment_text_lossy(i) {
            Ok(seg) => {
                let seg = seg.trim();
                if seg.is_empty() {
                    continue;
                }
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(seg);
            }
            Err(e) => warn!(?e, segment = i, "failed to read segment"),
        }
    }

    debug!(
        elapsed_ms = start.elapsed().as_millis(),
        segments = n_segments,
        "whisper inference complete"
    );

    Ok(Partial {
        text,
        is_final: true,
        confidence: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_full_params_matches_spec_defaults() {
        // Smoke test: the constructor should not panic and the resulting
        // struct should be usable. The actual flag values are opaque (private
        // fields) so we just exercise the path.
        let _auto = build_full_params(None);
        let _en = build_full_params(Some("en"));
    }

    #[test]
    fn append_16k_passes_through_and_resamples() {
        let mut buf = Vec::new();
        let mut rs = None;
        append_16k(
            &mut buf,
            &mut rs,
            &AudioChunk {
                samples: vec![0.1; 160],
                sample_rate: 16_000,
            },
        );
        assert_eq!(buf.len(), 160);
        assert!(rs.is_none());

        append_16k(
            &mut buf,
            &mut rs,
            &AudioChunk {
                samples: vec![0.1; 4_800],
                sample_rate: 48_000,
            },
        );
        // 100 ms at 48 kHz → ~1600 samples at 16 kHz.
        let added = buf.len() - 160;
        assert!((1_590..=1_600).contains(&added), "added {added}");
    }

    #[tokio::test]
    async fn missing_model_returns_not_installed() {
        match LocalEngine::load("/nonexistent/path/to/ggml-small.bin") {
            Ok(_) => panic!("expected ModelNotInstalled error"),
            Err(EngineError::ModelNotInstalled(path)) => {
                assert!(path.contains("ggml-small.bin"));
            }
            Err(other) => panic!("expected ModelNotInstalled, got {other:?}"),
        }
    }
}

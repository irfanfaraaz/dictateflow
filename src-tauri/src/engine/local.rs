//! `LocalEngine` — whisper.cpp via whisper-rs (Metal-accelerated on Apple Silicon).
//!
//! Phase 2 ships a *batch* implementation: we drain the audio receiver into a
//! `Vec<f32>` until end-of-stream (sender dropped) or cancellation, then run
//! `WhisperState::full(...)` once on the accumulated buffer and emit a single
//! `Partial { is_final: true }`. Phase 6 replaces this with a sliding-window
//! streaming loop (intermediate partials every ~1 s).
//!
//! # The WhisperContext-once pattern
//!
//! Loading a GGML model is expensive (file I/O, GGUF parsing, Metal shader
//! init). whisper.cpp's recommended pattern is to load the *context* exactly
//! once per process and create a fresh *state* per inference. We expose that
//! by holding `Arc<WhisperContext>` inside `LocalEngine` and calling
//! `ctx.create_state()` inside the per-dictation `spawn_blocking` task.
//!
//! # Why `spawn_blocking`
//!
//! `WhisperState::full(...)` is a synchronous, CPU/GPU-heavy call that can
//! easily block for hundreds of milliseconds. Running it on the async runtime
//! would stall every other task (timers, IPC, Tauri commands). `spawn_blocking`
//! moves it to the dedicated blocking pool — see JOURNEY §05 for the longer
//! explanation.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{self, BoxStream, StreamExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use super::{AudioChunk, Engine, EngineError, EngineResult, Partial};

/// Loaded whisper.cpp context. Cheap to clone (it's an `Arc` internally)
/// and safe to share across dictations.
pub struct LocalEngine {
    ctx: Arc<WhisperContext>,
    model_path: PathBuf,
}

impl LocalEngine {
    /// Load a GGML model from disk. The on-disk file is mmap'd by whisper.cpp,
    /// so this is fast (~tens of ms) once the OS page cache is warm. First
    /// call after boot may take a second or so as the file is paged in.
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
        })
    }

    pub fn model_path(&self) -> &Path {
        &self.model_path
    }
}

/// Build the per-inference parameter struct.
///
/// Pulled into a free function so we can unit-test the construction without
/// loading a real context. Defaults match the design spec §7:
/// - language `auto` (None → autodetect)
/// - no translation, no carry-over context (fresh per dictation)
/// - quiet stdout (no progress / realtime / special tokens)
fn build_full_params<'a>() -> FullParams<'a, 'a> {
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(None);
    params.set_translate(false);
    params.set_no_context(true);
    params.set_single_segment(false);
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

        // 1. Drain audio receiver in the async layer (we don't want to hold
        //    the blocking pool slot while waiting on IPC).
        //
        //    Exit conditions:
        //    - sender dropped (`recv()` returns None) → clean end-of-stream;
        //      run inference on whatever we collected.
        //    - cancel fired → abort with `EngineError::Cancelled`. The
        //      controller uses this for cancel-on-restart (Phase 9). The
        //      *normal* end of a hold-to-talk dictation is sender-drop, NOT
        //      cancellation.
        //
        //    Drainage runs on the runtime, but it's all tiny copies — no
        //    inference yet. Inference is a single `spawn_blocking` at the
        //    end (Phase 2 batch semantics).
        let mut buffer: Vec<f32> = Vec::with_capacity(16_000 * 30); // 30 s
        let cancelled = loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break true,
                maybe_chunk = audio.recv() => match maybe_chunk {
                    Some(chunk) => buffer.extend_from_slice(&chunk.samples),
                    None => break false,
                },
            }
        };

        debug!(
            samples = buffer.len(),
            seconds = buffer.len() as f32 / 16_000.0,
            cancelled,
            "local engine: audio drained, dispatching to spawn_blocking"
        );

        if cancelled {
            return Ok(stream::iter(vec![Err(EngineError::Cancelled)]).boxed());
        }

        if buffer.is_empty() {
            // Nothing to transcribe — emit a final empty Partial so the
            // controller can still advance its state machine.
            return Ok(stream::iter(vec![Ok(Partial {
                text: String::new(),
                is_final: true,
                confidence: None,
            })])
            .boxed());
        }

        // 2. Run inference on the blocking pool. `spawn_blocking` is the
        //    canonical way to call sync code from an async runtime — see
        //    JOURNEY §05.
        let result = tokio::task::spawn_blocking(move || run_full(&ctx, buffer))
            .await
            .map_err(|e| EngineError::Inference(format!("blocking task: {e}")))?;

        let partial = result?;
        Ok(stream::iter(vec![Ok(partial)]).boxed())
    }

    async fn health_check(&self) -> EngineResult<()> {
        // The context is non-null because `load()` would have errored;
        // a deeper health check (run a dummy 1-sample inference) is overkill
        // for Phase 2.
        Ok(())
    }
}

/// Run a single batch of `state.full(...)` and concatenate every segment's
/// text into one transcript. Caller is responsible for ensuring `samples` is
/// 16 kHz mono f32.
fn run_full(ctx: &WhisperContext, samples: Vec<f32>) -> EngineResult<Partial> {
    let mut state = ctx
        .create_state()
        .map_err(|e| EngineError::Inference(format!("create_state: {e}")))?;

    let params = build_full_params();
    state
        .full(params, &samples)
        .map_err(|e| EngineError::Inference(format!("state.full: {e}")))?;

    let n_segments = state
        .full_n_segments()
        .map_err(|e| EngineError::Inference(format!("full_n_segments: {e}")))?;

    let mut text = String::new();
    for i in 0..n_segments {
        match state.full_get_segment_text_lossy(i) {
            Ok(seg) => {
                if !text.is_empty() && !seg.starts_with(' ') {
                    text.push(' ');
                }
                text.push_str(seg.trim());
            }
            Err(e) => warn!(?e, segment = i, "failed to read segment"),
        }
    }

    let text = text.trim().to_string();
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
        let _params = build_full_params();
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

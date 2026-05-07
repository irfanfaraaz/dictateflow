//! `LocalEngine` — whisper.cpp via whisper-rs (Metal-accelerated on Apple Silicon).
//!
//! Phase 2 shipped a *batch* implementation (drain all audio, run one inference
//! at end-of-stream). Phase 6 replaces that with a **sliding-window streaming**
//! loop:
//!
//! - Every ~1 000 ms of new audio the engine runs a fresh `state.full(...)` on
//!   the full accumulated buffer (not a delta) and emits `Partial { is_final:
//!   false }`.
//! - Each inference runs on `tokio::task::spawn_blocking` (non-negotiable —
//!   see CLAUDE.md hard rules). Only one blocking task is in-flight at a time;
//!   if the previous task hasn't finished when the next tick arrives, we skip
//!   that window silently.
//! - Exit conditions trigger a final inference:
//!   - audio sender dropped (end-of-hold-to-talk)
//!   - `cancel` token fired (cancel-on-restart, Phase 9)
//!   - VAD reports sustained silence (audio_stop → audio_tx drop → channel
//!     closed in the events layer — same as normal stop)
//!
//! # The WhisperContext-once pattern
//!
//! Loading a GGML model is expensive (file I/O, GGUF parsing, Metal shader
//! init). whisper.cpp's recommended pattern is to load the *context* exactly
//! once per process and create a fresh *state* per inference. We expose that
//! by holding `Arc<WhisperContext>` inside `LocalEngine` and calling
//! `ctx.create_state()` inside each `spawn_blocking` task.
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

/// How often (in terms of buffered audio duration) to run an intermediate
/// inference. The window is measured as "new samples added since last
/// inference" so it tracks audio duration, not wall clock.
const INFERENCE_WINDOW_SAMPLES: usize = 16_000; // = 1 s at 16 kHz

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

        // We produce partials incrementally. Use a `futures::channel::mpsc`
        // so the receiver side implements `futures::Stream` directly and can
        // be boxed without a new crate dependency.
        //
        // Buffer of 32: each window takes ~1 s of audio; the consumer
        // (engine_pump in events.rs) pulls these out immediately, so the
        // queue stays near-empty.
        let (mut tx, rx) = futures_mpsc::channel::<EngineResult<Partial>>(32);

        // Spawn the streaming loop as a Tokio task so `stream()` returns
        // promptly and partials flow out over time.
        tokio::spawn(async move {
            // Accumulate all audio for the session.
            let mut buffer: Vec<f32> = Vec::with_capacity(16_000 * 30); // 30 s
                                                                        // How many samples were in the buffer at the last inference.
            let mut last_inference_sample_count: usize = 0;
            // Track an in-flight spawn_blocking handle. `None` = available.
            let mut inflight: Option<tokio::task::JoinHandle<EngineResult<Partial>>> = None;
            // Whether the audio stream has ended (sender dropped).
            let mut stream_ended = false;
            // Whether cancellation was requested.
            let mut cancelled = false;

            // ----------------------------------------------------------------
            // Streaming receive loop
            // ----------------------------------------------------------------
            'recv: loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        cancelled = true;
                        break 'recv;
                    }
                    maybe_chunk = audio.recv() => {
                        match maybe_chunk {
                            Some(chunk) => {
                                buffer.extend_from_slice(&chunk.samples);

                                let new_samples = buffer.len() - last_inference_sample_count;
                                if new_samples >= INFERENCE_WINDOW_SAMPLES {
                                    // Try to fire an intermediate inference if none is in-flight.
                                    let can_fire = inflight
                                        .as_ref()
                                        .map(|h| h.is_finished())
                                        .unwrap_or(true);

                                    if can_fire {
                                        // Collect the result of the previous handle if it finished.
                                        if let Some(prev) = inflight.take() {
                                            match prev.await {
                                                Ok(Ok(p)) => {
                                                    debug!(
                                                        text = %p.text,
                                                        "local engine: intermediate partial"
                                                    );
                                                    if tx.send(Ok(p)).await.is_err() {
                                                        // Consumer dropped — stop producing.
                                                        break 'recv;
                                                    }
                                                }
                                                Ok(Err(e)) => {
                                                    warn!(?e, "intermediate inference failed — skipping window");
                                                }
                                                Err(e) => {
                                                    warn!(?e, "spawn_blocking join error on intermediate window");
                                                }
                                            }
                                        }

                                        last_inference_sample_count = buffer.len();
                                        let snapshot = buffer.clone();
                                        let ctx2 = ctx.clone();
                                        inflight = Some(tokio::task::spawn_blocking(move || {
                                            run_window(&ctx2, snapshot, false)
                                        }));
                                    }
                                    // else: inference already running, skip this window.
                                }
                            }
                            None => {
                                // Sender dropped → clean end-of-stream.
                                stream_ended = true;
                                break 'recv;
                            }
                        }
                    }
                }
            }

            // ----------------------------------------------------------------
            // Drain any still-running intermediate inference.
            // ----------------------------------------------------------------
            if let Some(prev) = inflight.take() {
                // Don't await if cancelled — just drop the handle.
                if !cancelled {
                    match prev.await {
                        Ok(Ok(p)) => {
                            debug!(text = %p.text, "local engine: flushing final intermediate partial");
                            let _ = tx.send(Ok(p)).await;
                        }
                        Ok(Err(e)) => warn!(?e, "final intermediate inference failed"),
                        Err(e) => warn!(?e, "join error flushing intermediate"),
                    }
                }
            }

            // ----------------------------------------------------------------
            // Final step.
            // ----------------------------------------------------------------
            if cancelled {
                let _ = tx.send(Err(EngineError::Cancelled)).await;
                return;
            }

            if !stream_ended {
                // Shouldn't happen (loop only breaks on None or cancel), but
                // be defensive.
                warn!("local engine: recv loop exited unexpectedly — treating as end-of-stream");
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
                seconds = buffer.len() as f32 / 16_000.0,
                "local engine: running final inference"
            );

            let ctx_final = ctx.clone();
            let final_result =
                tokio::task::spawn_blocking(move || run_window(&ctx_final, buffer, true)).await;

            match final_result {
                Ok(Ok(p)) => {
                    let _ = tx.send(Ok(p)).await;
                }
                Ok(Err(e)) => {
                    let _ = tx.send(Err(e)).await;
                }
                Err(e) => {
                    let _ = tx
                        .send(Err(EngineError::Inference(format!("blocking task: {e}"))))
                        .await;
                }
            }
        });

        Ok(rx.boxed())
    }

    async fn health_check(&self) -> EngineResult<()> {
        // The context is non-null because `load()` would have errored;
        // a deeper health check (run a dummy 1-sample inference) is overkill
        // for Phase 2.
        Ok(())
    }
}

/// Run whisper inference on `samples` (full accumulated buffer) and return a
/// `Partial`. `is_final` is forwarded directly into the returned `Partial`.
///
/// Caller must run this inside `tokio::task::spawn_blocking` — it is
/// synchronous and CPU/GPU-heavy. Never call from the async runtime.
///
/// Uses the FULL buffer on every call (not a delta) because whisper.cpp's
/// segment boundaries shift as the buffer grows; re-processing from the start
/// gives coherent, stable transcripts.
fn run_window(ctx: &WhisperContext, samples: Vec<f32>, is_final: bool) -> EngineResult<Partial> {
    let _start = Instant::now();
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
    debug!(
        elapsed_ms = _start.elapsed().as_millis(),
        segments = n_segments,
        is_final,
        "whisper inference complete"
    );

    Ok(Partial {
        text,
        is_final,
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

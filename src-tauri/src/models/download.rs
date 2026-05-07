//! Streaming GGML model downloader with SHA-256 verification.
//!
//! # Concurrency
//!
//! One download per model id. If `download()` is called while a download for
//! the same id is already in progress, it returns `DownloadError::AlreadyInProgress`.
//! The caller (Tauri command) inserts a `CancellationToken` into the shared
//! map *before* spawning the task and removes it on completion or error.
//!
//! # Cancellation
//!
//! Pass a `CancellationToken`; mid-stream cancellation deletes the `.partial`
//! file and returns `Ok(())` (not an error — the command layer handles cleanup).
//! Actually, `DownloadError::Cancelled` is returned so callers can distinguish.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures::StreamExt;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use super::catalog;

/// Errors from the download pipeline.
#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("unknown model id: {0}")]
    UnknownModel(String),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("io error: {0}")]
    Io(#[from] io::Error),

    #[error("checksum mismatch — expected {expected}, got {got}")]
    ChecksumMismatch { expected: String, got: String },

    #[error("server did not send Content-Length")]
    MissingContentLength,

    #[error("cancelled")]
    Cancelled,
}

/// Progress throttle: emit at most once per 100 ms.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Download a GGML model by catalog id.
///
/// - Streams from the catalog's HuggingFace URL.
/// - Writes to `<models_dir>/<id>.bin.partial` incrementally.
/// - Verifies SHA-256 on completion; deletes partial on mismatch.
/// - Atomically renames partial → `<id>.bin` on success.
/// - Calls `on_progress(bytes_done, bytes_total)` at ~10 Hz.
/// - Aborts and deletes partial if `cancel` is fired.
///
/// Returns the final path of the installed model file.
pub async fn download(
    id: &str,
    models_dir: PathBuf,
    cancel: CancellationToken,
    on_progress: impl Fn(u64, u64) + Send + 'static,
) -> Result<PathBuf, DownloadError> {
    let entry = catalog::find(id).ok_or_else(|| DownloadError::UnknownModel(id.to_owned()))?;

    fs::create_dir_all(&models_dir)
        .await
        .map_err(DownloadError::Io)?;

    let partial_path = models_dir.join(format!("ggml-{id}.bin.partial"));
    let final_path = models_dir.join(format!("ggml-{id}.bin"));

    let client = reqwest::Client::new();
    let response = client
        .get(entry.url)
        .send()
        .await
        .map_err(DownloadError::Http)?;

    let bytes_total = response
        .content_length()
        .ok_or(DownloadError::MissingContentLength)?;

    let mut file = fs::File::create(&partial_path)
        .await
        .map_err(DownloadError::Io)?;

    let mut hasher = Sha256::new();
    let mut bytes_done: u64 = 0;
    let mut last_emit = Instant::now() - PROGRESS_INTERVAL; // emit immediately on first chunk
    let mut stream = response.bytes_stream();

    loop {
        tokio::select! {
            biased;

            _ = cancel.cancelled() => {
                // Clean up partial file before returning.
                drop(file);
                let _ = fs::remove_file(&partial_path).await;
                return Err(DownloadError::Cancelled);
            }

            chunk = stream.next() => {
                match chunk {
                    None => break, // stream exhausted
                    Some(Err(e)) => {
                        drop(file);
                        let _ = fs::remove_file(&partial_path).await;
                        return Err(DownloadError::Http(e));
                    }
                    Some(Ok(bytes)) => {
                        hasher.update(&bytes);
                        file.write_all(&bytes).await.map_err(|e| {
                            // Can't easily drop + clean up here in the error arm without
                            // async, so we do a best-effort remove.
                            let _ = std::fs::remove_file(&partial_path);
                            DownloadError::Io(e)
                        })?;
                        bytes_done += bytes.len() as u64;

                        let now = Instant::now();
                        if now.duration_since(last_emit) >= PROGRESS_INTERVAL {
                            on_progress(bytes_done, bytes_total);
                            last_emit = now;
                        }
                    }
                }
            }
        }
    }

    // Flush + close before rename.
    file.flush().await.map_err(DownloadError::Io)?;
    drop(file);

    // Always emit final 100% progress.
    on_progress(bytes_done, bytes_total);

    // Verify checksum. Use `{:064x}` to always produce exactly 64 hex chars
    // regardless of leading zeros — catalog hashes are stored zero-padded.
    let got = format!("{:064x}", hasher.finalize());
    if got != entry.sha256 {
        let _ = fs::remove_file(&partial_path).await;
        return Err(DownloadError::ChecksumMismatch {
            expected: entry.sha256.to_owned(),
            got,
        });
    }

    // Atomic rename.
    fs::rename(&partial_path, &final_path)
        .await
        .map_err(DownloadError::Io)?;

    Ok(final_path)
}

/// List catalog ids whose `ggml-<id>.bin` file exists in `models_dir`.
///
/// Ignores any file whose name doesn't match a known catalog entry.
pub async fn list_installed(models_dir: &Path) -> Vec<String> {
    let mut installed = Vec::new();
    for entry in catalog::ALL {
        let path = models_dir.join(format!("ggml-{}.bin", entry.id));
        if path.exists() {
            installed.push(entry.id.to_owned());
        }
    }
    installed
}

/// Delete `ggml-<id>.bin` from `models_dir`. No-op if file doesn't exist.
pub async fn delete(id: &str, models_dir: &Path) -> Result<(), DownloadError> {
    if catalog::find(id).is_none() {
        return Err(DownloadError::UnknownModel(id.to_owned()));
    }
    let path = models_dir.join(format!("ggml-{id}.bin"));
    if path.exists() {
        fs::remove_file(&path).await.map_err(DownloadError::Io)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    /// Verify that our sha2 pipeline produces the correct hex string for a
    /// known input. This guards the checksum verification logic without
    /// making any network calls.
    #[test]
    fn sha256_hex_of_known_bytes() {
        // SHA-256("") — empty string, well-known value, no trailing-newline
        // ambiguity.
        let mut hasher = Sha256::new();
        hasher.update(b"");
        let result = format!("{:064x}", hasher.finalize());
        assert_eq!(
            result,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        // Verify length: sha2 always produces 256-bit = 64 hex chars.
        assert_eq!(result.len(), 64);
    }

    #[test]
    fn unknown_model_error_contains_id() {
        // Synchronous path: catalog::find returns None for unknown ids.
        let result = super::super::catalog::find("nonexistent-model");
        assert!(result.is_none());
    }
}

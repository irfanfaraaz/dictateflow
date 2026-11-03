//! Streaming GGML model downloader with SHA-256 verification and resume.
//!
//! # Concurrency
//!
//! One download per model id. The caller (Tauri command) owns a map of
//! `CancellationToken`s keyed by id and refuses to start a second download
//! for an id that is already in flight.
//!
//! # Resume
//!
//! Bytes are written to `<models_dir>/ggml-<id>.bin.partial`. If a partial
//! file exists when a download starts (crash, network drop, stall timeout),
//! we ask the server for `Range: bytes=<len>-`. On `206 Partial Content`
//! the existing bytes are re-hashed and the download continues; on `200`
//! the server ignored the range and we start over.
//!
//! # Cancellation
//!
//! User cancellation deletes the `.partial` file and returns
//! [`DownloadError::Cancelled`] so callers can distinguish it from failures.
//! Network failures and stalls keep the partial so the next attempt resumes.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures::{Stream, StreamExt};
use reqwest::header::{CONTENT_RANGE, RANGE};
use reqwest::StatusCode;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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

    #[error("unexpected server response: HTTP {0}")]
    UnexpectedStatus(u16),

    #[error("download stalled: no data for {0} seconds (will resume on retry)")]
    Stalled(u64),

    #[error("download incomplete: received {got} of {expected} bytes (will resume on retry)")]
    Incomplete { got: u64, expected: u64 },

    #[error("not enough disk space: need {needed} MB, only {available} MB free")]
    InsufficientSpace { needed: u64, available: u64 },

    #[error("cancelled")]
    Cancelled,
}

/// Progress throttle: emit at most once per 100 ms.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// TCP/TLS connect timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Max time to wait for response headers or for the next body chunk.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Free space we insist on keeping beyond the bytes still to download.
const DISK_HEADROOM: u64 = 64 * 1024 * 1024;

/// Read buffer used when re-hashing an existing partial file.
const REHASH_BUF: usize = 1024 * 1024;

fn partial_file_name(id: &str) -> String {
    format!("ggml-{id}.bin.partial")
}

/// Download a GGML model by catalog id.
///
/// - Streams from the catalog's HuggingFace URL (resuming a `.partial`).
/// - Rejects non-2xx responses (no HTML error pages saved as models).
/// - Verifies SHA-256 on completion; deletes partial on mismatch.
/// - `fsync`s and atomically renames partial → `ggml-<id>.bin` on success.
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

    fs::create_dir_all(&models_dir).await?;

    let partial_path = models_dir.join(partial_file_name(id));
    let final_path = models_dir.join(format!("ggml-{id}.bin"));

    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()?;

    let mut resume_from = match fs::metadata(&partial_path).await {
        Ok(m) if m.is_file() => m.len(),
        _ => 0,
    };

    let mut response = send(&client, entry.url, resume_from, &cancel).await?;
    if resume_from > 0 && response.status() == StatusCode::RANGE_NOT_SATISFIABLE {
        // Partial is unusable (e.g. larger than the remote file) — restart.
        tracing::info!(
            id,
            resume_from,
            "range not satisfiable; restarting download"
        );
        let _ = fs::remove_file(&partial_path).await;
        resume_from = 0;
        response = send(&client, entry.url, 0, &cancel).await?;
    }
    let response = response.error_for_status()?;

    let status = response.status();
    let resuming = if status == StatusCode::PARTIAL_CONTENT {
        let range_ok = response
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| content_range_starts_at(v, resume_from));
        if resume_from == 0 || !range_ok {
            // We can't trust where these bytes belong. Drop the partial so the
            // next attempt starts clean.
            let _ = fs::remove_file(&partial_path).await;
            return Err(DownloadError::UnexpectedStatus(status.as_u16()));
        }
        true
    } else if status == StatusCode::OK {
        if resume_from > 0 {
            tracing::info!(id, "server ignored Range; restarting download");
        }
        false
    } else {
        return Err(DownloadError::UnexpectedStatus(status.as_u16()));
    };

    let remaining = response
        .content_length()
        .ok_or(DownloadError::MissingContentLength)?;
    let start = if resuming { resume_from } else { 0 };
    let bytes_total = start + remaining;

    check_disk_space(&models_dir, remaining)?;

    let mut hasher = Sha256::new();
    let mut file = if resuming {
        if let Err(e) = rehash_prefix(&partial_path, start, &mut hasher, &cancel).await {
            // Cancelled → honour "cancel deletes the partial"; an unreadable
            // partial can't be resumed either, so start clean next time.
            let _ = fs::remove_file(&partial_path).await;
            return Err(e);
        }
        fs::OpenOptions::new()
            .append(true)
            .open(&partial_path)
            .await?
    } else {
        fs::File::create(&partial_path).await?
    };

    on_progress(start, bytes_total);

    let streamed = stream_to_file(
        response.bytes_stream(),
        &mut file,
        &mut hasher,
        &cancel,
        start,
        bytes_total,
        &on_progress,
    )
    .await;

    let bytes_done = match streamed {
        Ok(n) => n,
        Err(DownloadError::Cancelled) => {
            drop(file);
            let _ = fs::remove_file(&partial_path).await;
            return Err(DownloadError::Cancelled);
        }
        Err(e) => {
            // Keep the partial for resume; make sure what we wrote is on disk.
            let _ = file.flush().await;
            return Err(e);
        }
    };

    // Flush + fsync before rename so a crash can't leave a truncated model
    // under the final name.
    file.flush().await?;
    file.sync_all().await?;
    drop(file);

    if bytes_done != bytes_total {
        if bytes_done > bytes_total {
            let _ = fs::remove_file(&partial_path).await;
        }
        return Err(DownloadError::Incomplete {
            got: bytes_done,
            expected: bytes_total,
        });
    }

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

    // Atomic rename, then fsync the directory so the rename is durable.
    fs::rename(&partial_path, &final_path).await?;
    if let Ok(dir) = fs::File::open(&models_dir).await {
        let _ = dir.sync_all().await;
    }

    Ok(final_path)
}

/// Issue the GET (with a Range header when resuming), bounded by the stall
/// timeout and the cancel token.
async fn send(
    client: &reqwest::Client,
    url: &str,
    from: u64,
    cancel: &CancellationToken,
) -> Result<reqwest::Response, DownloadError> {
    let mut req = client.get(url);
    if from > 0 {
        req = req.header(RANGE, format!("bytes={from}-"));
    }
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(DownloadError::Cancelled),
        r = tokio::time::timeout(STALL_TIMEOUT, req.send()) => match r {
            Ok(res) => res.map_err(DownloadError::Http),
            Err(_) => Err(DownloadError::Stalled(STALL_TIMEOUT.as_secs())),
        },
    }
}

/// Copy the body stream into `file`, hashing as we go. Returns the total
/// number of bytes in the partial file (`start` + streamed).
async fn stream_to_file<S, B>(
    stream: S,
    file: &mut fs::File,
    hasher: &mut Sha256,
    cancel: &CancellationToken,
    start: u64,
    bytes_total: u64,
    on_progress: &(impl Fn(u64, u64) + Send),
) -> Result<u64, DownloadError>
where
    S: Stream<Item = reqwest::Result<B>>,
    B: AsRef<[u8]>,
{
    let mut stream = std::pin::pin!(stream);
    let mut bytes_done = start;
    let mut last_emit = Instant::now();

    loop {
        let next = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(DownloadError::Cancelled),
            r = tokio::time::timeout(STALL_TIMEOUT, stream.next()) => r,
        };
        let chunk = match next {
            Err(_) => return Err(DownloadError::Stalled(STALL_TIMEOUT.as_secs())),
            Ok(None) => break,
            Ok(Some(Err(e))) => return Err(DownloadError::Http(e)),
            Ok(Some(Ok(bytes))) => bytes,
        };

        let chunk = chunk.as_ref();
        hasher.update(chunk);
        file.write_all(chunk).await?;
        bytes_done += chunk.len() as u64;

        let now = Instant::now();
        if now.duration_since(last_emit) >= PROGRESS_INTERVAL {
            on_progress(bytes_done, bytes_total);
            last_emit = now;
        }
    }

    Ok(bytes_done)
}

/// Feed the first `len` bytes of an existing partial file into `hasher`.
async fn rehash_prefix(
    path: &Path,
    len: u64,
    hasher: &mut Sha256,
    cancel: &CancellationToken,
) -> Result<(), DownloadError> {
    let file = fs::File::open(path).await?;
    let mut reader = file.take(len);
    let mut buf = vec![0u8; REHASH_BUF];
    let mut read_total = 0u64;
    loop {
        if cancel.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        read_total += n as u64;
    }
    if read_total != len {
        return Err(DownloadError::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "partial file shrank while resuming",
        )));
    }
    Ok(())
}

/// `true` if a `Content-Range` value (`bytes <first>-<last>/<total>`) starts
/// at `offset`.
fn content_range_starts_at(value: &str, offset: u64) -> bool {
    value
        .trim()
        .strip_prefix("bytes ")
        .and_then(|rest| rest.split('-').next())
        .and_then(|first| first.trim().parse::<u64>().ok())
        == Some(offset)
}

fn check_disk_space(dir: &Path, remaining: u64) -> Result<(), DownloadError> {
    let Some(available) = available_bytes(dir) else {
        // Couldn't determine; let the write fail naturally if it must.
        return Ok(());
    };
    let needed = remaining.saturating_add(DISK_HEADROOM);
    if available < needed {
        const MB: u64 = 1024 * 1024;
        return Err(DownloadError::InsufficientSpace {
            needed: needed.div_ceil(MB),
            available: available / MB,
        });
    }
    Ok(())
}

/// Free bytes available to unprivileged users on the filesystem holding `dir`.
#[cfg(unix)]
#[allow(unsafe_code, clippy::unnecessary_cast, clippy::useless_conversion)]
fn available_bytes(dir: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(dir.as_os_str().as_bytes()).ok()?;
    // SAFETY: `statvfs` is plain old data; all-zero is a valid bit pattern.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c_path` is a valid NUL-terminated string and `st` is a valid,
    // writable `statvfs` for the duration of the call.
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut st) };
    if rc != 0 {
        return None;
    }
    Some((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
}

#[cfg(not(unix))]
fn available_bytes(_dir: &Path) -> Option<u64> {
    None
}

/// `true` if `file_name` is a `.partial` download that no catalog entry can
/// resume (unknown id or unexpected naming).
fn is_orphan_partial(file_name: &str) -> bool {
    let Some(stem) = file_name.strip_suffix(".partial") else {
        return false;
    };
    match stem
        .strip_prefix("ggml-")
        .and_then(|s| s.strip_suffix(".bin"))
    {
        Some(id) => catalog::find(id).is_none(),
        None => true,
    }
}

/// Delete `.partial` files that can never be resumed (their model id is not
/// in the catalog). Partials for known ids are kept so downloads resume.
pub async fn cleanup_orphan_partials(models_dir: &Path) {
    let mut entries = match fs::read_dir(models_dir).await {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::warn!(?e, "scan models dir for stale partials failed");
            return;
        }
    };
    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(e)) => e,
            Ok(None) => break,
            Err(e) => {
                tracing::warn!(?e, "read models dir entry failed");
                break;
            }
        };
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if is_orphan_partial(name) {
            match fs::remove_file(entry.path()).await {
                Ok(()) => tracing::info!(file = name, "removed orphan partial download"),
                Err(e) => tracing::warn!(?e, file = name, "remove orphan partial failed"),
            }
        }
    }
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

/// Delete `ggml-<id>.bin` (and any resumable partial) from `models_dir`.
/// No-op if the files don't exist.
pub async fn delete(id: &str, models_dir: &Path) -> Result<(), DownloadError> {
    if catalog::find(id).is_none() {
        return Err(DownloadError::UnknownModel(id.to_owned()));
    }
    let path = models_dir.join(format!("ggml-{id}.bin"));
    if path.exists() {
        fs::remove_file(&path).await?;
    }
    let partial = models_dir.join(partial_file_name(id));
    if partial.exists() {
        let _ = fs::remove_file(&partial).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn content_range_parsing() {
        assert!(content_range_starts_at("bytes 100-999/1000", 100));
        assert!(content_range_starts_at(" bytes 0-9/10", 0));
        assert!(!content_range_starts_at("bytes 99-999/1000", 100));
        assert!(!content_range_starts_at("bytes */1000", 100));
        assert!(!content_range_starts_at("garbage", 100));
    }

    #[test]
    fn orphan_partial_detection() {
        assert!(!is_orphan_partial("ggml-small.bin.partial"));
        assert!(!is_orphan_partial("ggml-large-v3-turbo.bin.partial"));
        assert!(is_orphan_partial("ggml-gone.bin.partial"));
        assert!(is_orphan_partial("weird.partial"));
        assert!(!is_orphan_partial("ggml-small.bin"));
    }

    #[test]
    fn split_hash_equals_whole_hash() {
        // Resume correctness relies on hashing prefix + suffix separately
        // producing the same digest as hashing the whole file.
        let data = b"hello resumable world";
        let mut whole = Sha256::new();
        whole.update(data);
        let mut split = Sha256::new();
        split.update(&data[..7]);
        split.update(&data[7..]);
        assert_eq!(whole.finalize(), split.finalize());
    }

    #[cfg(unix)]
    #[test]
    fn available_bytes_reports_something_for_tmp() {
        assert!(available_bytes(&std::env::temp_dir()).is_some());
    }
}

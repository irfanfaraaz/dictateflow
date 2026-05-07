//! Integration test: feed JFK's "ask not what your country can do for you"
//! sample WAV through the LocalEngine pipeline and assert the transcript
//! contains a known substring.
//!
//! # Why `#[ignore]` by default
//!
//! Running this test requires `ggml-small.bin` (~466 MB) to be on disk in
//! `~/Library/Application Support/com.wispertype.app/models/`. CI cannot
//! download a 466 MB model on every run, and we never commit the model
//! itself. To enable the test locally:
//!
//! ```bash
//! ./scripts/download-model.sh
//! cargo test --manifest-path src-tauri/Cargo.toml --test local_engine -- --ignored
//! ```
//!
//! # Fixture provenance
//!
//! `tests/fixtures/jfk.wav` is the canonical whisper.cpp sample (16 kHz mono
//! PCM s16le, ~330 KB). SHA-256:
//! `59dfb9a4acb36fe2a2affc14bacbee2920ff435cb13cc314a08c13f66ba7860e`.
//! Source: <https://github.com/ggerganov/whisper.cpp/raw/master/samples/jfk.wav>.

use std::path::PathBuf;

use futures::StreamExt;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use wispertype_lib::__test_only::{AudioChunk, Engine, LocalEngine};

const JFK_WAV: &[u8] = include_bytes!("fixtures/jfk.wav");
const JFK_WAV_SHA256: &str = "59dfb9a4acb36fe2a2affc14bacbee2920ff435cb13cc314a08c13f66ba7860e";

fn dev_model_path() -> PathBuf {
    let home = std::env::var_os("HOME").expect("HOME must be set");
    PathBuf::from(home).join("Library/Application Support/com.wispertype.app/models/ggml-small.bin")
}

/// Decode 16-bit PCM mono @ 16 kHz from the fixture WAV into f32 in [-1, 1].
fn decode_jfk_samples() -> Vec<f32> {
    let cursor = std::io::Cursor::new(JFK_WAV);
    let mut reader = hound::WavReader::new(cursor).expect("decode jfk.wav");
    let spec = reader.spec();
    assert_eq!(spec.sample_rate, 16_000, "jfk.wav must be 16 kHz");
    assert_eq!(spec.channels, 1, "jfk.wav must be mono");
    assert_eq!(spec.bits_per_sample, 16, "jfk.wav must be 16-bit");
    reader
        .samples::<i16>()
        .map(|r| r.expect("decode sample") as f32 / i16::MAX as f32)
        .collect()
}

#[test]
fn fixture_checksum_matches() {
    let mut hasher = Sha256::new();
    hasher.update(JFK_WAV);
    let got = hex::encode(hasher.finalize());
    // We hex-encode by hand to keep `hex` out of dev-deps.
    assert_eq!(
        got, JFK_WAV_SHA256,
        "jfk.wav fixture has been modified; re-download from whisper.cpp samples/"
    );
}

mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let bytes = bytes.as_ref();
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            s.push(HEX[(b >> 4) as usize] as char);
            s.push(HEX[(b & 0x0f) as usize] as char);
        }
        s
    }
}

/// Streaming test: feed audio in 250 ms increments and assert that at least
/// one intermediate (`is_final = false`) partial is emitted before the final.
///
/// Ignored for the same reason as `local_engine_transcribes_jfk`: requires the
/// downloaded model. Run with `--ignored` locally.
#[tokio::test]
#[ignore = "requires ggml-small.bin in ~/Library/Application Support/com.wispertype.app/models/ — see scripts/download-model.sh"]
async fn streaming_emits_intermediate_partials_before_final() {
    let model = dev_model_path();
    if !model.exists() {
        panic!(
            "model missing: {} — run scripts/download-model.sh",
            model.display()
        );
    }

    let engine = LocalEngine::load(&model).expect("load engine");

    let (tx, rx) = mpsc::channel::<AudioChunk>(64);
    let cancel = CancellationToken::new();

    // Feed audio in 250 ms increments (4_000 samples) over a background task.
    // The streaming engine fires an intermediate inference after every 16_000
    // samples (= 1 s), so after 4 chunks (1 s) we should see a partial.
    let samples = decode_jfk_samples();
    let tx_for_feeder = tx.clone();
    tokio::spawn(async move {
        const CHUNK_SAMPLES: usize = 16_000 / 4; // 250 ms at 16 kHz
        for chunk in samples.chunks(CHUNK_SAMPLES) {
            if tx_for_feeder
                .send(AudioChunk {
                    samples: chunk.to_vec(),
                })
                .await
                .is_err()
            {
                break;
            }
            // Simulate real-time pacing so the engine has time to fire windows.
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        drop(tx_for_feeder);
    });
    // Drop the original tx so the channel closes after the feeder drops its clone.
    drop(tx);

    let mut stream = engine.stream(rx, cancel).await.expect("engine.stream");

    let mut saw_intermediate = false;
    let mut final_text = String::new();

    while let Some(item) = stream.next().await {
        let p = item.expect("partial");
        if p.is_final {
            final_text = p.text;
            break;
        } else {
            saw_intermediate = true;
        }
    }

    assert!(
        saw_intermediate,
        "expected at least one is_final=false partial before the final"
    );

    let lower = final_text.to_lowercase();
    assert!(
        lower.contains("country") || lower.contains("ask") || lower.contains("americans"),
        "transcript missing expected keywords; got: {final_text:?}"
    );
}

#[tokio::test]
#[ignore = "requires ggml-small.bin in ~/Library/Application Support/com.wispertype.app/models/ — see scripts/download-model.sh"]
async fn local_engine_transcribes_jfk() {
    let model = dev_model_path();
    if !model.exists() {
        panic!(
            "model missing: {} — run scripts/download-model.sh",
            model.display()
        );
    }

    let engine = LocalEngine::load(&model).expect("load engine");

    let (tx, rx) = mpsc::channel::<AudioChunk>(16);
    let cancel = CancellationToken::new();

    // Feed the entire fixture as one chunk; the batch engine will accumulate
    // it, see the sender drop, and run a single inference.
    let samples = decode_jfk_samples();
    tx.send(AudioChunk { samples }).await.expect("send chunk");
    drop(tx);

    let mut stream = engine.stream(rx, cancel).await.expect("engine.stream");
    let mut transcript = String::new();
    while let Some(item) = stream.next().await {
        let p = item.expect("partial");
        if p.is_final {
            transcript = p.text;
            break;
        }
    }

    let lower = transcript.to_lowercase();
    assert!(
        lower.contains("country") || lower.contains("ask") || lower.contains("americans"),
        "transcript missing expected keywords; got: {transcript:?}"
    );
}

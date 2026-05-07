//! ElevenLabs Scribe v2 realtime engine.
//!
//! Streams 16 kHz mono PCM to ElevenLabs over a single WebSocket session and
//! relays partial / final transcripts back through the [`Engine`] trait's
//! `Stream<Result<Partial, _>>` interface.
//!
//! # Wire shape
//!
//! - Connect: `wss://api.elevenlabs.io/v1/speech-to-text/realtime?model_id=scribe_v2_realtime`
//!   with the `xi-api-key: <user_key>` header on the upgrade request. The
//!   `model_id` query parameter is required — the handshake returns 403
//!   without it.
//! - Audio out: text frames containing JSON
//!   `{"message_type":"input_audio_chunk","audio_base_64":"<b64>","commit":bool}`.
//!   The audio bytes are 16 kHz mono little-endian 16-bit PCM (default
//!   `audio_format=pcm_16000`), base64-encoded into the JSON payload.
//!   Raw binary frames are rejected.
//! - Transcripts in: text frames discriminated by `message_type`:
//!   `partial_transcript` (interim) and `committed_transcript` (final).
//!   `session_started` and `scribe_*_error` events are dropped at
//!   [`parse_partial`] today — extend if richer error surfacing is needed.
//! - Stop: send a final `input_audio_chunk` with `commit: true` (forces the
//!   server to emit `committed_transcript`), then a WebSocket Close frame.
//!
//! # No reconnect
//!
//! On WebSocket failure we surface the error and end the stream. The next
//! dictation press starts fresh — simpler than coordinating audio buffer
//! replay across a reconnect, and matches the user-facing model where the
//! pill represents one in-flight session.

#![allow(dead_code)] // surfaced in Phase 7.3 via the engine factory

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use futures::stream::BoxStream;
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::http::Request;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use tracing::{debug, info, warn};

use super::{AudioChunk, Engine, EngineError, EngineResult, Partial};

/// Default Scribe v2 realtime streaming endpoint.
///
/// `model_id=scribe_v2_realtime` is required as a query parameter — the
/// handshake returns 403 without it. Tests can point at a local mock via
/// [`ScribeEngine::with_url`].
pub const SCRIBE_WS_URL: &str =
    "wss://api.elevenlabs.io/v1/speech-to-text/realtime?model_id=scribe_v2_realtime";

/// WebSocket-backed realtime engine for ElevenLabs Scribe v2.
pub struct ScribeEngine {
    api_key: String,
    url: String,
}

impl ScribeEngine {
    /// Build a ScribeEngine pointing at the production endpoint.
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            url: SCRIBE_WS_URL.to_string(),
        }
    }

    /// Build a ScribeEngine pointing at an arbitrary URL (mock servers, test
    /// fixtures, or a future endpoint rename).
    pub fn with_url(api_key: String, url: String) -> Self {
        Self { api_key, url }
    }
}

#[async_trait]
impl Engine for ScribeEngine {
    async fn stream(
        &self,
        mut audio_rx: mpsc::Receiver<AudioChunk>,
        cancel: CancellationToken,
    ) -> EngineResult<BoxStream<'static, EngineResult<Partial>>> {
        if self.api_key.trim().is_empty() {
            return Err(EngineError::MissingApiKey);
        }

        // Build the upgrade request manually so we can inject the
        // `xi-api-key` header — the simple `connect_async(url)` form has no
        // hook for custom headers.
        let req = Request::builder()
            .method("GET")
            .uri(&self.url)
            .header(
                "Host",
                host_from_url(&self.url).unwrap_or("api.elevenlabs.io"),
            )
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", generate_key())
            .header("xi-api-key", &self.api_key)
            .body(())
            .map_err(|e| EngineError::Inference(format!("ws request build: {e}")))?;

        let (ws_stream, _resp) = tokio_tungstenite::connect_async(req)
            .await
            .map_err(|e| EngineError::WebSocket(e.to_string()))?;
        info!("scribe: WebSocket connected");
        let (mut ws_tx, mut ws_rx) = ws_stream.split();

        let (partial_tx, partial_rx) = mpsc::channel::<EngineResult<Partial>>(64);

        tokio::spawn(async move {
            // Track whether the client has signalled end-of-audio so we can
            // stop driving the audio branch (otherwise `audio_rx.recv()`
            // returns `None` repeatedly and we busy-loop on a closed channel).
            let mut audio_done = false;
            let mut chunks_sent: u64 = 0;

            loop {
                tokio::select! {
                    biased;

                    _ = cancel.cancelled() => {
                        info!("scribe: cancelled after {} chunks", chunks_sent);
                        let _ = ws_tx.send(Message::Close(None)).await;
                        let _ = partial_tx.send(Err(EngineError::Cancelled)).await;
                        break;
                    }

                    chunk = audio_rx.recv(), if !audio_done => match chunk {
                        Some(AudioChunk { samples, sample_rate }) => {
                            chunks_sent += 1;
                            if chunks_sent == 1 {
                                info!(sample_rate, samples = samples.len(), "scribe: first audio chunk");
                            }
                            let frame = encode_audio_chunk(&samples, false, sample_rate);
                            if let Err(e) = ws_tx.send(Message::Text(frame)).await {
                                warn!(?e, "scribe: ws send failed after {} chunks", chunks_sent);
                                let _ = partial_tx
                                    .send(Err(EngineError::WebSocket(e.to_string())))
                                    .await;
                                break;
                            }
                        }
                        None => {
                            info!("scribe: audio EOF after {} chunks — sending commit", chunks_sent);
                            audio_done = true;
                            let frame = encode_audio_chunk(&[], true, 16_000);
                            let _ = ws_tx.send(Message::Text(frame)).await;
                        }
                    },

                    msg = ws_rx.next() => match msg {
                        Some(Ok(Message::Text(t))) => {
                            debug!("scribe: ws recv: {}", &t[..t.len().min(200)]);
                            if let Some(p) = parse_partial(&t) {
                                let is_final = p.is_final;
                                info!(
                                    is_final,
                                    text_len = p.text.len(),
                                    "scribe: transcript partial"
                                );
                                if partial_tx.send(Ok(p)).await.is_err() {
                                    break;
                                }
                                if is_final {
                                    let _ = ws_tx.send(Message::Close(None)).await;
                                    break;
                                }
                            }
                        }
                        Some(Ok(Message::Binary(_))) => continue,
                        Some(Ok(Message::Ping(p))) => {
                            let _ = ws_tx.send(Message::Pong(p)).await;
                        }
                        Some(Ok(Message::Pong(_))) => continue,
                        Some(Ok(Message::Frame(_))) => continue,
                        Some(Ok(Message::Close(_))) | None => {
                            info!("scribe: ws closed/ended after {} chunks", chunks_sent);
                            break;
                        }
                        Some(Err(e)) => {
                            warn!(?e, "scribe: ws error after {} chunks", chunks_sent);
                            let _ = partial_tx
                                .send(Err(EngineError::WebSocket(e.to_string())))
                                .await;
                            break;
                        }
                    },
                }
            }
        });

        let stream = futures::stream::unfold(partial_rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        });
        Ok(Box::pin(stream))
    }

    async fn health_check(&self) -> EngineResult<()> {
        // Phase 7.2 keeps this cheap: presence-only. Phase 7.3 may upgrade to
        // a real `/v1/user` GET so the settings UI can show "key valid".
        if self.api_key.trim().is_empty() {
            return Err(EngineError::MissingApiKey);
        }
        Ok(())
    }
}

/// Convert 16 kHz mono `f32` PCM in [-1.0, 1.0] to little-endian `i16` bytes.
///
/// Out-of-range samples are clamped — Whisper's capture path normalises but
/// streaming engines might receive a hot mic. We scale by `32767` (not
/// `32768`) to keep the positive and negative ranges symmetric.
fn pcm_f32_to_i16_le(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let clamped = s.clamp(-1.0, 1.0);
        let i = (clamped * 32767.0) as i16;
        out.extend_from_slice(&i.to_le_bytes());
    }
    out
}

/// Pull the host out of a `wss://host[:port]/path` URL for the upgrade
/// request's `Host` header. Falls back to `None` on malformed input so the
/// caller can substitute a sensible default rather than crash.
fn host_from_url(url: &str) -> Option<&str> {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let host_with_port = after_scheme.split('/').next()?;
    Some(host_with_port)
}

/// Encode one audio buffer as an `input_audio_chunk` JSON frame.
///
/// `commit: true` signals end-of-stream and forces the server to emit a
/// `committed_transcript`. We wrap the i16 LE PCM bytes in base64 because
/// the API takes JSON-only frames — raw binary frames are rejected.
fn encode_audio_chunk(samples: &[f32], commit: bool, sample_rate: u32) -> String {
    let pcm = pcm_f32_to_i16_le(samples);
    let b64 = BASE64.encode(&pcm);
    let payload = serde_json::json!({
        "message_type": "input_audio_chunk",
        "audio_base_64": b64,
        "commit": commit,
        "sample_rate": sample_rate,
    });
    payload.to_string()
}

/// Parse a Scribe transcript text frame into a [`Partial`].
///
/// The API discriminates events by the `message_type` field. We surface:
/// - `partial_transcript` → `Partial { is_final: false }`
/// - `committed_transcript` (and the *_with_timestamps variant) →
///   `Partial { is_final: true }`
///
/// `session_started` and pings are dropped (return `None`). All
/// `scribe_*_error` events are treated like committed text frames with the
/// error string as `text` so the dictation flow can surface them — but
/// callers should check upstream for these by the message_type if they want
/// distinct handling. For Phase 7 we keep the surface narrow and just
/// forward transcript text.
fn parse_partial(json: &str) -> Option<Partial> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let kind = v.get("message_type").and_then(|x| x.as_str())?;
    let (is_final, text) = match kind {
        "partial_transcript" => (false, v.get("text")?.as_str()?.to_string()),
        "committed_transcript" | "committed_transcript_with_timestamps" => {
            (true, v.get("text")?.as_str()?.to_string())
        }
        _ => return None,
    };
    Some(Partial {
        text,
        is_final,
        confidence: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_partial_partial_transcript() {
        let p =
            parse_partial(r#"{"message_type":"partial_transcript","text":"hi"}"#).expect("parse");
        assert_eq!(p.text, "hi");
        assert!(!p.is_final);
    }

    #[test]
    fn parse_partial_committed_transcript_marks_final() {
        let p = parse_partial(r#"{"message_type":"committed_transcript","text":"foo"}"#)
            .expect("parse");
        assert_eq!(p.text, "foo");
        assert!(p.is_final);
    }

    #[test]
    fn parse_partial_committed_with_timestamps_marks_final() {
        let p = parse_partial(
            r#"{"message_type":"committed_transcript_with_timestamps","text":"x","words":[]}"#,
        )
        .expect("parse");
        assert_eq!(p.text, "x");
        assert!(p.is_final);
    }

    #[test]
    fn parse_partial_session_started_returns_none() {
        assert!(parse_partial(r#"{"message_type":"session_started","session_id":"a"}"#).is_none());
    }

    #[test]
    fn parse_partial_unknown_type_returns_none() {
        assert!(parse_partial(r#"{"message_type":"weather_report","text":"sunny"}"#).is_none());
    }

    #[test]
    fn parse_partial_invalid_json_returns_none() {
        assert!(parse_partial("not-json").is_none());
    }

    #[test]
    fn encode_audio_chunk_shape_and_base64() {
        let frame = encode_audio_chunk(&[0.0_f32, 1.0, -1.0], false, 16_000);
        let v: serde_json::Value = serde_json::from_str(&frame).expect("valid json");
        assert_eq!(v["message_type"], "input_audio_chunk");
        assert_eq!(v["commit"], false);
        let b64 = v["audio_base_64"].as_str().expect("b64 string");
        let decoded = BASE64.decode(b64).expect("decodes");
        // 3 samples * 2 bytes each
        assert_eq!(decoded.len(), 6);
    }

    #[test]
    fn encode_audio_chunk_commit_flag_set_on_close() {
        let frame = encode_audio_chunk(&[], true, 16_000);
        let v: serde_json::Value = serde_json::from_str(&frame).expect("valid json");
        assert_eq!(v["commit"], true);
        assert_eq!(v["audio_base_64"], "");
    }

    #[test]
    fn pcm_f32_to_i16_le_endianness_and_clamp() {
        // 0.0 -> 0
        // 1.0 -> 32767  -> bytes [0xFF, 0x7F]   (little-endian)
        // -1.0 -> -32767 -> bytes [0x01, 0x80]
        let bytes = pcm_f32_to_i16_le(&[0.0, 1.0, -1.0]);
        assert_eq!(bytes, vec![0, 0, 0xFF, 0x7F, 0x01, 0x80]);
    }

    #[test]
    fn pcm_f32_to_i16_le_clamps_out_of_range() {
        let bytes = pcm_f32_to_i16_le(&[2.0, -2.0]);
        assert_eq!(bytes, vec![0xFF, 0x7F, 0x01, 0x80]);
    }

    #[test]
    fn host_from_url_extracts_host() {
        assert_eq!(
            host_from_url("wss://api.elevenlabs.io/v1/speech-to-text/stream"),
            Some("api.elevenlabs.io")
        );
        assert_eq!(
            host_from_url("wss://localhost:8080/x"),
            Some("localhost:8080")
        );
    }

    #[tokio::test]
    async fn health_check_rejects_empty_key() {
        let e = ScribeEngine::new(String::new());
        assert!(matches!(
            e.health_check().await,
            Err(EngineError::MissingApiKey)
        ));
    }

    #[tokio::test]
    async fn health_check_accepts_nonempty_key() {
        let e = ScribeEngine::new("xi-test-key".to_string());
        assert!(e.health_check().await.is_ok());
    }
}

//! ElevenLabs Scribe v2 realtime engine.
//!
//! Streams 16 kHz mono PCM to ElevenLabs over a single WebSocket session and
//! relays partial / final transcripts back through the [`Engine`] trait's
//! `Stream<Result<Partial, _>>` interface.
//!
//! # Wire shape
//!
//! - Connect: `wss://api.elevenlabs.io/v1/speech-to-text/realtime?model_id=scribe_v2_realtime`
//!   (plus `&language_code=xx` when the user picked a language) with the
//!   `xi-api-key: <user_key>` header on the upgrade request. The `model_id`
//!   query parameter is required — the handshake returns 403 without it. The
//!   connect is bounded by [`CONNECT_TIMEOUT`] and aborts on `cancel`.
//! - Audio out: text frames containing JSON
//!   `{"message_type":"input_audio_chunk","audio_base_64":"<b64>","commit":bool,"sample_rate":16000}`.
//!   The audio bytes are 16 kHz mono little-endian 16-bit PCM (the default
//!   `audio_format=pcm_16000`; the dictation audio pump always resamples to
//!   16 kHz, and any chunk at another rate is resampled here), base64-encoded
//!   into the JSON payload. Raw binary frames are rejected.
//! - Transcripts in: text frames discriminated by `message_type`:
//!   `partial_transcript` (interim) and `committed_transcript` (a finished
//!   segment). Committed segments accumulate; the session's final transcript
//!   is all segments joined with spaces, emitted once the end-of-audio commit
//!   has been answered. Any `*error*` message type ends the session with
//!   `Err(EngineError::WebSocket(..))`.
//! - Stop: send a final `input_audio_chunk` with `commit: true` (forces the
//!   server to emit `committed_transcript`), then a WebSocket Close frame
//!   once the answer arrives. If no answer arrives within
//!   [`FINAL_COMMIT_GRACE`], whatever text was received is used.
//!
//! # No reconnect
//!
//! On WebSocket failure we surface the error and end the stream. The next
//! dictation press starts fresh — simpler than coordinating audio buffer
//! replay across a reconnect, and matches the user-facing model where the
//! pill represents one in-flight session.

#![allow(dead_code)] // surfaced via the engine factory

use std::time::Duration;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use futures::stream::BoxStream;
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::http::Request;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_util::sync::CancellationToken;

use tracing::{debug, info, warn};

use super::transcript::truncate_on_char_boundary;
use super::{AudioChunk, Engine, EngineError, EngineResult, Partial};
use crate::audio::resample::Resampler;
use crate::audio::TARGET_SAMPLE_RATE;

/// Default Scribe v2 realtime streaming endpoint.
///
/// `model_id=scribe_v2_realtime` is required as a query parameter — the
/// handshake returns 403 without it. Tests can point at a local mock via
/// [`ScribeEngine::with_url`].
pub const SCRIBE_WS_URL: &str =
    "wss://api.elevenlabs.io/v1/speech-to-text/realtime?model_id=scribe_v2_realtime";

/// Upper bound on the WebSocket handshake (DNS + TCP + TLS + upgrade).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

/// How long to wait for the server to answer the end-of-audio commit before
/// finishing with whatever text has been received.
const FINAL_COMMIT_GRACE: Duration = Duration::from_secs(5);

/// WebSocket-backed realtime engine for ElevenLabs Scribe v2.
pub struct ScribeEngine {
    api_key: String,
    url: String,
    /// ISO language code sent as `language_code`; `None` = autodetect.
    language: Option<String>,
}

impl ScribeEngine {
    /// Build a ScribeEngine pointing at the production endpoint.
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            url: SCRIBE_WS_URL.to_string(),
            language: None,
        }
    }

    /// Build a ScribeEngine pointing at an arbitrary URL (mock servers, test
    /// fixtures, or a future endpoint rename).
    pub fn with_url(api_key: String, url: String) -> Self {
        Self {
            api_key,
            url,
            language: None,
        }
    }

    /// Force a transcription language (`None` = autodetect).
    pub fn with_language(mut self, language: Option<String>) -> Self {
        self.language = language;
        self
    }

    /// The URL actually dialled: base URL plus `language_code` if set.
    fn session_url(&self) -> String {
        build_session_url(&self.url, self.language.as_deref())
    }
}

/// Append `language_code` to `base` (if any), URL-encoding the value.
fn build_session_url(base: &str, language: Option<&str>) -> String {
    let Some(lang) = language else {
        return base.to_string();
    };
    match url::Url::parse(base) {
        Ok(mut u) => {
            u.query_pairs_mut().append_pair("language_code", lang);
            u.to_string()
        }
        Err(e) => {
            warn!(
                ?e,
                "scribe: could not parse endpoint URL; ignoring language"
            );
            base.to_string()
        }
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

        let url = self.session_url();
        // Build the upgrade request manually so we can inject the
        // `xi-api-key` header — the simple `connect_async(url)` form has no
        // hook for custom headers.
        let req = Request::builder()
            .method("GET")
            .uri(&url)
            .header("Host", host_from_url(&url).unwrap_or("api.elevenlabs.io"))
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", generate_key())
            .header("xi-api-key", &self.api_key)
            .body(())
            .map_err(|e| EngineError::Inference(format!("ws request build: {e}")))?;

        let connect = tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(req));
        let ws_stream = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(EngineError::Cancelled),
            res = connect => match res {
                Err(_) => {
                    return Err(EngineError::Connect(format!(
                        "timed out after {} s connecting to ElevenLabs",
                        CONNECT_TIMEOUT.as_secs()
                    )))
                }
                Ok(Err(e)) => return Err(map_connect_error(e)),
                Ok(Ok((ws, _resp))) => ws,
            },
        };
        info!("scribe: WebSocket connected");
        let (mut ws_tx, mut ws_rx) = ws_stream.split();

        let (partial_tx, partial_rx) = mpsc::channel::<EngineResult<Partial>>(64);

        tokio::spawn(async move {
            // Track whether the client has signalled end-of-audio so we can
            // stop driving the audio branch (otherwise `audio_rx.recv()`
            // returns `None` repeatedly and we busy-loop on a closed channel).
            let mut audio_done = false;
            let mut chunks_sent: u64 = 0;
            let mut session = TranscriptAccumulator::default();
            let mut resampler: Option<Resampler> = None;
            // Re-armed when the end-of-audio commit goes out.
            let mut finish_by = tokio::time::Instant::now() + Duration::from_secs(86_400);

            loop {
                tokio::select! {
                    biased;

                    _ = cancel.cancelled() => {
                        info!("scribe: cancelled after {} chunks", chunks_sent);
                        let _ = ws_tx.send(Message::Close(None)).await;
                        let _ = partial_tx.send(Err(EngineError::Cancelled)).await;
                        break;
                    }

                    _ = tokio::time::sleep_until(finish_by), if audio_done => {
                        warn!("scribe: no answer to final commit within grace period");
                        let _ = ws_tx.send(Message::Close(None)).await;
                        let item = if session.is_empty() {
                            Err(EngineError::WebSocket(
                                "no transcript received from ElevenLabs".into(),
                            ))
                        } else {
                            Ok(session.final_partial())
                        };
                        let _ = partial_tx.send(item).await;
                        break;
                    }

                    chunk = audio_rx.recv(), if !audio_done => match chunk {
                        Some(chunk) => {
                            chunks_sent += 1;
                            if chunks_sent == 1 {
                                info!(
                                    sample_rate = chunk.sample_rate,
                                    samples = chunk.samples.len(),
                                    "scribe: first audio chunk"
                                );
                            }
                            let samples = to_16k(&mut resampler, chunk);
                            if samples.is_empty() {
                                continue;
                            }
                            let frame = encode_audio_chunk(&samples, false, TARGET_SAMPLE_RATE);
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
                            finish_by = tokio::time::Instant::now() + FINAL_COMMIT_GRACE;
                            let frame = encode_audio_chunk(&[], true, TARGET_SAMPLE_RATE);
                            if let Err(e) = ws_tx.send(Message::Text(frame)).await {
                                warn!(?e, "scribe: commit send failed");
                                let _ = partial_tx
                                    .send(Err(EngineError::WebSocket(e.to_string())))
                                    .await;
                                break;
                            }
                        }
                    },

                    msg = ws_rx.next() => match msg {
                        Some(Ok(Message::Text(t))) => {
                            debug!("scribe: ws recv: {}", truncate_on_char_boundary(&t, 200));
                            match parse_message(&t) {
                                ScribeMessage::Partial(text) => {
                                    session.set_partial(text);
                                    if partial_tx.send(Ok(session.interim_partial())).await.is_err() {
                                        break;
                                    }
                                }
                                ScribeMessage::Committed(text) => {
                                    session.commit(text);
                                    info!(
                                        segments = session.segments.len(),
                                        audio_done,
                                        "scribe: committed segment"
                                    );
                                    if audio_done {
                                        let _ = partial_tx.send(Ok(session.final_partial())).await;
                                        let _ = ws_tx.send(Message::Close(None)).await;
                                        break;
                                    }
                                    if partial_tx.send(Ok(session.interim_partial())).await.is_err() {
                                        break;
                                    }
                                }
                                ScribeMessage::Error(msg) => {
                                    warn!(%msg, "scribe: server error");
                                    let _ = partial_tx.send(Err(EngineError::WebSocket(msg))).await;
                                    let _ = ws_tx.send(Message::Close(None)).await;
                                    break;
                                }
                                ScribeMessage::Ignored => {}
                            }
                        }
                        Some(Ok(Message::Binary(_))) => continue,
                        Some(Ok(Message::Ping(p))) => {
                            let _ = ws_tx.send(Message::Pong(p)).await;
                        }
                        Some(Ok(Message::Pong(_))) => continue,
                        Some(Ok(Message::Frame(_))) => continue,
                        Some(Ok(Message::Close(frame))) => {
                            let reason = frame
                                .map(|f| format!("code {} {}", u16::from(f.code), f.reason))
                                .unwrap_or_else(|| "no reason given".into());
                            info!(%reason, "scribe: server closed after {} chunks", chunks_sent);
                            let _ = partial_tx
                                .send(Err(EngineError::WebSocket(format!(
                                    "connection closed before final transcript ({reason})"
                                ))))
                                .await;
                            break;
                        }
                        None => {
                            info!("scribe: ws ended after {} chunks", chunks_sent);
                            let _ = partial_tx
                                .send(Err(EngineError::WebSocket(
                                    "connection ended before final transcript".into(),
                                )))
                                .await;
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
        // Presence-only. A real `/v1/user` GET could later let the settings
        // UI show "key valid".
        if self.api_key.trim().is_empty() {
            return Err(EngineError::MissingApiKey);
        }
        Ok(())
    }
}

/// Classify a handshake failure: unreachable host vs. server refusal.
fn map_connect_error(e: WsError) -> EngineError {
    match e {
        WsError::Http(resp) => {
            let status = resp.status().as_u16();
            if status == 401 || status == 403 {
                EngineError::WebSocket(format!("auth rejected (HTTP {status})"))
            } else if status == 429 {
                EngineError::WebSocket(format!("rate limited (HTTP {status})"))
            } else {
                EngineError::WebSocket(format!("handshake failed (HTTP {status})"))
            }
        }
        WsError::Io(io) => EngineError::Connect(io.to_string()),
        WsError::Tls(tls) => EngineError::Connect(tls.to_string()),
        WsError::Url(u) => EngineError::Connect(u.to_string()),
        other => EngineError::WebSocket(other.to_string()),
    }
}

/// Resample a chunk to 16 kHz if needed (the audio pump already does this;
/// this is a guard for other callers).
fn to_16k(resampler: &mut Option<Resampler>, chunk: AudioChunk) -> Vec<f32> {
    if chunk.sample_rate == TARGET_SAMPLE_RATE {
        return chunk.samples;
    }
    let needs_new = resampler
        .as_ref()
        .is_none_or(|r| r.in_rate() != chunk.sample_rate);
    if needs_new {
        warn!(
            sample_rate = chunk.sample_rate,
            "scribe: resampling non-16 kHz input"
        );
        *resampler = Some(Resampler::new(chunk.sample_rate, TARGET_SAMPLE_RATE));
    }
    let mut out = Vec::new();
    if let Some(r) = resampler.as_mut() {
        r.process(&chunk.samples, &mut out);
    }
    out
}

/// Committed segments + the in-progress partial for one session.
#[derive(Debug, Default)]
struct TranscriptAccumulator {
    segments: Vec<String>,
    partial: String,
}

impl TranscriptAccumulator {
    fn set_partial(&mut self, text: String) {
        self.partial = text;
    }

    fn commit(&mut self, text: String) {
        let t = text.trim();
        if !t.is_empty() {
            self.segments.push(t.to_string());
        }
        self.partial.clear();
    }

    fn is_empty(&self) -> bool {
        self.segments.is_empty() && self.partial.trim().is_empty()
    }

    fn joined(&self) -> String {
        let mut parts: Vec<&str> = self.segments.iter().map(String::as_str).collect();
        let p = self.partial.trim();
        if !p.is_empty() {
            parts.push(p);
        }
        parts.join(" ")
    }

    fn interim_partial(&self) -> Partial {
        Partial {
            text: self.joined(),
            is_final: false,
            confidence: None,
        }
    }

    fn final_partial(&self) -> Partial {
        Partial {
            text: self.joined(),
            is_final: true,
            confidence: None,
        }
    }
}

/// Convert 16 kHz mono `f32` PCM in [-1.0, 1.0] to little-endian `i16` bytes.
///
/// Out-of-range samples are clamped (a hot mic can exceed full scale). We
/// scale by `32767` (not `32768`) to keep the positive and negative ranges
/// symmetric.
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
    let host_with_port = after_scheme.split(['/', '?']).next()?;
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

/// A decoded server → client text frame.
#[derive(Debug, PartialEq, Eq)]
enum ScribeMessage {
    /// `partial_transcript` — interim text for the current segment.
    Partial(String),
    /// `committed_transcript` (or the `_with_timestamps` variant).
    Committed(String),
    /// Any `*error*` message type (auth, quota, throttling, input, …),
    /// formatted as `"<message_type>: <detail>"`.
    Error(String),
    /// `session_started`, unknown types, and unparseable frames.
    Ignored,
}

/// Parse a Scribe text frame.
fn parse_message(json: &str) -> ScribeMessage {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return ScribeMessage::Ignored;
    };
    let Some(kind) = v.get("message_type").and_then(|x| x.as_str()) else {
        return ScribeMessage::Ignored;
    };
    let text = || {
        v.get("text")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string()
    };
    match kind {
        "partial_transcript" => ScribeMessage::Partial(text()),
        "committed_transcript" | "committed_transcript_with_timestamps" => {
            ScribeMessage::Committed(text())
        }
        k if k.contains("error") => {
            let detail = ["error", "message", "detail"]
                .iter()
                .find_map(|key| v.get(*key).and_then(|x| x.as_str()))
                .unwrap_or("no details");
            ScribeMessage::Error(format!("{k}: {detail}"))
        }
        _ => ScribeMessage::Ignored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_partial_transcript() {
        assert_eq!(
            parse_message(r#"{"message_type":"partial_transcript","text":"hi"}"#),
            ScribeMessage::Partial("hi".into())
        );
    }

    #[test]
    fn parse_committed_transcript() {
        assert_eq!(
            parse_message(r#"{"message_type":"committed_transcript","text":"foo"}"#),
            ScribeMessage::Committed("foo".into())
        );
    }

    #[test]
    fn parse_committed_with_timestamps() {
        assert_eq!(
            parse_message(
                r#"{"message_type":"committed_transcript_with_timestamps","text":"x","words":[]}"#
            ),
            ScribeMessage::Committed("x".into())
        );
    }

    #[test]
    fn parse_error_messages_surface_detail() {
        assert_eq!(
            parse_message(r#"{"message_type":"scribe_auth_error","error":"invalid api key"}"#),
            ScribeMessage::Error("scribe_auth_error: invalid api key".into())
        );
        assert_eq!(
            parse_message(r#"{"message_type":"scribe_quota_exceeded_error"}"#),
            ScribeMessage::Error("scribe_quota_exceeded_error: no details".into())
        );
    }

    #[test]
    fn parse_session_started_is_ignored() {
        assert_eq!(
            parse_message(r#"{"message_type":"session_started","session_id":"a"}"#),
            ScribeMessage::Ignored
        );
    }

    #[test]
    fn parse_unknown_type_is_ignored() {
        assert_eq!(
            parse_message(r#"{"message_type":"weather_report","text":"sunny"}"#),
            ScribeMessage::Ignored
        );
    }

    #[test]
    fn parse_invalid_json_is_ignored() {
        assert_eq!(parse_message("not-json"), ScribeMessage::Ignored);
    }

    #[test]
    fn accumulator_joins_segments_with_spaces() {
        let mut acc = TranscriptAccumulator::default();
        assert!(acc.is_empty());
        acc.set_partial("hello wor".into());
        assert_eq!(acc.interim_partial().text, "hello wor");
        acc.commit("Hello world.".into());
        acc.set_partial("how are".into());
        assert_eq!(acc.interim_partial().text, "Hello world. how are");
        acc.commit(" How are you? ".into());
        acc.commit("   ".into());
        let f = acc.final_partial();
        assert!(f.is_final);
        assert_eq!(f.text, "Hello world. How are you?");
    }

    #[test]
    fn session_url_adds_language_code_only_when_set() {
        assert_eq!(build_session_url(SCRIBE_WS_URL, None), SCRIBE_WS_URL);
        let with = build_session_url(SCRIBE_WS_URL, Some("de"));
        assert!(with.starts_with(SCRIBE_WS_URL), "{with}");
        assert!(with.ends_with("&language_code=de"), "{with}");
    }

    #[test]
    fn encode_audio_chunk_shape_and_base64() {
        let frame = encode_audio_chunk(&[0.0_f32, 1.0, -1.0], false, 16_000);
        let v: serde_json::Value = serde_json::from_str(&frame).expect("valid json");
        assert_eq!(v["message_type"], "input_audio_chunk");
        assert_eq!(v["commit"], false);
        assert_eq!(v["sample_rate"], 16_000);
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
    fn to_16k_resamples_other_rates() {
        let mut rs = None;
        let same = to_16k(
            &mut rs,
            AudioChunk {
                samples: vec![0.0; 100],
                sample_rate: 16_000,
            },
        );
        assert_eq!(same.len(), 100);
        let out = to_16k(
            &mut rs,
            AudioChunk {
                samples: vec![0.0; 800],
                sample_rate: 8_000,
            },
        );
        assert!((1_590..=1_600).contains(&out.len()), "{}", out.len());
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
        assert_eq!(host_from_url("wss://h.example?x=1"), Some("h.example"));
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

    #[tokio::test]
    async fn connect_failure_maps_to_connect_error() {
        // Port 9 on localhost is (almost) never listening → immediate refusal.
        let e = ScribeEngine::with_url("k".into(), "ws://127.0.0.1:9/x".into());
        let (_tx, rx) = mpsc::channel(1);
        match e.stream(rx, CancellationToken::new()).await {
            Err(EngineError::Connect(_)) => {}
            Err(other) => panic!("expected Connect, got {other:?}"),
            Ok(_) => panic!("expected connect failure"),
        }
    }

    #[tokio::test]
    async fn cancel_aborts_connect() {
        // 10.255.255.1 is non-routable: the connect would hang until timeout.
        let e = ScribeEngine::with_url("k".into(), "ws://10.255.255.1:81/x".into());
        let (_tx, rx) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        cancel.cancel();
        match e.stream(rx, cancel).await {
            Err(EngineError::Cancelled) => {}
            Err(other) => panic!("expected Cancelled, got {other:?}"),
            Ok(_) => panic!("expected cancellation"),
        }
    }
}

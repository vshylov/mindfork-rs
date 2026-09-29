//! The speech client for the OpenAI `POST /v1/audio/speech` protocol. Serves
//! **three** modes: the OpenAI cloud, any third-party OpenAI-compatible TTS
//! server (Kokoro-FastAPI, speaches, LocalAI, …) and the OpenRouter gateway —
//! they differ in base URL, key, and how the response format is arrived at.
//! See docs/research/tts.md §3.1, §3.4 and docs/research/openrouter-mode.md
//! §4.4, §13.
//!
//! The first two **ask for one format** and know what comes back. The gateway
//! is some twenty models of a dozen vendors behind one route, and measured
//! (§4.4) no format is taken by all of them — `pcm` by 19 of 21, `mp3` by 18 —
//! while the rate is 24, 32 or 44.1 kHz and one model answers in stereo. So
//! there the format is **negotiated** and what came back is read from its
//! label:
//!
//! - `pcm` is asked first — raw samples need no decoder — and on a `400` that
//!   names `response_format` the other format is asked, once;
//! - the format that was answered is remembered per model ([`FormatMemo`]), so
//!   the refused request is paid once a session, not once a fragment;
//! - the rate and the channel count come from `Content-Type`
//!   (`audio/pcm;rate=44100;channels=1`), and the **label decides** what the
//!   body is: raw PCM may begin with `0xFFFF` — a sample of −1 — which is an
//!   MP3 frame sync to anything that sniffs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::{Context, Result};
use reqwest::StatusCode;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::{AudioClip, GatewaySpeech, TtsEngine, refusal};
use crate::shared::api::openai::attributed;

/// The ceiling on `input` length for OpenAI (characters) — a hard API limit.
const OPENAI_MAX_INPUT_CHARS: usize = 4096;
/// A conservative ceiling for a third-party server: every one has its own
/// limits, usually undocumented, and a short chunk also gives the first sound sooner.
const EXTERNAL_MAX_INPUT_CHARS: usize = 2000;

/// The gateway's ceiling, the one native Gemini has (fork F9). 4000
/// characters in one request are accepted (measured on two models), but not
/// every model streams — Gemini's first byte for 1500 characters came after
/// 28 s — so a shorter fragment is what gives the first sound sooner.
const GATEWAY_MAX_INPUT_CHARS: usize = 2000;

/// The sample rate of raw PCM from OpenAI (`response_format:"pcm"`): 24 kHz,
/// s16le, mono — documented, not reported in the response.
const OPENAI_PCM_RATE: u32 = 24_000;

/// What a raw answer of the gateway's is taken to be where its label names no
/// rate: the rate of 17 of the 19 models that answer in `pcm`, and OpenAI's
/// own. Every answer measured did name one; this is the fallback, and it is
/// logged when taken.
const UNLABELLED_PCM_RATE: u32 = 24_000;

/// The two formats the gateway's speech route takes. Its schema refuses every
/// other — `wav` is a `400` before any model is asked (measured, §13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayFormat {
    /// Raw signed 16-bit little-endian samples.
    Pcm,
    Mp3,
}

impl GatewayFormat {
    /// The value of `response_format`.
    fn wire(self) -> &'static str {
        match self {
            GatewayFormat::Pcm => "pcm",
            GatewayFormat::Mp3 => "mp3",
        }
    }

    fn other(self) -> Self {
        match self {
            GatewayFormat::Pcm => GatewayFormat::Mp3,
            GatewayFormat::Mp3 => GatewayFormat::Pcm,
        }
    }
}

/// Which format each of the gateway's models was last answered in.
///
/// Lives as long as the session, outside the clients: a client is built per
/// `/tts` command from a snapshot of the settings, and a memo inside it would
/// pay the refused request again with every command. What it holds is what the
/// gateway **did**, so a model that changes its mind costs one refused request
/// and is remembered the new way.
#[derive(Debug, Default)]
pub struct FormatMemo(Mutex<HashMap<String, GatewayFormat>>);

impl FormatMemo {
    /// The format to ask `model` for first: the one it answered in, or `pcm`
    /// for a model nothing is known about.
    pub fn of(&self, model: &str) -> GatewayFormat {
        let known = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        known.get(model).copied().unwrap_or(GatewayFormat::Pcm)
    }

    fn keep(&self, model: &str, format: GatewayFormat) {
        let mut known = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        known.insert(model.to_string(), format);
    }
}

/// How the response format is arrived at.
enum Format {
    /// Asked for whatever the model: `pcm` of the OpenAI cloud (bypassing a
    /// decoder) / `wav` of a third-party server (the most portable — the only
    /// one some servers support).
    Fixed(&'static str),
    /// The gateway: asked for, and settled by the answer.
    Negotiated(Arc<FormatMemo>),
}

/// A `/v1/audio/speech` client.
pub struct OpenAiTts {
    http: reqwest::Client,
    /// The base URL with the `/v1` suffix.
    base_url: String,
    api_key: Option<String>,
    model: Option<String>,
    voice: Option<String>,
    /// Instructions on tone/language/speed (OpenAI cloud only).
    instructions: Option<String>,
    speed: f32,
    format: Format,
    /// Name the application to the gateway in the two headers every other
    /// request to it carries (fork F5). The gateway's switch, and nobody
    /// else's: `false` in the other two modes.
    attribution: bool,
    max_input_chars: usize,
}

/// What one request was answered with.
enum Answer {
    Audio {
        /// The answer's `Content-Type`, when it had one.
        label: Option<String>,
        bytes: Vec<u8>,
    },
    Refused {
        status: StatusCode,
        body: String,
    },
}

impl OpenAiTts {
    /// The OpenAI cloud: a key is required, we ask for raw PCM.
    pub fn cloud(
        base_url: String,
        api_key: String,
        model: String,
        voice: Option<String>,
        instructions: Option<String>,
        speed: f32,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: Some(api_key),
            model: Some(model),
            voice,
            instructions,
            speed,
            format: Format::Fixed("pcm"),
            attribution: false,
            max_input_chars: OPENAI_MAX_INPUT_CHARS,
        }
    }

    /// The OpenRouter gateway: a key and a model are required, the format is
    /// negotiated (the module's docs). `instructions` is not a field of this
    /// route — it is dropped there with a `200`, like any unknown key — so none
    /// is taken.
    pub fn gateway(
        base_url: String,
        api_key: String,
        model: String,
        voice: Option<String>,
        speed: f32,
        gateway: &GatewaySpeech,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: Some(api_key),
            model: Some(model),
            voice,
            instructions: None,
            speed,
            format: Format::Negotiated(gateway.formats.clone()),
            attribution: gateway.attribution,
            max_input_chars: GATEWAY_MAX_INPUT_CHARS,
        }
    }

    /// A third-party OpenAI-compatible server: everything but the URL is
    /// optional; we ask for `wav`. We don't send `instructions` — only the
    /// OpenAI cloud supports it.
    pub fn external(
        base_url: String,
        api_key: Option<String>,
        model: Option<String>,
        voice: Option<String>,
        speed: f32,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model,
            voice,
            instructions: None,
            speed,
            format: Format::Fixed("wav"),
            attribution: false,
            max_input_chars: EXTERNAL_MAX_INPUT_CHARS,
        }
    }

    /// The request body. An unset value isn't sent (`skip_serializing_if`):
    /// third-party servers support different field sets, and an extra field
    /// might be rejected.
    fn body<'a>(&'a self, text: &'a str, response_format: &'static str) -> SpeechRequest<'a> {
        SpeechRequest {
            model: self.model.as_deref(),
            input: text,
            voice: self.voice.as_deref(),
            instructions: self.instructions.as_deref(),
            response_format,
            // We only send speed when it differs from normal. For
            // `gpt-4o-mini-tts` the field is effectively ignored (a known
            // defect) — there speed is requested via words in `instructions`.
            speed: (self.speed != 1.0).then_some(self.speed),
        }
    }

    /// One request, in one format. A refusal is an answer, not an error: the
    /// gateway's is read before it is decided what it means.
    async fn ask(
        &self,
        text: &str,
        response_format: &'static str,
        cancel: &CancellationToken,
    ) -> Result<Answer> {
        let url = format!("{}/audio/speech", self.base_url);
        let mut rb = self.http.post(&url).json(&self.body(text, response_format));
        if let Some(key) = &self.api_key {
            rb = rb.bearer_auth(key);
        }
        if self.attribution {
            rb = attributed(rb);
        }
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => anyhow::bail!("speech synthesis cancelled"),
            r = rb.send() => r.with_context(|| format!("POST {url}"))?,
        };
        let status = resp.status();
        if !status.is_success() {
            let body = tokio::select! {
                biased;
                _ = cancel.cancelled() => anyhow::bail!("speech synthesis cancelled"),
                b = resp.text() => b.unwrap_or_default(),
            };
            return Ok(Answer::Refused { status, body });
        }
        let label = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let bytes = tokio::select! {
            biased;
            _ = cancel.cancelled() => anyhow::bail!("speech synthesis cancelled"),
            b = resp.bytes() => b.context("reading TTS audio response")?,
        };
        Ok(Answer::Audio {
            label,
            bytes: bytes.to_vec(),
        })
    }

    /// The gateway's road: the format the model is known by, and on a refusal
    /// that names the format — the other one, once.
    async fn negotiate(
        &self,
        memo: &FormatMemo,
        text: &str,
        cancel: &CancellationToken,
    ) -> Result<AudioClip> {
        let model = self.model.as_deref().unwrap_or_default();
        let first = memo.of(model);
        let (status, body) = match self.ask(text, first.wire(), cancel).await? {
            // Nothing to remember: `first` is what the memo said already, or
            // what it says of a model it knows nothing about.
            Answer::Audio { label, bytes } => {
                return Ok(labelled(label.as_deref(), first, bytes));
            }
            Answer::Refused { status, body } => (status, body),
        };
        if !names_the_format(status, &body) {
            return Err(gateway_refusal(status, &body));
        }
        let second = first.other();
        tracing::info!(
            model,
            refused = first.wire(),
            asking = second.wire(),
            "the gateway's model does not take this audio format"
        );
        match self.ask(text, second.wire(), cancel).await? {
            Answer::Audio { label, bytes } => {
                memo.keep(model, second);
                Ok(labelled(label.as_deref(), second, bytes))
            }
            // Refused both ways: the second refusal is the one that says what
            // is wrong now, and nothing is remembered.
            Answer::Refused { status, body } => Err(gateway_refusal(status, &body)),
        }
    }
}

/// Whether a refusal is about the format and nothing else. The gateway's two
/// are `400 "Gemini TTS only supports response_format=\"pcm\". Got \"mp3\"."`
/// and MiniMax's mirror of it; the test is the field's name, which both carry,
/// and not the sentence around it.
fn names_the_format(status: StatusCode, body: &str) -> bool {
    status == StatusCode::BAD_REQUEST && body.contains("response_format")
}

/// A refusal in the gateway's own words: `error.message` out of its JSON,
/// where there is one — *"An explicit voice is required for this TTS
/// provider."* reads better in the chat than the object around it.
fn gateway_refusal(status: StatusCode, body: &str) -> anyhow::Error {
    let said = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.pointer("/error/message")?.as_str().map(str::to_string))
        .filter(|m| !m.trim().is_empty());
    refusal("TTS", status, said.as_deref().unwrap_or(body))
}

/// An answer of the gateway's as a clip, by what its label says it is.
///
/// `audio/pcm` (and `audio/L16`, the same thing under its registered name) is
/// raw samples at the rate and the channel count the label names; anything
/// else is a container for the decoder. An answer with no label at all is
/// taken for what was asked.
fn labelled(label: Option<&str>, asked: GatewayFormat, bytes: Vec<u8>) -> AudioClip {
    let label = label.unwrap_or_default().to_ascii_lowercase();
    let mut parts = label.split(';').map(str::trim);
    let raw = match parts.next().unwrap_or_default() {
        "audio/pcm" | "audio/l16" => true,
        "" => asked == GatewayFormat::Pcm,
        _ => false,
    };
    if !raw {
        return AudioClip::Encoded(bytes);
    }
    let named = |name: &str| {
        parts.clone().find_map(|p| {
            p.strip_prefix(name)?
                .trim_start()
                .strip_prefix('=')?
                .trim()
                .parse()
                .ok()
        })
    };
    let sample_rate = named("rate").unwrap_or_else(|| {
        tracing::warn!(%label, "raw speech with no rate in its label — taken for 24 kHz");
        UNLABELLED_PCM_RATE
    });
    AudioClip::Pcm {
        sample_rate,
        channels: named("channels")
            .and_then(|n: u32| u16::try_from(n).ok())
            .unwrap_or(1),
        bytes,
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct SpeechRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<&'a str>,
    input: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    voice: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<&'a str>,
    response_format: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    speed: Option<f32>,
}

#[async_trait::async_trait]
impl TtsEngine for OpenAiTts {
    async fn synthesize(&self, text: &str, cancel: &CancellationToken) -> Result<AudioClip> {
        let asked = match &self.format {
            Format::Negotiated(memo) => return self.negotiate(memo, text, cancel).await,
            Format::Fixed(asked) => *asked,
        };
        match self.ask(text, asked, cancel).await? {
            Answer::Refused { status, body } => Err(refusal("TTS", status, &body)),
            Answer::Audio { bytes, .. } if asked == "pcm" => Ok(AudioClip::Pcm {
                sample_rate: OPENAI_PCM_RATE,
                channels: 1,
                bytes,
            }),
            Answer::Audio { bytes, .. } => Ok(AudioClip::Encoded(bytes)),
        }
    }

    fn max_input_chars(&self) -> usize {
        self.max_input_chars
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json_of(engine: &OpenAiTts, text: &str) -> serde_json::Value {
        let Format::Fixed(asked) = engine.format else {
            panic!("a client that asks for one format");
        };
        serde_json::to_value(engine.body(text, asked)).unwrap()
    }

    #[test]
    fn cloud_request_asks_for_raw_pcm_and_carries_instructions() {
        let e = OpenAiTts::cloud(
            "https://api.openai.com/v1".into(),
            "sk-x".into(),
            "gpt-4o-mini-tts".into(),
            Some("marin".into()),
            Some("говори по-русски, спокойно".into()),
            1.0,
        );
        let v = json_of(&e, "привет");
        assert_eq!(v["model"], "gpt-4o-mini-tts");
        assert_eq!(v["input"], "привет");
        assert_eq!(v["voice"], "marin");
        assert_eq!(v["response_format"], "pcm");
        assert_eq!(v["instructions"], "говори по-русски, спокойно");
        // A normal speed isn't sent at all.
        assert!(v.get("speed").is_none(), "speed=1.0 isn't sent: {v}");
        assert_eq!(e.max_input_chars(), OPENAI_MAX_INPUT_CHARS);
    }

    #[test]
    fn external_request_asks_for_wav_and_omits_unset_fields() {
        let e = OpenAiTts::external("http://127.0.0.1:8880/v1/".into(), None, None, None, 1.25);
        let v = json_of(&e, "текст");
        assert_eq!(v["response_format"], "wav");
        assert_eq!(v["speed"], 1.25);
        // Unset fields aren't sent: third-party servers have their own field sets.
        assert!(v.get("model").is_none(), "model isn't sent: {v}");
        assert!(v.get("voice").is_none(), "voice isn't sent: {v}");
        assert!(
            v.get("instructions").is_none(),
            "instructions — OpenAI cloud only: {v}"
        );
        // The URL's trailing slash is stripped (otherwise it would end up `//audio/speech`).
        assert_eq!(e.base_url, "http://127.0.0.1:8880/v1");
        assert_eq!(e.max_input_chars(), EXTERNAL_MAX_INPUT_CHARS);
    }

    /// A live OpenAI-cloud smoke: synthesizing a short phrase (no playback).
    /// Requires `MINDFORK_OPENAI_KEY`; silently skipped without it.
    #[tokio::test]
    #[ignore = "requires an OpenAI key (MINDFORK_OPENAI_KEY)"]
    async fn openai_synthesizes_russian_speech_live() {
        let Ok(key) = std::env::var("MINDFORK_OPENAI_KEY") else {
            eprintln!("MINDFORK_OPENAI_KEY not set — smoke skipped");
            return;
        };
        let e = OpenAiTts::cloud(
            "https://api.openai.com/v1".into(),
            key,
            std::env::var("MINDFORK_TTS_MODEL")
                .unwrap_or_else(|_| crate::shared::config::DEFAULT_TTS_OPENAI_MODEL.into()),
            Some(crate::shared::config::DEFAULT_TTS_OPENAI_VOICE.into()),
            Some("говори по-русски, спокойно и разборчиво".into()),
            1.0,
        );
        let clip = e
            .synthesize(
                "Проверка озвучивания. Латинская вставка: API, JSON.",
                &CancellationToken::new(),
            )
            .await
            .expect("synthesis should succeed");
        match clip {
            AudioClip::Pcm {
                sample_rate,
                channels,
                bytes,
            } => {
                assert_eq!(sample_rate, OPENAI_PCM_RATE);
                assert_eq!(channels, 1);
                eprintln!("received PCM: {} bytes", bytes.len());
                assert!(bytes.len() > 10_000, "the clip is suspiciously short");
            }
            AudioClip::Encoded(bytes) => {
                panic!(
                    "the OpenAI cloud should return raw PCM, got a container ({} bytes)",
                    bytes.len()
                )
            }
        }
    }

    /// A live smoke of a third-party OpenAI-compatible server (Kokoro-FastAPI,
    /// speaches, LocalAI…). Requires `MINDFORK_TTS_URL` (e.g. `http://127.0.0.1:8880/v1`).
    #[tokio::test]
    #[ignore = "requires a local TTS server (MINDFORK_TTS_URL)"]
    async fn external_server_synthesizes_live() {
        let Ok(url) = std::env::var("MINDFORK_TTS_URL") else {
            eprintln!("MINDFORK_TTS_URL not set — smoke skipped");
            return;
        };
        let e = OpenAiTts::external(
            url,
            None,
            std::env::var("MINDFORK_TTS_MODEL").ok(),
            std::env::var("MINDFORK_TTS_VOICE").ok(),
            1.0,
        );
        let clip = e
            .synthesize("Проверка озвучивания.", &CancellationToken::new())
            .await
            .expect("synthesis should succeed");
        match clip {
            AudioClip::Encoded(bytes) => {
                eprintln!("received wav: {} bytes", bytes.len());
                assert!(bytes.len() > 1000, "the clip is suspiciously short");
            }
            AudioClip::Pcm { bytes, .. } => panic!(
                "the third-party server should return a container, got raw PCM ({} bytes)",
                bytes.len()
            ),
        }
    }
}

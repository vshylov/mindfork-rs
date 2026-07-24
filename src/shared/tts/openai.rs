//! The speech client for the OpenAI `POST /v1/audio/speech` protocol. Serves
//! **two** modes: the OpenAI cloud and any third-party OpenAI-compatible TTS
//! server (Kokoro-FastAPI, speaches, LocalAI, …) — they differ in base URL, key,
//! and the requested response format. See docs/research/tts.md §3.1, §3.4.

use anyhow::{Context, Result};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::{AudioClip, TtsEngine, error_body};

/// The ceiling on `input` length for OpenAI (characters) — a hard API limit.
const OPENAI_MAX_INPUT_CHARS: usize = 4096;
/// A conservative ceiling for a third-party server: every one has its own
/// limits, usually undocumented, and a short chunk also gives the first sound sooner.
const EXTERNAL_MAX_INPUT_CHARS: usize = 2000;

/// The sample rate of raw PCM from OpenAI (`response_format:"pcm"`): 24 kHz,
/// s16le, mono — documented, not reported in the response.
const OPENAI_PCM_RATE: u32 = 24_000;

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
    /// The requested response format: `pcm` for the cloud (bypassing a
    /// decoder) / `wav` for a third-party server (the most portable — the
    /// only one some servers support).
    response_format: &'static str,
    max_input_chars: usize,
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
            response_format: "pcm",
            max_input_chars: OPENAI_MAX_INPUT_CHARS,
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
            response_format: "wav",
            max_input_chars: EXTERNAL_MAX_INPUT_CHARS,
        }
    }

    /// The request body. An unset value isn't sent (`skip_serializing_if`):
    /// third-party servers support different field sets, and an extra field
    /// might be rejected.
    fn body<'a>(&'a self, text: &'a str) -> SpeechRequest<'a> {
        SpeechRequest {
            model: self.model.as_deref(),
            input: text,
            voice: self.voice.as_deref(),
            instructions: self.instructions.as_deref(),
            response_format: self.response_format,
            // We only send speed when it differs from normal. For
            // `gpt-4o-mini-tts` the field is effectively ignored (a known
            // defect) — there speed is requested via words in `instructions`.
            speed: (self.speed != 1.0).then_some(self.speed),
        }
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
        let url = format!("{}/audio/speech", self.base_url);
        let mut rb = self.http.post(&url).json(&self.body(text));
        if let Some(key) = &self.api_key {
            rb = rb.bearer_auth(key);
        }
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => anyhow::bail!("speech synthesis cancelled"),
            r = rb.send() => r.with_context(|| format!("POST {url}"))?,
        };
        if !resp.status().is_success() {
            return Err(error_body("TTS", resp).await);
        }
        let bytes = tokio::select! {
            biased;
            _ = cancel.cancelled() => anyhow::bail!("speech synthesis cancelled"),
            b = resp.bytes() => b.context("reading TTS audio response")?,
        };
        Ok(if self.response_format == "pcm" {
            AudioClip::Pcm {
                sample_rate: OPENAI_PCM_RATE,
                channels: 1,
                bytes: bytes.to_vec(),
            }
        } else {
            AudioClip::Encoded(bytes.to_vec())
        })
    }

    fn max_input_chars(&self) -> usize {
        self.max_input_chars
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json_of(engine: &OpenAiTts, text: &str) -> serde_json::Value {
        serde_json::to_value(engine.body(text)).unwrap()
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

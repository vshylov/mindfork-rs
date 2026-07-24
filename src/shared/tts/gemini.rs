//! The Gemini speech client. A key stroke of luck: Gemini's TTS lives **in the
//! same protocol** already used by [`crate::shared::api::gemini`] — plain
//! `generateContent` with `responseModalities:["AUDIO"]` and the
//! `x-goog-api-key` header; no new transport is needed. The response is
//! base64 raw PCM **with no WAV header**; the sample rate is taken from
//! `mimeType` (`audio/L16;codec=pcm;rate=24000`), not hardcoded. See
//! docs/research/tts.md §3.2.

use anyhow::{Context, Result};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::{AudioClip, TtsEngine, error_body};

/// The ceiling on a single request's text length (characters). Gemini's TTS
/// models have a 32k-token context — the limit isn't driven by that, but by
/// wanting the first sound sooner.
const GEMINI_MAX_INPUT_CHARS: usize = 2000;

/// Sample rate when `mimeType` doesn't report one (the documented default).
const DEFAULT_PCM_RATE: u32 = 24_000;

/// The default directive when the user hasn't set "Instructions" (see
/// [`GeminiTts::prompt`]).
const DEFAULT_DIRECTIVE: &str = "Read this text aloud verbatim";

/// A client for native `generateContent` in audio-response mode.
pub struct GeminiTts {
    http: reqwest::Client,
    /// The base URL (`…/v1beta`); we append the
    /// `/models/{model}:generateContent` path ourselves.
    base_url: String,
    api_key: String,
    model: String,
    voice: Option<String>,
    /// Style instructions. Gemini has no dedicated field for this — style is
    /// set in natural language, inside the request text itself (provider
    /// docs).
    instructions: Option<String>,
}

impl GeminiTts {
    pub fn new(
        base_url: String,
        api_key: String,
        model: String,
        voice: Option<String>,
        instructions: Option<String>,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            // The model name may arrive with a `models/` prefix — the path
            // already carries it.
            model: model.trim().trim_start_matches("models/").to_string(),
            voice,
            instructions,
        }
    }

    /// The request text: the style instruction goes as a **prefix** to the
    /// phrase — there's no dedicated field for it in the API (the canonical
    /// shape from the provider docs: `Say cheerfully: …`).
    ///
    /// The prefix is required, not cosmetic: without it Gemini treats a
    /// short utterance as a task it must respond to, and returns `400 Model
    /// tried to generate text, but it should only be used for TTS` (caught
    /// by a live e2e smoke on a short test phrase). So an empty
    /// "Instructions" field falls back to a neutral directive. The
    /// directive's language doesn't affect the speech language — that's
    /// determined by the transcript itself.
    fn prompt(&self, text: &str) -> String {
        let style = self.instructions.as_deref().unwrap_or(DEFAULT_DIRECTIVE);
        format!("{style}: {text}")
    }

    fn body(&self, text: &str) -> GenerateRequest {
        GenerateRequest {
            contents: vec![Content {
                parts: vec![Part {
                    text: self.prompt(text),
                }],
            }],
            generation_config: GenerationConfig {
                response_modalities: vec!["AUDIO"],
                speech_config: self.voice.as_ref().map(|v| SpeechConfig {
                    voice_config: VoiceConfig {
                        prebuilt_voice_config: PrebuiltVoiceConfig {
                            voice_name: v.clone(),
                        },
                    },
                }),
            },
        }
    }
}

// ── request body ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct GenerateRequest {
    contents: Vec<Content>,
    #[serde(rename = "generationConfig")]
    generation_config: GenerationConfig,
}

#[derive(Debug, Serialize)]
struct Content {
    parts: Vec<Part>,
}

#[derive(Debug, Serialize)]
struct Part {
    text: String,
}

#[derive(Debug, Serialize)]
struct GenerationConfig {
    #[serde(rename = "responseModalities")]
    response_modalities: Vec<&'static str>,
    #[serde(rename = "speechConfig", skip_serializing_if = "Option::is_none")]
    speech_config: Option<SpeechConfig>,
}

#[derive(Debug, Serialize)]
struct SpeechConfig {
    #[serde(rename = "voiceConfig")]
    voice_config: VoiceConfig,
}

#[derive(Debug, Serialize)]
struct VoiceConfig {
    #[serde(rename = "prebuiltVoiceConfig")]
    prebuilt_voice_config: PrebuiltVoiceConfig,
}

#[derive(Debug, Serialize)]
struct PrebuiltVoiceConfig {
    #[serde(rename = "voiceName")]
    voice_name: String,
}

// ── response body ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct GenerateResponse {
    #[serde(default)]
    candidates: Vec<Candidate>,
    #[serde(rename = "promptFeedback", default)]
    prompt_feedback: Option<PromptFeedback>,
}

#[derive(Debug, Deserialize)]
struct Candidate {
    #[serde(default)]
    content: Option<RespContent>,
}

#[derive(Debug, Deserialize)]
struct RespContent {
    #[serde(default)]
    parts: Vec<RespPart>,
}

#[derive(Debug, Deserialize)]
struct RespPart {
    #[serde(rename = "inlineData", default)]
    inline_data: Option<InlineData>,
}

#[derive(Debug, Deserialize)]
struct InlineData {
    #[serde(rename = "mimeType", default)]
    mime_type: String,
    #[serde(default)]
    data: String,
}

#[derive(Debug, Deserialize)]
struct PromptFeedback {
    #[serde(rename = "blockReason", default)]
    block_reason: Option<String>,
}

/// Sample rate from a `mimeType` like `audio/L16;codec=pcm;rate=24000`. Can't
/// be hardcoded — the provider is free to return a different one (provider
/// docs).
fn rate_from_mime(mime: &str) -> u32 {
    mime.split(';')
        .filter_map(|p| p.trim().strip_prefix("rate="))
        .find_map(|v| v.trim().parse().ok())
        .unwrap_or(DEFAULT_PCM_RATE)
}

/// Extracts audio from the response: base64 PCM + the rate from `mimeType`.
/// Turns a filter block or an empty response into a clear error (otherwise it
/// would be "silence for no reason").
fn clip_from_response(resp: GenerateResponse) -> Result<AudioClip> {
    if let Some(reason) = resp.prompt_feedback.and_then(|f| f.block_reason) {
        anyhow::bail!("Gemini rejected the speech-synthesis request (reason: {reason})");
    }
    let data = resp
        .candidates
        .into_iter()
        .filter_map(|c| c.content)
        .flat_map(|c| c.parts)
        .find_map(|p| p.inline_data);
    let Some(data) = data else {
        anyhow::bail!("Gemini returned no audio");
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.data.trim())
        .context("decoding base64 audio from Gemini")?;
    Ok(AudioClip::Pcm {
        sample_rate: rate_from_mime(&data.mime_type),
        channels: 1,
        bytes,
    })
}

#[async_trait::async_trait]
impl TtsEngine for GeminiTts {
    async fn synthesize(&self, text: &str, cancel: &CancellationToken) -> Result<AudioClip> {
        // Non-streaming `generateContent`: `streamGenerateContent` for TTS is
        // only supported by 3.1+ models, and the per-sentence pipeline
        // already gives a fast first sound (docs/research/tts.md §3.2, §8).
        let url = format!("{}/models/{}:generateContent", self.base_url, self.model);
        let rb = self
            .http
            .post(&url)
            .header("x-goog-api-key", &self.api_key)
            .json(&self.body(text));
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => anyhow::bail!("speech synthesis cancelled"),
            r = rb.send() => r.with_context(|| format!("POST {url}"))?,
        };
        if !resp.status().is_success() {
            return Err(error_body("Gemini TTS", resp).await);
        }
        let parsed: GenerateResponse = tokio::select! {
            biased;
            _ = cancel.cancelled() => anyhow::bail!("speech synthesis cancelled"),
            j = resp.json() => j.context("decoding Gemini TTS response")?,
        };
        clip_from_response(parsed)
    }

    fn max_input_chars(&self) -> usize {
        GEMINI_MAX_INPUT_CHARS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> GeminiTts {
        GeminiTts::new(
            "https://generativelanguage.googleapis.com/v1beta/".into(),
            "key".into(),
            "models/gemini-2.5-flash-preview-tts".into(),
            Some("Kore".into()),
            None,
        )
    }

    #[test]
    fn request_asks_for_audio_modality_and_voice() {
        let e = engine();
        let v = serde_json::to_value(e.body("привет")).unwrap();
        assert_eq!(
            v["contents"][0]["parts"][0]["text"],
            format!("{DEFAULT_DIRECTIVE}: привет")
        );
        assert_eq!(v["generationConfig"]["responseModalities"][0], "AUDIO");
        assert_eq!(
            v["generationConfig"]["speechConfig"]["voiceConfig"]["prebuiltVoiceConfig"]["voiceName"],
            "Kore"
        );
        // The `models/` prefix and the trailing slash are stripped — the
        // path already carries them.
        assert_eq!(e.model, "gemini-2.5-flash-preview-tts");
        assert_eq!(
            e.base_url,
            "https://generativelanguage.googleapis.com/v1beta"
        );
    }

    #[test]
    fn text_always_carries_read_directive() {
        // With no "Instructions" set, the default directive is substituted:
        // without the prefix Gemini responds 400 "Model tried to generate
        // text" (see `prompt`).
        let v = serde_json::to_value(engine().body("проверка связи")).unwrap();
        assert_eq!(
            v["contents"][0]["parts"][0]["text"],
            format!("{DEFAULT_DIRECTIVE}: проверка связи")
        );
    }

    #[test]
    fn instructions_go_as_style_prefix() {
        let e = GeminiTts::new(
            "https://x/v1beta".into(),
            "k".into(),
            "m".into(),
            None,
            Some("скажи бодро".into()),
        );
        let v = serde_json::to_value(e.body("текст")).unwrap();
        assert_eq!(v["contents"][0]["parts"][0]["text"], "скажи бодро: текст");
        // No voice set — the speechConfig section is absent from the body
        // entirely.
        assert!(
            v["generationConfig"].get("speechConfig").is_none(),
            "an unset voice is not sent: {v}"
        );
    }

    #[test]
    fn parses_inline_pcm_and_rate_from_mime() {
        let json = serde_json::json!({
            "candidates": [{
                "content": {"parts": [{"inlineData": {
                    "mimeType": "audio/L16;codec=pcm;rate=16000",
                    "data": "AAECAw=="
                }}]},
                "finishReason": "STOP"
            }]
        });
        let resp: GenerateResponse = serde_json::from_value(json).unwrap();
        assert_eq!(
            clip_from_response(resp).unwrap(),
            AudioClip::Pcm {
                sample_rate: 16_000,
                channels: 1,
                bytes: vec![0, 1, 2, 3],
            }
        );
    }

    #[test]
    fn rate_defaults_when_mime_says_nothing() {
        assert_eq!(rate_from_mime("audio/L16;codec=pcm"), DEFAULT_PCM_RATE);
        assert_eq!(rate_from_mime(""), DEFAULT_PCM_RATE);
        assert_eq!(rate_from_mime("audio/L16; rate=48000 "), 48_000);
    }

    #[test]
    fn block_and_empty_answer_become_clear_errors() {
        let blocked: GenerateResponse = serde_json::from_value(
            serde_json::json!({"promptFeedback": {"blockReason": "SAFETY"}}),
        )
        .unwrap();
        let err = clip_from_response(blocked).unwrap_err().to_string();
        assert!(
            err.contains("SAFETY"),
            "the block reason is in the error: {err}"
        );

        let empty: GenerateResponse =
            serde_json::from_value(serde_json::json!({"candidates": []})).unwrap();
        assert!(
            clip_from_response(empty).is_err(),
            "an empty response is an error"
        );
    }

    /// A live Gemini smoke: synthesizing a short Russian phrase. Requires
    /// `MINDFORK_GEMINI_KEY`; silently skipped without it.
    #[tokio::test]
    #[ignore = "requires MINDFORK_GEMINI_KEY (live Gemini API)"]
    async fn gemini_synthesizes_russian_speech_live() {
        let Ok(key) = std::env::var("MINDFORK_GEMINI_KEY") else {
            eprintln!("skip: MINDFORK_GEMINI_KEY not set");
            return;
        };
        let e = GeminiTts::new(
            crate::shared::config::CloudProvider::Gemini
                .chat_base_url()
                .to_string(),
            key,
            std::env::var("MINDFORK_TTS_MODEL")
                .unwrap_or_else(|_| crate::shared::config::DEFAULT_TTS_GEMINI_MODEL.into()),
            Some(crate::shared::config::DEFAULT_TTS_GEMINI_VOICE.into()),
            None,
        );
        let clip = e
            .synthesize(
                "Проверка озвучивания. Латинская вставка: API, JSON.",
                &CancellationToken::new(),
            )
            .await
            .expect("synthesis should succeed");
        let AudioClip::Pcm {
            sample_rate,
            channels,
            bytes,
        } = clip
        else {
            panic!("Gemini should return raw PCM");
        };
        eprintln!("received PCM: {} bytes, {sample_rate} Hz", bytes.len());
        assert_eq!(channels, 1);
        assert!(sample_rate >= 8000, "a reasonable sample rate");
        assert!(bytes.len() > 10_000, "the clip is suspiciously short");
    }
}

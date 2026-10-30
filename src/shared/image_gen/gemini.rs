//! Gemini's picture models through `generateContent`
//! (docs/history/image-generation.md §3.2, §6.2): the same endpoint and the
//! same `x-goog-api-key` header the chat client and speech use, with
//! `responseModalities: ["IMAGE"]` — measured enough on its own (§11.1), so no
//! sentence is asked for and thrown away — and `imageConfig`.
//!
//! The answer's first image part is the picture: a JPEG, measured, carrying a
//! thought signature of up to three million characters, which is dropped with
//! everything else. A prompt the provider blocks is `promptFeedback.blockReason`;
//! a picture it blocks is an `IMAGE_*` finish reason; and a model that declines
//! answers in **plain text with no picture** — caught by the picture's absence,
//! its text kept as the reason (§3.2). In the probe Gemini did not refuse at all:
//! it softened the violent prompt and drew (§11.1), so these paths are tested on
//! recorded shapes, not provoked.

use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{Drawing, ImageGenConfig, ImageGenerator, ImageRequest, Picture, Spent, excerpt, send};

/// What the errors of this client are said about.
const SUBJECT: &str = "Gemini images";

/// The finish reasons Gemini stops a picture by — its moderation, not a fault.
const REFUSING_FINISH: [&str; 8] = [
    "SAFETY",
    "PROHIBITED_CONTENT",
    "BLOCKLIST",
    "IMAGE_SAFETY",
    "IMAGE_PROHIBITED_CONTENT",
    "IMAGE_RECITATION",
    "IMAGE_OTHER",
    "RECITATION",
];

pub struct GeminiImages {
    http: reqwest::Client,
    cfg: ImageGenConfig,
}

impl GeminiImages {
    pub fn new(cfg: ImageGenConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            cfg: ImageGenConfig {
                // The name may arrive with a `models/` prefix — the path carries it.
                model: cfg.model.trim().trim_start_matches("models/").to_string(),
                ..cfg
            },
        }
    }

    pub(crate) fn url(&self) -> String {
        format!(
            "{}/models/{}:generateContent",
            self.cfg.base_url, self.cfg.model
        )
    }

    /// The request body. Pure. The pictures to change come first as
    /// `inlineData` parts of the one user turn, the prompt after them; no
    /// thinking config — it cannot be turned off on these models (§3.2).
    pub(crate) fn body(&self, req: &ImageRequest) -> Value {
        let mut parts: Vec<Value> = req
            .inputs
            .iter()
            .map(|i| json!({ "inlineData": { "mimeType": i.mime, "data": i.base64() } }))
            .collect();
        parts.push(json!({ "text": req.prompt }));
        let mut image_config = json!({ "aspectRatio": req.aspect.ratio() });
        if let Some(size) = &self.cfg.quality {
            image_config["imageSize"] = json!(size);
        }
        json!({
            "contents": [{ "role": "user", "parts": parts }],
            "generationConfig": {
                "responseModalities": ["IMAGE"],
                "imageConfig": image_config,
            },
        })
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Response {
    #[serde(default)]
    candidates: Vec<Candidate>,
    #[serde(default)]
    prompt_feedback: Option<PromptFeedback>,
    #[serde(default)]
    usage_metadata: Option<Usage>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    #[serde(default)]
    content: Option<Content>,
    #[serde(default)]
    finish_reason: Option<String>,
    #[serde(default)]
    finish_message: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct Content {
    #[serde(default)]
    parts: Vec<Part>,
}

/// One part. A thought signature is not read — serde skips it, megabytes and
/// all — and a thought's own part is not the picture.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Part {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    inline_data: Option<Inline>,
    #[serde(default)]
    thought: bool,
}

#[derive(Debug, Deserialize, Default)]
struct Inline {
    #[serde(default)]
    data: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PromptFeedback {
    #[serde(default)]
    block_reason: Option<String>,
    #[serde(default)]
    block_reason_message: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Usage {
    #[serde(default)]
    prompt_token_count: Option<u64>,
    #[serde(default)]
    candidates_token_count: Option<u64>,
    #[serde(default)]
    thoughts_token_count: Option<u64>,
}

impl Usage {
    /// What Gemini counted: the prompt, and the picture with the thinking it
    /// cannot be told to skip — both billed as output (§11.1).
    fn spent(&self) -> Spent {
        let output = match (self.candidates_token_count, self.thoughts_token_count) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
        };
        Spent {
            input_tokens: self.prompt_token_count,
            output_tokens: output,
            cost_usd: None,
        }
    }
}

/// The picture, or the refusal, out of what Gemini answered. Pure.
pub(crate) fn answer_from(status: reqwest::StatusCode, text: &str) -> anyhow::Result<Drawing> {
    if !status.is_success() {
        let said = serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|b| {
                b.pointer("/error/message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| excerpt(text));
        anyhow::bail!("{SUBJECT}: status {status}: {said}");
    }
    let resp: Response = serde_json::from_str(text)
        .map_err(|e| anyhow::anyhow!("{SUBJECT}: the answer cannot be read: {e}"))?;
    let spent = resp
        .usage_metadata
        .as_ref()
        .map(Usage::spent)
        .unwrap_or_default();
    if let Some(feedback) = &resp.prompt_feedback
        && let Some(reason) = &feedback.block_reason
    {
        let said = feedback
            .block_reason_message
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map_or_else(
                || format!("the prompt was blocked ({reason})"),
                str::to_string,
            );
        return Ok(Drawing::Refused {
            reason: said,
            spent,
        });
    }
    let candidate = resp.candidates.into_iter().next().unwrap_or_default();
    let finish = candidate.finish_reason.unwrap_or_default();
    let parts = candidate.content.map(|c| c.parts).unwrap_or_default();
    if let Some(inline) = parts
        .iter()
        .filter(|p| !p.thought)
        .find_map(|p| p.inline_data.as_ref())
    {
        let (bytes, mime) = super::picture_bytes(SUBJECT, &inline.data)?;
        return Ok(Drawing::Picture(Picture {
            bytes,
            mime,
            revised_prompt: None,
            spent,
        }));
    }
    let said: String = parts
        .iter()
        .filter(|p| !p.thought)
        .filter_map(|p| p.text.as_deref())
        .collect::<Vec<_>>()
        .join("")
        .trim()
        .to_string();
    let message = candidate
        .finish_message
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty());
    if REFUSING_FINISH.contains(&finish.as_str()) {
        let reason = message
            .map(str::to_string)
            .or_else(|| (!said.is_empty()).then(|| said.clone()))
            .unwrap_or_else(|| format!("the picture was blocked ({finish})"));
        return Ok(Drawing::Refused { reason, spent });
    }
    // A model that declines says so in words and draws nothing (§3.2).
    if !said.is_empty() {
        return Ok(Drawing::Refused {
            reason: said,
            spent,
        });
    }
    if finish.is_empty() {
        anyhow::bail!("{SUBJECT}: no picture came back");
    }
    anyhow::bail!("{SUBJECT}: no picture came back (finish reason: {finish})")
}

#[async_trait::async_trait]
impl ImageGenerator for GeminiImages {
    async fn draw(&self, req: ImageRequest, cancel: &CancellationToken) -> anyhow::Result<Drawing> {
        let rb = self
            .http
            .post(self.url())
            .header("x-goog-api-key", &self.cfg.api_key)
            .json(&self.body(&req));
        let (status, text) = send(SUBJECT, rb, cancel).await?;
        let answer = answer_from(status, &text);
        match &answer {
            Ok(Drawing::Refused { reason, .. }) => {
                tracing::info!(%status, %reason, "Gemini refused to draw");
            }
            Err(err) => tracing::warn!(%status, error = %err, "Gemini did not draw"),
            Ok(Drawing::Picture(p)) => {
                tracing::info!(bytes = p.bytes.len(), spent = ?p.spent, "Gemini drew");
            }
        }
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::config::ImageGenProvider;
    use crate::shared::image_gen::{Aspect, InputImage};
    use base64::Engine as _;

    fn client(quality: Option<&str>) -> GeminiImages {
        GeminiImages::new(ImageGenConfig {
            provider: ImageGenProvider::Gemini,
            attribution: false,
            model: "models/gemini-nano-banana-2.1".into(),
            quality: quality.map(str::to_string),
            base_url: "https://generativelanguage.googleapis.com/v1beta".into(),
            api_key: "k".into(),
        })
    }

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    /// The picture alone is asked for, at the aspect and size; a picture to
    /// change goes before the prompt, inline.
    #[test]
    fn the_request_asks_for_a_picture_of_the_shape() {
        let c = client(Some("1K"));
        assert_eq!(
            c.url(),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-nano-banana-2.1:generateContent"
        );
        let png = crate::shared::image_gen::tests::tiny_png();
        let body = c.body(&ImageRequest {
            prompt: "make it night".into(),
            aspect: Aspect::Portrait,
            inputs: vec![InputImage {
                mime: "image/png".into(),
                bytes: png.clone(),
            }],
        });
        assert_eq!(
            body,
            json!({
                "contents": [{ "role": "user", "parts": [
                    { "inlineData": { "mimeType": "image/png", "data": b64(&png) } },
                    { "text": "make it night" },
                ]}],
                "generationConfig": {
                    "responseModalities": ["IMAGE"],
                    "imageConfig": { "aspectRatio": "2:3", "imageSize": "1K" },
                },
            })
        );
        let plain = client(None).body(&ImageRequest {
            prompt: "a fox".into(),
            aspect: Aspect::Square,
            inputs: Vec::new(),
        });
        assert_eq!(
            plain["generationConfig"]["imageConfig"],
            json!({ "aspectRatio": "1:1" })
        );
    }

    /// The shape of §11.1: a thought's part first, then the picture with its
    /// signature beside it; the thinking is billed and counted as output.
    #[test]
    fn the_first_picture_that_is_no_thought_is_taken() {
        let png = crate::shared::image_gen::tests::tiny_png();
        let text = json!({
            "candidates": [{
                "content": { "role": "model", "parts": [
                    { "thought": true, "inlineData": { "mimeType": "image/png", "data": b64(b"not it") } },
                    { "inlineData": { "mimeType": "image/png", "data": b64(&png) },
                      "thoughtSignature": "x".repeat(10_000) },
                ]},
                "finishReason": "STOP",
            }],
            "usageMetadata": {
                "promptTokenCount": 12, "candidatesTokenCount": 1120,
                "thoughtsTokenCount": 640, "totalTokenCount": 1772,
            },
        })
        .to_string();
        let Drawing::Picture(p) = answer_from(reqwest::StatusCode::OK, &text).unwrap() else {
            panic!("a picture");
        };
        assert_eq!(p.bytes, png);
        assert_eq!(p.spent.input_tokens, Some(12));
        assert_eq!(p.spent.output_tokens, Some(1760));
    }

    /// The three ways Gemini says no (§3.2), each an answer, not an error.
    #[test]
    fn a_block_a_blocked_picture_and_a_sentence_are_refusals() {
        let ok = reqwest::StatusCode::OK;
        let blocked = json!({"promptFeedback": {"blockReason": "PROHIBITED_CONTENT"}});
        let Drawing::Refused { reason, .. } = answer_from(ok, &blocked.to_string()).unwrap() else {
            panic!("a refusal");
        };
        assert_eq!(reason, "the prompt was blocked (PROHIBITED_CONTENT)");

        let picture = json!({"candidates": [{"finishReason": "IMAGE_SAFETY",
            "finishMessage": "Unable to show the generated image."}]});
        let Drawing::Refused { reason, .. } = answer_from(ok, &picture.to_string()).unwrap() else {
            panic!("a refusal");
        };
        assert_eq!(reason, "Unable to show the generated image.");

        let words = json!({"candidates": [{"content": {"parts": [
            {"text": "I can't create images of that. "}]}, "finishReason": "STOP"}]});
        let Drawing::Refused { reason, .. } = answer_from(ok, &words.to_string()).unwrap() else {
            panic!("a refusal");
        };
        assert_eq!(reason, "I can't create images of that.");
    }

    /// Nothing at all is not a refusal — it is a broken answer — and an error
    /// status is said in Google's words: a free-tier key is refused pictures.
    #[test]
    fn nothing_and_an_error_status_are_errors() {
        let ok = reqwest::StatusCode::OK;
        let empty = json!({"candidates": [{"finishReason": "STOP"}]});
        let err = answer_from(ok, &empty.to_string()).unwrap_err();
        assert!(err.to_string().contains("finish reason: STOP"), "{err}");

        let quota = json!({"error": {"code": 429, "status": "RESOURCE_EXHAUSTED",
            "message": "Quota exceeded for metric: generate_content_free_tier_requests"}});
        let err = answer_from(reqwest::StatusCode::TOO_MANY_REQUESTS, &quota.to_string())
            .unwrap_err()
            .to_string();
        assert!(err.contains("429"), "{err}");
        assert!(err.contains("free_tier"), "{err}");
    }
}

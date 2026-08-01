//! Gemini video understanding: a YouTube URL handed to `generateContent`
//! directly, no download and no captions scraping on our side.
//!
//! The same protocol the project already speaks
//! ([`crate::shared::api::gemini`]) and the same `x-goog-api-key` header — the
//! only new part is the `file_data` part plus `videoMetadata`. Measured
//! 2026-08-01 (docs/research/youtube-integration.md §3.2): a 20 s clip costs
//! 2098 prompt tokens and answers in 3.4 s; the whole 213 s video, 22 050 tokens
//! in 8.0 s. `watch?v=`, `youtu.be/` and `/shorts/` URLs are all accepted
//! verbatim, so nothing needs normalizing before the call.
//!
//! Non-streaming on purpose: the tool returns one block of text into the
//! conversation, so there is nothing to stream it to.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{VideoAnswer, VideoConfig, VideoRequest, VideoUnderstanding, error_body};

/// Client for `…/models/{model}:generateContent` in video-input mode.
pub struct GeminiVideo {
    http: reqwest::Client,
    cfg: VideoConfig,
}

impl GeminiVideo {
    pub fn new(cfg: VideoConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            cfg: VideoConfig {
                base_url: cfg.base_url.trim_end_matches('/').to_string(),
                // The name may arrive with a `models/` prefix — the path carries it.
                model: cfg.model.trim().trim_start_matches("models/").to_string(),
                ..cfg
            },
        }
    }

    /// Builds the request body. Pure — the shape is what the tests assert on.
    pub(crate) fn body(&self, req: &VideoRequest) -> Value {
        let mut file_part = json!({ "file_data": { "file_uri": req.url } });
        // Only send videoMetadata when a bound is actually set: an empty object
        // is a needless way to be rejected.
        if req.start_secs.is_some() || req.end_secs.is_some() {
            let mut meta = serde_json::Map::new();
            if let Some(s) = req.start_secs {
                meta.insert("start_offset".into(), json!(format!("{s}s")));
            }
            if let Some(e) = req.end_secs {
                meta.insert("end_offset".into(), json!(format!("{e}s")));
            }
            file_part["video_metadata"] = Value::Object(meta);
        }

        let mut generation_config = serde_json::Map::new();
        generation_config.insert("maxOutputTokens".into(), json!(req.max_output_tokens));
        generation_config.insert(
            "mediaResolution".into(),
            json!(self.cfg.media_resolution.as_arg()),
        );
        // Describing a video is not a reasoning task, and `maxOutputTokens`
        // **includes thought tokens** — left alone, thinking can consume the whole
        // budget and return an empty answer (observed on gemini-3.6-flash at a
        // small budget). Mute it the way this generation expects, reusing the
        // engine wire's own inference rather than a second copy of it.
        generation_config.insert("thinkingConfig".into(), self.muted_thinking());
        json!({
            "contents": [{
                "role": "user",
                "parts": [ { "text": req.prompt }, file_part ]
            }],
            "generationConfig": Value::Object(generation_config),
        })
    }

    /// The "as little thinking as this model allows" config. Gemini 3.x has no
    /// off switch — `minimal` is the floor (and 3 **Pro** rejects even that, so
    /// it gets `low`); 2.5 takes a zero budget.
    fn muted_thinking(&self) -> Value {
        let m = &self.cfg.model;
        if crate::shared::api::gemini::is_gemini_3(m) {
            let level = if crate::shared::api::gemini::is_gemini_3_pro(m) {
                "low"
            } else {
                "minimal"
            };
            json!({ "thinkingLevel": level })
        } else {
            json!({ "thinkingBudget": 0 })
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Default)]
struct GenerateResponse {
    #[serde(default)]
    candidates: Vec<Candidate>,
    #[serde(rename = "promptFeedback", default)]
    prompt_feedback: Option<PromptFeedback>,
}

#[derive(Debug, Deserialize, Serialize, Default)]
struct Candidate {
    #[serde(default)]
    content: Option<RespContent>,
    #[serde(rename = "finishReason", default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Default)]
struct RespContent {
    #[serde(default)]
    parts: Vec<RespPart>,
}

#[derive(Debug, Deserialize, Serialize, Default)]
struct RespPart {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Default)]
struct PromptFeedback {
    #[serde(rename = "blockReason", default)]
    block_reason: Option<String>,
}

/// Pulls the answer out of the response. A filter block, an unavailable video or
/// a budget spent entirely on thinking all produce **no text** — without this
/// they would surface as a blank result, which reads like a broken tool. Pure —
/// tested without a network.
///
/// A `MAX_TOKENS` finish **with** text is not an error: it is a complete-looking
/// prefix, which is why it is reported as [`VideoAnswer::truncated`] rather than
/// dropped (docs/youtube-transcript.md §3 F5).
fn text_from_response(resp: GenerateResponse) -> Result<VideoAnswer> {
    if let Some(reason) = resp.prompt_feedback.and_then(|f| f.block_reason) {
        anyhow::bail!("Gemini refused the video request (reason: {reason})");
    }
    let finish = resp
        .candidates
        .first()
        .and_then(|c| c.finish_reason.clone())
        .unwrap_or_default();
    let text: String = resp
        .candidates
        .into_iter()
        .filter_map(|c| c.content)
        .flat_map(|c| c.parts)
        .filter_map(|p| p.text)
        .collect::<Vec<_>>()
        .join("");
    if text.trim().is_empty() {
        if finish.is_empty() {
            anyhow::bail!("Gemini returned no description of the video");
        }
        anyhow::bail!("Gemini returned no description of the video (finish reason: {finish})");
    }
    Ok(VideoAnswer {
        text,
        truncated: finish.eq_ignore_ascii_case("MAX_TOKENS"),
    })
}

#[async_trait::async_trait]
impl VideoUnderstanding for GeminiVideo {
    async fn describe(&self, req: VideoRequest, cancel: &CancellationToken) -> Result<VideoAnswer> {
        let url = format!(
            "{}/models/{}:generateContent",
            self.cfg.base_url, self.cfg.model
        );
        let rb = self
            .http
            .post(&url)
            .header("x-goog-api-key", &self.cfg.api_key)
            .json(&self.body(&req));
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => anyhow::bail!("video request cancelled"),
            r = rb.send() => r.with_context(|| format!("POST {url}"))?,
        };
        if !resp.status().is_success() {
            return Err(error_body("Gemini video", resp).await);
        }
        let parsed: GenerateResponse = tokio::select! {
            biased;
            _ = cancel.cancelled() => anyhow::bail!("video request cancelled"),
            j = resp.json() => j.context("decoding the Gemini video response")?,
        };
        text_from_response(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::config::MediaResolution;

    fn client(model: &str) -> GeminiVideo {
        GeminiVideo::new(VideoConfig {
            model: model.into(),
            base_url: "https://generativelanguage.googleapis.com/v1beta/".into(),
            api_key: "k".into(),
            media_resolution: MediaResolution::Low,
            max_minutes: 30,
        })
    }

    fn req() -> VideoRequest {
        VideoRequest {
            url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into(),
            prompt: "describe it".into(),
            start_secs: None,
            end_secs: None,
            max_output_tokens: 1500,
        }
    }

    #[test]
    fn body_carries_the_url_as_a_file_part() {
        let b = client("gemini-2.5-flash").body(&req());
        let parts = &b["contents"][0]["parts"];
        assert_eq!(parts[0]["text"], "describe it");
        assert_eq!(
            parts[1]["file_data"]["file_uri"],
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
        assert_eq!(
            b["generationConfig"]["mediaResolution"],
            "MEDIA_RESOLUTION_LOW"
        );
    }

    #[test]
    fn segment_bounds_are_sent_only_when_set() {
        // No bounds → no videoMetadata at all.
        let b = client("gemini-2.5-flash").body(&req());
        assert!(b["contents"][0]["parts"][1].get("video_metadata").is_none());

        let r = VideoRequest {
            start_secs: Some(40),
            end_secs: Some(80),
            ..req()
        };
        let b = client("gemini-2.5-flash").body(&r);
        let meta = &b["contents"][0]["parts"][1]["video_metadata"];
        assert_eq!(meta["start_offset"], "40s");
        assert_eq!(meta["end_offset"], "80s");
    }

    #[test]
    fn thinking_is_muted_per_generation() {
        // 2.5 takes a zero budget; 3.x has no off switch, so `minimal` — except
        // 3 Pro, which rejects `minimal` (mirrors the engine wire).
        let b = client("gemini-2.5-flash").body(&req());
        assert_eq!(b["generationConfig"]["thinkingConfig"]["thinkingBudget"], 0);
        let b = client("gemini-3.6-flash").body(&req());
        assert_eq!(
            b["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "minimal"
        );
        let b = client("gemini-3-pro-preview").body(&req());
        assert_eq!(
            b["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "low"
        );
    }

    #[test]
    fn model_prefix_and_trailing_slash_are_normalized() {
        let c = client("models/gemini-2.5-flash");
        assert_eq!(c.cfg.model, "gemini-2.5-flash");
        assert!(!c.cfg.base_url.ends_with('/'));
    }

    #[test]
    fn response_text_is_joined_across_parts() {
        let resp: GenerateResponse = serde_json::from_value(json!({
            "candidates": [{"content": {"parts": [{"text": "a"}, {"text": "b"}]},
                            "finishReason": "STOP"}]
        }))
        .unwrap();
        let answer = text_from_response(resp).unwrap();
        assert_eq!(answer.text, "ab");
        assert!(!answer.truncated);
    }

    #[test]
    fn a_cut_off_answer_is_returned_and_flagged_not_dropped() {
        // The failure this exists to prevent: a transcript stopped at the output
        // ceiling still *looks* whole, so handing it back silently would let the
        // model believe it read the video to the end.
        let resp: GenerateResponse = serde_json::from_value(json!({
            "candidates": [{"content": {"parts": [{"text": "[0:00] the beginning"}]},
                            "finishReason": "MAX_TOKENS"}]
        }))
        .unwrap();
        let answer = text_from_response(resp).unwrap();
        assert!(answer.truncated, "MAX_TOKENS with text means truncated");
        assert_eq!(answer.text, "[0:00] the beginning");
    }

    #[test]
    fn empty_answer_names_the_finish_reason() {
        // The failure mode worth naming: the budget went to thinking, so the
        // answer is blank. A bare empty string would read as a broken tool.
        let resp: GenerateResponse = serde_json::from_value(json!({
            "candidates": [{"content": {"parts": []}, "finishReason": "MAX_TOKENS"}]
        }))
        .unwrap();
        let err = text_from_response(resp).unwrap_err().to_string();
        assert!(err.contains("MAX_TOKENS"), "got: {err}");
    }

    #[test]
    fn a_filter_block_is_reported_not_silently_empty() {
        let resp: GenerateResponse =
            serde_json::from_value(json!({"promptFeedback": {"blockReason": "SAFETY"}})).unwrap();
        let err = text_from_response(resp).unwrap_err().to_string();
        assert!(err.contains("SAFETY"), "got: {err}");
    }
}

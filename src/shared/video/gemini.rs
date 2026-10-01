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

use std::sync::OnceLock;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{VideoAnswer, VideoConfig, VideoRequest, VideoUnderstanding, error_from};
use crate::shared::api::gemini::{is_gemini_3, is_gemini_3_pro, level_above, says_level_refused};

/// Client for `…/models/{model}:generateContent` in video-input mode.
pub struct GeminiVideo {
    http: reqwest::Client,
    cfg: VideoConfig,
    /// The thinking level a 3.x model took after refusing the one below it —
    /// what [`Self::muted_thinking`] asks for from then on. Learned from the
    /// refusal, like the chat client's memo (`shared::api::effort`): which
    /// models have `minimal` is not something a name tells. Set only once the
    /// level was accepted.
    muted_level: OnceLock<&'static str>,
}

/// Where a 3.x body carries its thinking level.
const LEVEL_AT: &str = "/generationConfig/thinkingConfig/thinkingLevel";

impl GeminiVideo {
    pub fn new(cfg: VideoConfig) -> Self {
        Self {
            muted_level: OnceLock::new(),
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
    /// off switch — `minimal` is the floor, and a 3 **Pro** rejects even that, so
    /// it gets `low`; 2.5 takes a zero budget.
    ///
    /// The name is only what is known ahead of a refusal: `gemini-3.7-flash` and
    /// `gemini-3.8-flash` reject `minimal` too (measured on this request,
    /// 2026-10-01), and what such a model took instead is asked for from then
    /// on ([`Self::describe`]).
    fn muted_thinking(&self) -> Value {
        let m = &self.cfg.model;
        if !is_gemini_3(m) {
            return json!({ "thinkingBudget": 0 });
        }
        let level = match self.muted_level.get() {
            Some(learned) => learned,
            None if is_gemini_3_pro(m) => "low",
            None => "minimal",
        };
        json!({ "thinkingLevel": level })
    }

    /// The level this model took after refusing the one below — unset while it
    /// has refused nothing. For the tests and smokes, which must tell a request
    /// that survived the refusal from one that never met it.
    #[cfg(test)]
    pub(super) fn learned_level(&self) -> Option<&'static str> {
        self.muted_level.get().copied()
    }

    /// One request: the status and the whole body, both read under `cancel`.
    async fn post(
        &self,
        body: &Value,
        cancel: &CancellationToken,
    ) -> Result<(reqwest::StatusCode, String)> {
        let url = format!(
            "{}/models/{}:generateContent",
            self.cfg.base_url, self.cfg.model
        );
        let rb = self
            .http
            .post(&url)
            .header("x-goog-api-key", &self.cfg.api_key)
            .json(body);
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => anyhow::bail!("video request cancelled"),
            r = rb.send() => r.with_context(|| format!("POST {url}"))?,
        };
        let status = resp.status();
        let text = tokio::select! {
            biased;
            _ = cancel.cancelled() => anyhow::bail!("video request cancelled"),
            t = resp.text() => t,
        };
        match text {
            Ok(text) => Ok((status, text)),
            // An answer that cannot be read is an error; a refusal that cannot
            // is still that refusal, with nothing to quote.
            Err(err) if status.is_success() => {
                Err(err).context("reading the Gemini video response")
            }
            Err(_) => Ok((status, String::new())),
        }
    }

    /// The level to ask for in place of the one `body` carries, when `answer`
    /// is a refusal of it — *"Thinking level MINIMAL is not supported for this
    /// model."*, which names nothing, so the answer is the level above
    /// (docs/research/effort-tiers.md §2).
    fn level_instead(
        body: &Value,
        status: reqwest::StatusCode,
        answer: &str,
    ) -> Option<&'static str> {
        if status != reqwest::StatusCode::BAD_REQUEST || !says_level_refused(answer) {
            return None;
        }
        level_above(body.pointer(LEVEL_AT)?.as_str()?)
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
/// dropped (docs/history/youtube-transcript.md §3 F5).
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
        let mut body = self.body(&req);
        let (mut status, mut text) = self.post(&body, cancel).await?;
        // A thinking level the model does not have: asked once more at the level
        // above, and that level kept for the client only once it was accepted —
        // the chat client's rule (`shared::api::effort::send_asking_again`), on
        // a request path with errors of its own.
        if let Some(level) = Self::level_instead(&body, status, &text) {
            tracing::info!(
                model = %self.cfg.model,
                level,
                "the video model does not take this thinking level; asking again with the one above"
            );
            if let Some(slot) = body.pointer_mut(LEVEL_AT) {
                *slot = json!(level);
            }
            (status, text) = self.post(&body, cancel).await?;
            if status.is_success() {
                let _ = self.muted_level.set(level);
            }
        }
        if !status.is_success() {
            return Err(error_from("Gemini video", status, &text));
        }
        let parsed: GenerateResponse =
            serde_json::from_str(&text).context("decoding the Gemini video response")?;
        text_from_response(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::config::MediaResolution;

    fn client(model: &str) -> GeminiVideo {
        client_at("https://generativelanguage.googleapis.com/v1beta/", model)
    }

    fn client_at(base_url: &str, model: &str) -> GeminiVideo {
        GeminiVideo::new(VideoConfig {
            provider: crate::shared::config::VideoProvider::Gemini,
            attribution: false,
            model: model.into(),
            base_url: base_url.into(),
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

    use crate::shared::api::sse_stub::{self, Canned};

    /// A whole answer: one sentence about the video.
    const DESCRIBED: Canned = (
        "200 OK",
        sse_stub::JSON,
        r#"{"candidates":[{"content":{"parts":[{"text":"A man sings."}]},"finishReason":"STOP"}]}"#,
    );
    const LEVEL_REFUSED: Canned = (
        sse_stub::BAD_REQUEST,
        sse_stub::JSON,
        crate::shared::api::gemini::LEVEL_REFUSED,
    );

    /// The thinking level of each request the stub was sent, in order.
    fn levels_sent(bodies: &[String]) -> Vec<Value> {
        bodies
            .iter()
            .map(|b| {
                let body: Value = serde_json::from_str(b).unwrap_or_else(|e| panic!("{e}: {b:?}"));
                body.pointer(LEVEL_AT).cloned().unwrap_or(Value::Null)
            })
            .collect()
    }

    /// The defect: `gemini-3.8-flash` answers the muted `minimal` with a `400`,
    /// and nothing in its name says so — so with it in the video slot every
    /// video request failed. The request is made again at the level above, and
    /// the next one asks for that at once.
    #[tokio::test]
    async fn a_refused_minimal_level_is_asked_again_as_low_and_remembered() {
        let (url, stub) = sse_stub::serve_in_turn(&[LEVEL_REFUSED, DESCRIBED, DESCRIBED]);
        let client = client_at(&url, "gemini-3.8-flash");
        for _ in 0..2 {
            let answer = client
                .describe(req(), &CancellationToken::new())
                .await
                .expect("the request survives the refusal");
            assert_eq!(answer.text, "A man sings.");
        }
        assert_eq!(
            levels_sent(&stub.join().unwrap()),
            ["minimal", "low", "low"]
        );
        assert_eq!(client.learned_level(), Some("low"));
    }

    /// A level refused twice is the request's error, in the provider's words —
    /// and is not kept: the next request starts from the name's level again.
    #[tokio::test]
    async fn a_level_refused_again_is_reported_and_not_remembered() {
        let (url, stub) = sse_stub::serve_in_turn(&[LEVEL_REFUSED, LEVEL_REFUSED, DESCRIBED]);
        let client = client_at(&url, "gemini-3.8-flash");
        let err = client
            .describe(req(), &CancellationToken::new())
            .await
            .expect_err("both attempts were refused")
            .to_string();
        assert!(err.contains("Gemini video: status 400"), "{err}");
        assert!(err.contains("Thinking level MINIMAL"), "{err}");
        assert_eq!(client.learned_level(), None);
        client
            .describe(req(), &CancellationToken::new())
            .await
            .expect("the stub takes the third request");
        assert_eq!(
            levels_sent(&stub.join().unwrap()),
            ["minimal", "low", "minimal"]
        );
    }

    /// Any other `400` is reported as it came, after one request: the stub has
    /// a single answer, so a second request would meet a closed port.
    #[tokio::test]
    async fn another_refusal_is_reported_as_it_came() {
        let (url, stub) = sse_stub::serve_in_turn(&[(
            sse_stub::BAD_REQUEST,
            sse_stub::JSON,
            r#"{"error":{"code":400,"message":"The video is not available.","status":"INVALID_ARGUMENT"}}"#,
        )]);
        let client = client_at(&url, "gemini-3.8-flash");
        let err = client
            .describe(req(), &CancellationToken::new())
            .await
            .expect_err("a 400 about the video is an error")
            .to_string();
        assert!(err.contains("The video is not available."), "{err}");
        assert_eq!(stub.join().unwrap().len(), 1);
        assert_eq!(client.learned_level(), None);
    }

    /// A 2.5 body mutes with a budget and carries no level: the same words in
    /// an answer are not a refusal there is anything to do about.
    #[tokio::test]
    async fn a_body_without_a_level_is_never_asked_again() {
        let (url, stub) = sse_stub::serve_in_turn(&[LEVEL_REFUSED]);
        let client = client_at(&url, "gemini-2.5-flash");
        client
            .describe(req(), &CancellationToken::new())
            .await
            .expect_err("the refusal is the answer");
        assert_eq!(stub.join().unwrap().len(), 1);
    }

    /// What was learned is what the next body is built with.
    #[test]
    fn a_learned_level_replaces_the_names_guess() {
        let c = client("gemini-3.8-flash");
        assert_eq!(c.body(&req()).pointer(LEVEL_AT), Some(&json!("minimal")));
        c.muted_level.set("low").unwrap();
        assert_eq!(c.body(&req()).pointer(LEVEL_AT), Some(&json!("low")));
    }
}

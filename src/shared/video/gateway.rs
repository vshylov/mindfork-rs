//! A YouTube video through the OpenRouter gateway: the link as a `video_url`
//! part of a chat completion (docs/research/openrouter-mode.md §4.5, fork F10,
//! §14).
//!
//! The same video costs what it costs through Google's own API — measured, the
//! 67 s video is 6147 prompt tokens either way — and the transcripts agree
//! almost word for word. What the gateway **does not carry** is what makes this
//! client differ from [`GeminiVideo`](super::gemini::GeminiVideo):
//!
//! - **no segment bounds** — four spellings tried, none honoured — so the whole
//!   video is read and charged, and [`VideoUnderstanding::reads_segments`] says
//!   so: the tool names the segment in words and measures the whole video
//!   against its ceiling;
//! - **no media resolution** — three spellings, none honoured;
//! - **no guarantee that a `200` is about the video.** A link the gateway does
//!   not read as a video is answered `200` with a plausible description made up
//!   from the link, or from the page behind it. What tells the two apart is
//!   `usage.prompt_tokens_details.video_tokens`: **an answer with no video
//!   tokens is an error**, whatever its text says.
//!
//! And one refusal arrives as a `200` whose body is the error envelope — no
//! `choices`, an `error` with a code of its own — so the body is read before the
//! status is believed.
//!
//! The link has to be the canonical one. Measured on 2026-09-29: the same video
//! with one more parameter in its address (`&feature=share`, `&t=20s`) was not
//! read as a video — 551 337 prompt tokens where the video is 6 116, ninety
//! times the price, and a description of a page. `youtube_watch` builds the
//! address from the video's id and hands over nothing else.

use std::sync::{Mutex, PoisonError};

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{VideoAnswer, VideoConfig, VideoRequest, VideoUnderstanding};
use crate::shared::api::openai::{ModelEnvelope, attributed};

/// What the errors of this client are said about.
const SUBJECT: &str = "OpenRouter video";

/// Client for `{base}/chat/completions` with a video part. Non-streaming, like
/// the native client: the tool returns one block of text.
pub struct GatewayVideo {
    http: reqwest::Client,
    cfg: VideoConfig,
    /// The lowest reasoning effort the gateway lists for the model, once the
    /// gateway has answered about it: `Some(None)` — it lists none. Not set by
    /// an outage, which is asked about again with the next video.
    effort: Mutex<Option<Option<&'static str>>>,
}

impl GatewayVideo {
    pub fn new(cfg: VideoConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            cfg: VideoConfig {
                base_url: cfg.base_url.trim_end_matches('/').to_string(),
                model: cfg.model.trim().to_string(),
                ..cfg
            },
            effort: Mutex::new(None),
        }
    }

    /// The key, and the application's name where the provider's switch says so.
    fn signed(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let rb = rb.bearer_auth(&self.cfg.api_key);
        if self.cfg.attribution {
            attributed(rb)
        } else {
            rb
        }
    }

    /// The lowest reasoning effort the gateway's entry for the model lists, or
    /// `None` to say nothing about reasoning.
    ///
    /// Describing a video is not a reasoning task, and the reply's ceiling
    /// covers the reasoning — so as little of it as the model allows. Gemini 3.5
    /// cannot be told to stop (`400 "Reasoning is mandatory for this endpoint"`)
    /// and lists `minimal`; what a model lists is its own, which is why the
    /// entry is asked rather than the name read.
    async fn lowest_effort(&self, cancel: &CancellationToken) -> Option<&'static str> {
        if let Some(known) = *self.effort.lock().unwrap_or_else(PoisonError::into_inner) {
            return known;
        }
        let url = format!("{}/model/{}", self.cfg.base_url, self.cfg.model);
        let answer = tokio::select! {
            biased;
            _ = cancel.cancelled() => return None,
            r = self.signed(self.http.get(&url)).send() => r,
        };
        let listed = match answer {
            Ok(resp) if resp.status().is_success() => resp
                .json::<ModelEnvelope>()
                .await
                .ok()
                .and_then(|entry| entry.data.lowest_effort()),
            // An outage, or a limit reached: not an answer about the model.
            Ok(resp)
                if resp.status().is_server_error()
                    || resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS =>
            {
                tracing::debug!(%url, status = %resp.status(), "the gateway's entry is unavailable for now");
                return None;
            }
            // The gateway's answer — no such model: the request that follows
            // will say so in its own words.
            Ok(_) => None,
            Err(err) => {
                tracing::debug!(%url, error = %err, "no answer about the video model");
                return None;
            }
        };
        *self.effort.lock().unwrap_or_else(PoisonError::into_inner) = Some(listed);
        listed
    }

    /// The request body. Pure — the shape is what the tests assert on. No
    /// bounds, no resolution and no `processing`: the first two are not carried,
    /// and the third moves the video somewhere the usage does not show.
    pub(crate) fn body(&self, req: &VideoRequest, effort: Option<&str>) -> Value {
        let mut body = json!({
            "model": self.cfg.model,
            "max_tokens": req.max_output_tokens,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": req.prompt },
                    { "type": "video_url", "video_url": { "url": req.url } },
                ],
            }],
        });
        if let Some(effort) = effort {
            body["reasoning"] = json!({ "effort": effort });
        }
        body
    }
}

/// What the gateway said went wrong, in its own words and — where it passed a
/// provider's refusal on — in the provider's: *"Provider returned error: The
/// caller does not have permission"* is a video that is private or not there.
fn refusal_in(body: &Value) -> Option<String> {
    let error = body.get("error")?;
    let said = error.get("message").and_then(Value::as_str)?.trim();
    let upstream = error
        .pointer("/metadata/raw")
        .and_then(Value::as_str)
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|raw| Some(raw.pointer("/error/message")?.as_str()?.trim().to_string()))
        .filter(|m| !m.is_empty() && m != said);
    Some(match upstream {
        Some(upstream) => format!("{said}: {upstream}"),
        None => said.to_string(),
    })
}

/// The answer out of what the gateway sent back. Pure — tested without a
/// network, on the bodies the gateway answered with.
///
/// The order is the order of what can be trusted: the body's `error` before the
/// status, since a refusal may come as a `200`; the video tokens before the
/// text, since a text may come without a video.
fn answer_from(status: reqwest::StatusCode, text: &str) -> Result<VideoAnswer> {
    let body: Value = match serde_json::from_str(text) {
        Ok(body) => body,
        Err(_) => {
            let detail: String = text.trim().chars().take(500).collect();
            anyhow::bail!("{SUBJECT}: status {status}: {detail}");
        }
    };
    if let Some(said) = refusal_in(&body) {
        anyhow::bail!("{SUBJECT}: status {status}: {said}");
    }
    anyhow::ensure!(status.is_success(), "{SUBJECT}: status {status}");

    let usage = body.get("usage");
    let tokens = |path: &str| usage.and_then(|u| u.pointer(path)).and_then(Value::as_u64);
    if tokens("/prompt_tokens_details/video_tokens").unwrap_or(0) == 0 {
        anyhow::bail!(
            "{SUBJECT}: the gateway answered without reading the video — no video tokens \
             among the {} prompt tokens it counted — so what it said is not about the video",
            tokens("/prompt_tokens").unwrap_or(0)
        );
    }

    let choice = body.pointer("/choices/0");
    let finish = choice
        .and_then(|c| c.get("finish_reason"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let said = choice
        .and_then(|c| c.pointer("/message/content"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if said.trim().is_empty() {
        if finish.is_empty() {
            anyhow::bail!("{SUBJECT}: no description of the video came back");
        }
        anyhow::bail!("{SUBJECT}: no description of the video came back (finish reason: {finish})");
    }
    Ok(VideoAnswer {
        text: said.to_string(),
        truncated: finish == "length",
    })
}

#[async_trait::async_trait]
impl VideoUnderstanding for GatewayVideo {
    async fn describe(&self, req: VideoRequest, cancel: &CancellationToken) -> Result<VideoAnswer> {
        let effort = self.lowest_effort(cancel).await;
        let url = format!("{}/chat/completions", self.cfg.base_url);
        let rb = self
            .signed(self.http.post(&url))
            .json(&self.body(&req, effort));
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => anyhow::bail!("video request cancelled"),
            r = rb.send() => r.with_context(|| format!("POST {url}"))?,
        };
        let status = resp.status();
        let text = tokio::select! {
            biased;
            _ = cancel.cancelled() => anyhow::bail!("video request cancelled"),
            t = resp.text() => t.context("reading the gateway's answer about the video")?,
        };
        let answer = answer_from(status, &text);
        if let Err(err) = &answer {
            tracing::warn!(%status, error = %err, "the gateway did not describe the video");
        }
        answer
    }

    fn reads_segments(&self) -> bool {
        false
    }
}

//! Video understanding: the contract + the provider client, for the
//! `youtube_watch` tool (spec §9.9, docs/research/youtube-integration.md).
//!
//! Like TTS (ADR 0009) and unlike everything behind [`EngineBackend`], this slot
//! is **independent of the chat engine** — and here that is not a convenience but
//! the whole point: measured 2026-08-01, Gemini is the only provider that ingests
//! video at all (OpenAI's Responses API and Anthropic take text and images only).
//! Routing the call through the chat engine would therefore hand the capability
//! to exactly the users least likely to need it, and deny it to the ones on a
//! local `llama-server`. So the tool calls Gemini out of band and returns text
//! into the conversation, whatever the chat engine is.
//!
//! The client is **stateless**: built from a config snapshot when the registry is
//! built, no manager slot.
//!
//! [`EngineBackend`]: crate::shared::api::EngineBackend

pub mod gemini;

use std::fmt;

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use crate::shared::config::{MediaResolution, VideoSettings};

/// One "watch this and tell me about it" request.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoRequest {
    /// A public video URL the provider ingests directly (no download on our side).
    pub url: String,
    /// What to say about it — already localized by the caller (axis A).
    pub prompt: String,
    /// Optional segment, in seconds from the start. Both bounds are honored by
    /// the provider (`videoMetadata.startOffset`/`endOffset`) and are the only
    /// way to look at a long video without paying for all of it.
    pub start_secs: Option<u32>,
    pub end_secs: Option<u32>,
    /// Ceiling on the answer.
    pub max_output_tokens: usize,
}

/// What the provider answered.
///
/// [`Self::truncated`] exists because a transcript makes truncation a
/// **correctness** problem rather than a cosmetic one: an answer cut off at
/// `max_output_tokens` still arrives as perfectly good-looking text, and a model
/// told "here is the transcript" would believe it has the whole thing — losing
/// exactly the guarantee `attachment_read`'s page walk is there to give. For a
/// description it barely matters; hence one flag rather than a whole finish
/// reason. See docs/history/youtube-transcript.md §3 F5.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoAnswer {
    pub text: String,
    /// The provider stopped at the output ceiling — the answer is a prefix.
    pub truncated: bool,
}

/// A provider that can look at a video behind a URL and describe it.
#[async_trait::async_trait]
pub trait VideoUnderstanding: Send + Sync {
    /// Returns the model's answer. Cancellation must be honored: a long video is
    /// a long request, and `Esc` has to work through it.
    async fn describe(&self, req: VideoRequest, cancel: &CancellationToken) -> Result<VideoAnswer>;
}

/// Resolved configuration for the video slot: settings plus the key that was
/// found for them. Built once (registry build), then owned by the client.
///
/// `Debug` is **hand-written to redact the key**: this type is reachable from
/// `ToolConfig`, which derives `Debug`, and a plaintext key must not be one
/// stray `{:?}` away from a log file.
#[derive(Clone)]
pub struct VideoConfig {
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub media_resolution: MediaResolution,
    /// `0` — no ceiling.
    pub max_minutes: u32,
}

impl fmt::Debug for VideoConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VideoConfig")
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .field("media_resolution", &self.media_resolution)
            .field("max_minutes", &self.max_minutes)
            .finish()
    }
}

/// Resolves settings + the stored provider key (ADR 0008) into a usable config.
/// `None` — not configured (no model or no key anywhere); the tool reports that
/// and degrades to metadata rather than disappearing, so the model can explain
/// itself to the user (fork R5a).
pub fn resolve_config(video: &VideoSettings, stored_key: Option<String>) -> Option<VideoConfig> {
    let model = non_empty(video.model_name.clone())?;
    let key = stored_key
        .filter(|k| !k.trim().is_empty())
        .or_else(|| env_key(video.api_key_env.as_deref()))?;
    let base_url = non_empty(video.url.clone()).unwrap_or_else(|| {
        crate::shared::config::CloudProvider::Gemini
            .chat_base_url()
            .to_string()
    });
    Some(VideoConfig {
        model,
        base_url,
        api_key: key,
        media_resolution: video.media_resolution,
        max_minutes: video.max_minutes,
    })
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn env_key(var: Option<&str>) -> Option<String> {
    let var = var?.trim();
    (!var.is_empty())
        .then(|| std::env::var(var).ok())
        .flatten()
        .filter(|v| !v.trim().is_empty())
}

/// Doesn't swallow the provider's error body (ADR 0004) — mirrors
/// `shared::tts::error_body`.
pub(crate) async fn error_body(what: &str, resp: reqwest::Response) -> anyhow::Error {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let detail: String = body.trim().chars().take(500).collect();
    tracing::warn!(%status, body = %detail, "{what} returned an error status");
    if detail.is_empty() {
        anyhow::anyhow!("{what}: status {status}")
    } else {
        anyhow::anyhow!("{what}: status {status}: {detail}")
    }
}

#[cfg(test)]
pub(crate) mod mock {
    use super::*;
    use std::sync::Mutex;

    /// Records the request and replies with fixed text — lets the tool be tested
    /// without a network or a key.
    pub struct MockVideo {
        pub last: Mutex<Option<VideoRequest>>,
        pub reply: std::result::Result<VideoAnswer, String>,
    }

    impl MockVideo {
        pub fn ok(reply: &str) -> Self {
            Self {
                last: Mutex::new(None),
                reply: Ok(VideoAnswer {
                    text: reply.to_string(),
                    truncated: false,
                }),
            }
        }
        /// An answer the provider cut off at the output ceiling.
        pub fn truncated(reply: &str) -> Self {
            Self {
                last: Mutex::new(None),
                reply: Ok(VideoAnswer {
                    text: reply.to_string(),
                    truncated: true,
                }),
            }
        }
        pub fn failing(err: &str) -> Self {
            Self {
                last: Mutex::new(None),
                reply: Err(err.to_string()),
            }
        }
        pub fn taken(&self) -> Option<VideoRequest> {
            self.last.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl VideoUnderstanding for MockVideo {
        async fn describe(
            &self,
            req: VideoRequest,
            _cancel: &CancellationToken,
        ) -> Result<VideoAnswer> {
            *self.last.lock().unwrap() = Some(req);
            self.reply.clone().map_err(|e| anyhow::anyhow!("{e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unconfigured_without_model_or_key() {
        let mut v = VideoSettings::default();
        // A model is there by default, but no key anywhere → not configured.
        assert!(resolve_config(&v, None).is_none());
        // A stored key is enough — nothing has to be re-entered (ADR 0008).
        assert!(resolve_config(&v, Some("k".into())).is_some());
        // No model → not configured, even with a key.
        v.model_name = Some("   ".into());
        assert!(resolve_config(&v, Some("k".into())).is_none());
    }

    #[test]
    fn blank_stored_key_falls_through_to_env() {
        // A blank stored key must not count as "configured" — otherwise the tool
        // would send an empty Authorization and fail confusingly.
        let v = VideoSettings::default();
        assert!(resolve_config(&v, Some("   ".into())).is_none());
    }

    #[test]
    fn base_url_defaults_to_the_native_gemini_path() {
        let cfg = resolve_config(&VideoSettings::default(), Some("k".into())).unwrap();
        assert_eq!(
            cfg.base_url,
            crate::shared::config::CloudProvider::Gemini.chat_base_url()
        );
        // An override wins.
        let v = VideoSettings {
            url: Some("https://proxy.example/v1beta".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_config(&v, Some("k".into())).unwrap().base_url,
            "https://proxy.example/v1beta"
        );
    }

    #[test]
    fn debug_redacts_the_key() {
        // This type is reachable from ToolConfig, which derives Debug.
        let cfg = resolve_config(&VideoSettings::default(), Some("secret-key".into())).unwrap();
        let dump = format!("{cfg:?}");
        assert!(
            !dump.contains("secret-key"),
            "key leaked into Debug: {dump}"
        );
        assert!(dump.contains("redacted"), "got: {dump}");
    }
}

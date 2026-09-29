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
//! Gemini is reached two ways ([`VideoProvider`]): by Google's own API
//! ([`gemini`]) and through the OpenRouter gateway ([`gateway`]) — the same
//! models behind the key a user of the gateway has. The two differ in what they
//! carry, which [`VideoUnderstanding::reads_segments`] is for.
//!
//! The client is **stateless**: built from a config snapshot when the registry is
//! built, no manager slot.
//!
//! [`EngineBackend`]: crate::shared::api::EngineBackend

pub mod gateway;
pub mod gemini;

#[cfg(test)]
pub(crate) mod gateway_live_tests;
#[cfg(test)]
mod gateway_tests;

use std::fmt;
use std::sync::Arc;

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use crate::shared::config::{MediaResolution, VideoProvider, VideoSettings};

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

    /// Whether [`VideoRequest::start_secs`] and [`VideoRequest::end_secs`] are
    /// the provider's to honour: it looks at the segment and charges for it.
    /// `false` — the whole video is read whatever the bounds say, so the caller
    /// has to name the segment in the prompt, measure the **whole** video
    /// against its ceiling, and say that the whole of it was read.
    fn reads_segments(&self) -> bool {
        true
    }
}

/// The client of the provider the config names.
pub fn client(cfg: VideoConfig) -> Arc<dyn VideoUnderstanding> {
    match cfg.provider {
        VideoProvider::Gemini => Arc::new(gemini::GeminiVideo::new(cfg)),
        VideoProvider::OpenRouter => Arc::new(gateway::GatewayVideo::new(cfg)),
    }
}

/// Resolved configuration for the video slot: settings plus the key that was
/// found for them. Built once (registry build), then owned by the client.
///
/// `Debug` is **hand-written to redact the key**: this type is reachable from
/// `ToolConfig`, which derives `Debug`, and a plaintext key must not be one
/// stray `{:?}` away from a log file.
#[derive(Clone)]
pub struct VideoConfig {
    pub provider: VideoProvider,
    /// Name the application to the gateway (`openrouter.attribution`, fork F5
    /// of docs/research/openrouter-mode.md). Read by the gateway's client
    /// alone.
    pub attribution: bool,
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
            .field("provider", &self.provider)
            .field("attribution", &self.attribution)
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .field("media_resolution", &self.media_resolution)
            .field("max_minutes", &self.max_minutes)
            .finish()
    }
}

/// Resolves settings + the stored key of **the provider they name** (ADR 0008,
/// [`VideoSettings::secret_key`]) into a usable config. `None` — not configured
/// (no model or no key anywhere); the tool reports that and degrades to metadata
/// rather than disappearing, so the model can explain itself to the user (fork
/// R5a). `attribution` is the gateway's provider-wide switch.
pub fn resolve_config(
    video: &VideoSettings,
    stored_key: Option<String>,
    attribution: bool,
) -> Option<VideoConfig> {
    let gateway = &video.openrouter;
    let (model, url, key_env) = match video.provider {
        VideoProvider::Gemini => (&video.model_name, &video.url, &video.api_key_env),
        VideoProvider::OpenRouter => (&gateway.model_name, &gateway.url, &gateway.api_key_env),
    };
    let model = non_empty(model.clone())?;
    let key = stored_key
        .filter(|k| !k.trim().is_empty())
        .or_else(|| env_key(key_env.as_deref()))?;
    let base_url = non_empty(url.clone()).unwrap_or_else(|| {
        let provider = video.provider.cloud_provider();
        provider.chat_base_url().to_string()
    });
    Some(VideoConfig {
        provider: video.provider,
        attribution: attribution && video.provider == VideoProvider::OpenRouter,
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
        /// What [`VideoUnderstanding::reads_segments`] answers.
        pub segments: bool,
    }

    impl MockVideo {
        fn replying(reply: std::result::Result<VideoAnswer, String>) -> Self {
            Self {
                last: Mutex::new(None),
                reply,
                segments: true,
            }
        }
        pub fn ok(reply: &str) -> Self {
            Self::replying(Ok(VideoAnswer {
                text: reply.to_string(),
                truncated: false,
            }))
        }
        /// An answer the provider cut off at the output ceiling.
        pub fn truncated(reply: &str) -> Self {
            Self::replying(Ok(VideoAnswer {
                text: reply.to_string(),
                truncated: true,
            }))
        }
        pub fn failing(err: &str) -> Self {
            Self::replying(Err(err.to_string()))
        }
        /// A provider that reads the whole video whatever the bounds say, as
        /// the gateway does.
        pub fn whole_video(self) -> Self {
            Self {
                segments: false,
                ..self
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

        fn reads_segments(&self) -> bool {
            self.segments
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `resolve_config` with the gateway's switch on, as the settings have it
    /// until somebody turns it off.
    fn resolved(v: &VideoSettings, key: Option<&str>) -> Option<VideoConfig> {
        resolve_config(v, key.map(str::to_string), true)
    }

    #[test]
    fn unconfigured_without_model_or_key() {
        let mut v = VideoSettings::default();
        // A model is there by default, but no key anywhere → not configured.
        assert!(resolved(&v, None).is_none());
        // A stored key is enough — nothing has to be re-entered (ADR 0008).
        assert!(resolved(&v, Some("k")).is_some());
        // No model → not configured, even with a key.
        v.model_name = Some("   ".into());
        assert!(resolved(&v, Some("k")).is_none());
    }

    #[test]
    fn blank_stored_key_falls_through_to_env() {
        // A blank stored key must not count as "configured" — otherwise the tool
        // would send an empty Authorization and fail confusingly.
        let v = VideoSettings::default();
        assert!(resolved(&v, Some("   ")).is_none());
    }

    #[test]
    fn base_url_defaults_to_the_native_gemini_path() {
        let cfg = resolved(&VideoSettings::default(), Some("k")).unwrap();
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
            resolved(&v, Some("k")).unwrap().base_url,
            "https://proxy.example/v1beta"
        );
    }

    /// Each provider is resolved from what is its own — the model, the
    /// address, the variable its key is read from — and from nothing of the
    /// other's; the gateway has no model to fall back on; and the application
    /// is named to the gateway alone, where the switch says so.
    #[test]
    fn each_provider_is_resolved_from_its_own_section() {
        use crate::shared::config::{CloudProvider, VideoGatewaySettings};
        let mut v = VideoSettings {
            provider: VideoProvider::OpenRouter,
            url: Some("https://gemini.example/v1beta".into()),
            // A variable that is certainly unset: Gemini's, and not read here.
            api_key_env: Some("MINDFORK_DEFINITELY_UNSET_VAR_42".into()),
            ..Default::default()
        };
        assert!(
            resolved(&v, Some("k")).is_none(),
            "Gemini's default model is not the gateway's"
        );
        v.openrouter.model_name = Some(" google/gemini-3.5-flash ".into());
        let cfg = resolved(&v, Some("k")).expect("a model and a key");
        assert_eq!(cfg.provider, VideoProvider::OpenRouter);
        assert_eq!(cfg.model, "google/gemini-3.5-flash");
        assert_eq!(cfg.base_url, CloudProvider::OpenRouter.chat_base_url());
        assert!(cfg.attribution);
        assert!(
            !resolve_config(&v, Some("k".into()), false)
                .unwrap()
                .attribution
        );

        // The key's variable is the gateway section's: `PATH` is read, never
        // sent — nothing is asked here.
        v.openrouter.api_key_env = Some("PATH".into());
        assert!(resolved(&v, None).is_some());
        v.openrouter = VideoGatewaySettings {
            url: Some("https://eu.openrouter.ai/api/v1".into()),
            ..v.openrouter
        };
        assert_eq!(
            resolved(&v, Some("k")).unwrap().base_url,
            "https://eu.openrouter.ai/api/v1"
        );

        // Back on Gemini everything is Gemini's again, and nobody is named.
        v.provider = VideoProvider::Gemini;
        let cfg = resolved(&v, Some("k")).expect("Gemini's defaults");
        assert_eq!(cfg.provider, VideoProvider::Gemini);
        assert_eq!(cfg.model, crate::shared::config::DEFAULT_VIDEO_MODEL);
        assert_eq!(cfg.base_url, "https://gemini.example/v1beta");
        assert!(!cfg.attribution, "the switch is the gateway's");
        assert!(
            resolved(&v, None).is_none(),
            "the gateway's variable is not Gemini's"
        );
    }

    /// The contract's default is the native provider's: a segment is cut by
    /// the provider. The gateway's client says otherwise.
    #[test]
    fn the_provider_says_whether_a_segment_is_its_to_cut() {
        let built = |provider| {
            let v = VideoSettings {
                provider,
                openrouter: crate::shared::config::VideoGatewaySettings {
                    model_name: Some("google/gemini-3.5-flash".into()),
                    ..Default::default()
                },
                ..Default::default()
            };
            client(resolved(&v, Some("k")).expect("configured")).reads_segments()
        };
        assert!(built(VideoProvider::Gemini));
        assert!(!built(VideoProvider::OpenRouter));
    }

    #[test]
    fn debug_redacts_the_key() {
        // This type is reachable from ToolConfig, which derives Debug.
        let cfg = resolved(&VideoSettings::default(), Some("secret-key")).unwrap();
        let dump = format!("{cfg:?}");
        assert!(
            !dump.contains("secret-key"),
            "key leaked into Debug: {dump}"
        );
        assert!(dump.contains("redacted"), "got: {dump}");
    }
}

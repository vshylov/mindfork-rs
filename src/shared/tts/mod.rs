//! Speech synthesis (TTS): the synthesis contract + provider clients +
//! playback. The speech slot is **independent of the chat engine** (Anthropic
//! has no TTS at all), so the provider is configured separately — like the
//! dedicated embedding server (ADR 0002). Clients are **stateless**: built from
//! a config snapshot per call, no manager slot is needed.
//!
//! Layout (docs/research/tts.md §8):
//! - [`openai`] — `POST /v1/audio/speech`: the OpenAI cloud **and** any
//!   third-party OpenAI-compatible TTS server (different base URL/key/response
//!   format);
//! - [`gemini`] — native `generateContent` with `responseModalities:["AUDIO"]`;
//! - [`playback`] — playback: an `rodio` source queue + cancellation.
//!
//! A local sidecar (managed mode) is **future work** (a spike: local engines
//! are NO-GO for Russian, or need their own frontend; docs/research/tts.md
//! §13). The primary path is the cloud (OpenAI); offline — `external` (your own
//! OpenAI-compatible server).

pub mod gemini;
pub mod openai;
pub mod playback;

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use crate::shared::config::{TtsMode, TtsSettings};

/// A synthesized speech fragment.
///
/// Clouds return **raw PCM with no header** (OpenAI `response_format:"pcm"` —
/// 24 kHz s16le mono; Gemini `audio/L16;codec=pcm;rate=…`), so they need no
/// decoder at all — samples go straight into the playback buffer. Third-party
/// servers respond with a container (`wav` — the most portable), parsed by the
/// decoder.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioClip {
    /// Raw signed 16-bit little-endian PCM with no header.
    Pcm {
        sample_rate: u32,
        channels: u16,
        bytes: Vec<u8>,
    },
    /// Audio in a container (wav/mp3) — with a header, parsed by the decoder.
    Encoded(Vec<u8>),
}

impl AudioClip {
    /// Whether the clip is empty (nothing to play).
    pub fn is_empty(&self) -> bool {
        match self {
            AudioClip::Pcm { bytes, .. } => bytes.len() < 2,
            AudioClip::Encoded(bytes) => bytes.is_empty(),
        }
    }
}

/// A speech-synthesis provider. Implementations are thin HTTP clients (see the
/// module).
#[async_trait::async_trait]
pub trait TtsEngine: Send + Sync {
    /// Synthesizes speech for a piece of text. Cancellable: on `cancel` the
    /// request is aborted (with an error), accumulated output is not used.
    async fn synthesize(&self, text: &str, cancel: &CancellationToken) -> Result<AudioClip>;

    /// The ceiling on a single request's text length (characters) — text is
    /// cut into chunks by it. A hard provider limit, not a preference.
    fn max_input_chars(&self) -> usize;
}

/// Why speech isn't configured. A structured error (not text): the message is
/// built by the UI in the interface language — axis B, a precedent from
/// `supervisor::ApiKeyError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtsSetupError {
    /// No model name set.
    Model,
    /// No API key (neither entered in settings nor in an env variable).
    ApiKey,
    /// No external server URL set.
    Url,
}

/// A pair of speech engines: the assistant's voice (always) and an optional
/// user voice.
pub type TtsEnginePair = (Box<dyn TtsEngine>, Option<Box<dyn TtsEngine>>);

/// Engines for multi-voice speech: `(assistant, opt. user)`. `stored_key` — the
/// stored key of whatever the active mode reads (a cloud provider's, shared with
/// chat, or the `external` slot's own — ADR 0008, docs/history/external-api-key.md):
/// already decrypted by the caller; if absent, the key is read from an env
/// variable. The second engine is built
/// **only** if the active mode has a separate "user voice" set and it differs
/// from the assistant's voice — then `/tts all`/`/tts N` read the user's turns
/// with it (spec §11.9). Otherwise `None` → everything in one voice.
pub fn engines_from_config(
    tts: &TtsSettings,
    stored_key: Option<String>,
) -> std::result::Result<TtsEnginePair, TtsSetupError> {
    let (assistant_voice, user_voice) = tts.active_voices();
    let assistant = build_engine(tts, stored_key.clone(), None)?;
    let user = match user_voice {
        Some(uv) if Some(uv) != assistant_voice => {
            Some(build_engine(tts, stored_key, Some(uv.to_string()))?)
        }
        _ => None,
    };
    Ok((assistant, user))
}

/// Shared client constructor. `voice_override` (`Some`) replaces the voice from
/// settings — this is how the second engine for user turns is built, without
/// duplicating model/key/base resolution.
fn build_engine(
    tts: &TtsSettings,
    stored_key: Option<String>,
    voice_override: Option<String>,
) -> std::result::Result<Box<dyn TtsEngine>, TtsSetupError> {
    let speed = tts.speed;
    match tts.mode {
        TtsMode::OpenAi | TtsMode::Gemini => {
            let cloud = tts.cloud().ok_or(TtsSetupError::Model)?;
            let model = non_empty(cloud.model_name.clone()).ok_or(TtsSetupError::Model)?;
            let key = resolve_key(stored_key, cloud.api_key_env.as_deref())
                .ok_or(TtsSetupError::ApiKey)?;
            let provider = tts.mode.cloud_provider().ok_or(TtsSetupError::Model)?;
            let base = non_empty(cloud.url.clone())
                .unwrap_or_else(|| provider.chat_base_url().to_string());
            let voice = voice_override.or_else(|| non_empty(cloud.voice.clone()));
            let instructions = non_empty(cloud.instructions.clone());
            Ok(match tts.mode {
                TtsMode::Gemini => Box::new(gemini::GeminiTts::new(
                    base,
                    key,
                    model,
                    voice,
                    instructions,
                )),
                // OpenAI (cloud): ask for raw PCM — no decoder needed.
                _ => Box::new(openai::OpenAiTts::cloud(
                    base,
                    key,
                    model,
                    voice,
                    instructions,
                    speed,
                )),
            })
        }
        TtsMode::External => {
            let url = non_empty(tts.external.url.clone()).ok_or(TtsSetupError::Url)?;
            Ok(Box::new(openai::OpenAiTts::external(
                url,
                // The same resolution as the cloud arm above, but **without** the
                // `ok_or`: here having no key at all is legitimate, since a local speech
                // server needs none (docs/history/external-api-key.md F3).
                resolve_key(stored_key, tts.external.api_key_env.as_deref()),
                non_empty(tts.external.model_name.clone()),
                voice_override.or_else(|| non_empty(tts.external.voice.clone())),
                speed,
            )))
        }
    }
}

/// The speech server's key: a key **stored for this slot** wins over the variable
/// named in settings, and a blank stored key counts as absent (ADR 0008 §3,
/// docs/history/external-api-key.md F2). `None` — neither is available; what that
/// means is the caller's to decide, and the two modes decide differently: a cloud
/// refuses ([`TtsSetupError::ApiKey`]), an external server simply sends no
/// authorization. The mirror of `app::supervisor::resolve_api_key`, kept separate
/// because that one lives above this layer and returns a *localizable* error.
fn resolve_key(stored: Option<String>, api_key_env: Option<&str>) -> Option<String> {
    stored
        .filter(|k| !k.trim().is_empty())
        .or_else(|| env_key(api_key_env))
}

/// An env variable's value by name (the fallback under a stored key, ADR 0008).
fn env_key(var: Option<&str>) -> Option<String> {
    let var = var?.trim();
    if var.is_empty() {
        return None;
    }
    std::env::var(var).ok().filter(|v| !v.is_empty())
}

/// A non-empty string, or `None` (an empty field in settings is stored as
/// `Some("")`).
fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Doesn't swallow the provider's error body: the status + reason are logged
/// and land in the error text (truncated to 500 chars) — as in the engine
/// clients (ADR 0004).
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
mod tests {
    use super::*;
    use crate::shared::config::{TtsCloudSettings, TtsExternalSettings};

    #[test]
    fn cloud_requires_model_and_key() {
        let mut tts = TtsSettings {
            openai: TtsCloudSettings {
                model_name: None,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            engines_from_config(&tts, Some("sk-x".into())).err(),
            Some(TtsSetupError::Model)
        );
        tts.openai.model_name = Some("gpt-4o-mini-tts".into());
        // No key at all, neither stored nor in env → a clear structured error.
        assert_eq!(
            engines_from_config(&tts, None).err(),
            Some(TtsSetupError::ApiKey)
        );
        // A stored key (ADR 0008) is enough — nothing needs re-entering.
        assert!(engines_from_config(&tts, Some("sk-x".into())).is_ok());
    }

    #[test]
    fn external_requires_url_only() {
        let tts = TtsSettings {
            mode: TtsMode::External,
            external: TtsExternalSettings::default(),
            ..Default::default()
        };
        assert_eq!(
            engines_from_config(&tts, None).err(),
            Some(TtsSetupError::Url)
        );
        let tts = TtsSettings {
            mode: TtsMode::External,
            external: TtsExternalSettings {
                url: Some("http://127.0.0.1:8880/v1".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        // A local server needs neither a key nor a model.
        assert!(engines_from_config(&tts, None).is_ok());
    }

    /// The speech key follows the same chain as the engine's: a key stored for this
    /// slot first, then the variable named in settings, and a blank stored key counts
    /// as absent rather than as an empty `Authorization`. `PATH` is read rather than
    /// set — `set_var` is `unsafe` in edition 2024 and races the whole test binary.
    #[test]
    fn stored_key_wins_over_the_named_variable() {
        let from_env = std::env::var("PATH").unwrap();
        assert_eq!(
            resolve_key(Some("sk-slot".into()), Some("PATH")).as_deref(),
            Some("sk-slot")
        );
        assert_eq!(resolve_key(None, Some("PATH")), Some(from_env));
        assert!(
            resolve_key(Some("  ".into()), Some("PATH")).is_some(),
            "a blank stored key falls through to the variable, not to nothing"
        );
        assert_eq!(resolve_key(Some("   ".into()), None), None);
        assert_eq!(resolve_key(None, None), None);
        assert_eq!(
            resolve_key(None, Some("MINDFORK_DEFINITELY_UNSET_VAR_42")),
            None
        );
    }

    /// An external speech server takes its **own** stored key (`ExternalSlot::Tts`),
    /// and having none stays legitimate — the two halves of
    /// docs/history/external-api-key.md for this slot.
    #[test]
    fn external_takes_a_stored_key_but_does_not_require_one() {
        let tts = TtsSettings {
            mode: TtsMode::External,
            external: TtsExternalSettings {
                url: Some("http://127.0.0.1:8880/v1".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            tts.secret_key(),
            Some(crate::shared::secrets::SecretKey::External(
                crate::shared::secrets::ExternalSlot::Tts
            )),
            "the speech slot must not read a provider's key in external mode"
        );
        assert!(engines_from_config(&tts, Some("sk-voice".into())).is_ok());
        assert!(engines_from_config(&tts, None).is_ok());
    }

    #[test]
    fn blank_fields_count_as_unset() {
        let tts = TtsSettings {
            openai: TtsCloudSettings {
                model_name: Some("   ".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            engines_from_config(&tts, Some("sk-x".into())).err(),
            Some(TtsSetupError::Model)
        );
    }

    #[test]
    fn second_engine_built_only_when_user_voice_set_and_differs() {
        let base = TtsSettings {
            openai: TtsCloudSettings {
                model_name: Some("gpt-4o-mini-tts".into()),
                voice: Some("onyx".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        // No user voice → a single engine.
        let (_a, user) = engines_from_config(&base, Some("sk-x".into())).unwrap();
        assert!(user.is_none(), "no user_voice → no second engine is built");

        // Set and differs → the second one is built.
        let mut with_user = base.clone();
        with_user.openai.user_voice = Some("nova".into());
        let (_a, user) = engines_from_config(&with_user, Some("sk-x".into())).unwrap();
        assert!(user.is_some(), "a separate user_voice → a second engine");

        // Matches the assistant's voice → no second one needed.
        let mut same = base.clone();
        same.openai.user_voice = Some("onyx".into());
        let (_a, user) = engines_from_config(&same, Some("sk-x".into())).unwrap();
        assert!(user.is_none(), "matching voice — a single engine");
    }

    #[test]
    fn empty_clip_detected() {
        assert!(AudioClip::Encoded(Vec::new()).is_empty());
        assert!(
            AudioClip::Pcm {
                sample_rate: 24000,
                channels: 1,
                bytes: vec![0],
            }
            .is_empty()
        );
        assert!(!AudioClip::Encoded(vec![1, 2, 3]).is_empty());
    }
}

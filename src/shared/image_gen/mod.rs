//! Image generation: the contract and the provider clients, for the
//! `image_generate` tool (spec §9.15, docs/research/image-generation.md §6.2).
//!
//! Like speech (ADR 0009) and video (`shared::video`), this slot is
//! **independent of the chat engine**: the providers that draw are not the ones
//! that chat — a Gemini image model takes no function calls at all (§3.2) — and a
//! person on a local model, who cannot run a diffusion model beside it, is the
//! one who needs it most (§1). The tool calls the provider out of band and hands
//! the picture back into the conversation, whatever the chat engine is.
//!
//! Two wires (§6.2): the OpenAI shape — OpenAI's own `/images/generations` and
//! `/images/edits` ([`openai`]), xAI's dialect of them ([`xai`]) and the gateway's
//! `/images` ([`gateway`]) — and Gemini's `generateContent` ([`gemini`]). An edit is always a **fresh request**
//! with the pictures as inputs, never a replay of an image model's conversation,
//! so no thought signature and no stored response id is ever kept.
//!
//! A refusal is an **answer**, not an error ([`Drawing::Refused`]): the tool says
//! it in the directive form the probe measured and stops drawing for the turn
//! (F10); an error is a connection, a key, a limit — said in the provider's words.
//!
//! The clients are **stateless** but for what a provider's list says about a
//! model (the gateway's catalogue entry): built from a config snapshot when the
//! registry is built, no manager slot.

pub mod gateway;
pub mod gemini;
#[cfg(test)]
mod live_tests;
pub mod openai;
pub mod xai;

use std::fmt;
use std::sync::Arc;

use anyhow::Result;
use base64::Engine as _;
use tokio_util::sync::CancellationToken;

use crate::shared::config::{ImageGenProvider, ImageGenSettings};

/// The shape of the picture: the five the tool offers (§6.3). Each client maps
/// it to what its provider takes — a pixel size for OpenAI, an aspect ratio for
/// Gemini, xAI and the gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Aspect {
    #[default]
    Square,
    /// 3:2.
    Landscape,
    /// 2:3.
    Portrait,
    /// 16:9.
    Wide,
    /// 9:16.
    Tall,
}

impl Aspect {
    pub const ALL: [Aspect; 5] = [
        Aspect::Square,
        Aspect::Landscape,
        Aspect::Portrait,
        Aspect::Wide,
        Aspect::Tall,
    ];

    /// The ratio as the tool's schema, Gemini and the gateway spell it.
    pub fn ratio(self) -> &'static str {
        match self {
            Aspect::Square => "1:1",
            Aspect::Landscape => "3:2",
            Aspect::Portrait => "2:3",
            Aspect::Wide => "16:9",
            Aspect::Tall => "9:16",
        }
    }

    /// The ratio the model wrote, or `None` for one the tool does not offer.
    pub fn parse(said: &str) -> Option<Self> {
        let said = said.trim();
        Self::ALL.into_iter().find(|a| a.ratio() == said)
    }

    /// OpenAI's pixel size: the three standard ones, and 1536×864 for the wide
    /// pair — divisible by 16 and within the 1:3–3:1 the 2 and 2.5 models take,
    /// measured exact (§11.1).
    pub fn openai_size(self) -> &'static str {
        match self {
            Aspect::Square => "1024x1024",
            Aspect::Landscape => "1536x1024",
            Aspect::Portrait => "1024x1536",
            Aspect::Wide => "1536x864",
            Aspect::Tall => "864x1536",
        }
    }
}

/// A picture to change or to draw from: the bytes and the type they are.
#[derive(Clone, PartialEq, Eq)]
pub struct InputImage {
    pub mime: String,
    pub bytes: Vec<u8>,
}

impl InputImage {
    fn base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(&self.bytes)
    }

    /// The picture as a `data:` URL — what OpenAI's edits and the gateway's
    /// `input_references` take.
    fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.mime, self.base64())
    }
}

impl fmt::Debug for InputImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputImage")
            .field("mime", &self.mime)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// One request: the whole picture in words, its shape, and the pictures to
/// change (empty — a new picture).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRequest {
    pub prompt: String,
    pub aspect: Aspect,
    pub inputs: Vec<InputImage>,
}

/// What the provider said the call cost. OpenAI and Gemini count tokens; the
/// gateway and xAI state the price. The app keeps no price list of its own (§6.6).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Spent {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// US dollars, as the provider stated them.
    pub cost_usd: Option<f64>,
}

/// A picture the provider drew.
#[derive(Clone, PartialEq)]
pub struct Picture {
    /// The picture as the provider sent it — the original the chat keeps.
    pub bytes: Vec<u8>,
    /// What the bytes are, read off their first bytes.
    pub mime: &'static str,
    /// The prompt as the provider rewrote it, when it says so.
    pub revised_prompt: Option<String>,
    pub spent: Spent,
}

impl fmt::Debug for Picture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Picture")
            .field("bytes", &self.bytes.len())
            .field("mime", &self.mime)
            .field("revised_prompt", &self.revised_prompt)
            .field("spent", &self.spent)
            .finish()
    }
}

/// What a provider answered to a request it took.
#[derive(Debug, Clone, PartialEq)]
pub enum Drawing {
    Picture(Picture),
    /// The provider would not draw this — its moderation, in its own words.
    Refused {
        reason: String,
        spent: Spent,
    },
}

/// A provider that draws.
#[async_trait::async_trait]
pub trait ImageGenerator: Send + Sync {
    /// Draws, or says why not. An `Err` is everything that is not the
    /// provider's answer about the picture: the network, a key, a limit, a
    /// model that is not there. Cancellation must be honoured — a picture takes
    /// ten to thirty seconds (§11.1), and `Esc` has to work through it.
    async fn draw(&self, req: ImageRequest, cancel: &CancellationToken) -> Result<Drawing>;
}

/// The client of the provider the config names.
pub fn client(cfg: ImageGenConfig) -> Arc<dyn ImageGenerator> {
    match cfg.provider {
        ImageGenProvider::OpenAi => Arc::new(openai::OpenAiImages::new(cfg)),
        ImageGenProvider::Gemini => Arc::new(gemini::GeminiImages::new(cfg)),
        ImageGenProvider::OpenRouter => Arc::new(gateway::GatewayImages::new(cfg)),
        ImageGenProvider::Grok => Arc::new(xai::XaiImages::new(cfg)),
    }
}

/// A usable configuration for one provider: the settings' section and the key
/// that was found for it.
///
/// `Debug` is **hand-written to redact the key**: this type is reachable from
/// `ToolConfig`, which derives `Debug`.
#[derive(Clone)]
pub struct ImageGenConfig {
    pub provider: ImageGenProvider,
    /// Name the application to the gateway (`openrouter.attribution`). Read by
    /// the gateway's client alone.
    pub attribution: bool,
    pub model: String,
    /// OpenAI's and xAI's `quality`, Gemini's `imageSize`, the gateway's
    /// `resolution`; `None` — the provider's own default.
    pub quality: Option<String>,
    pub base_url: String,
    pub api_key: String,
}

impl fmt::Debug for ImageGenConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageGenConfig")
            .field("provider", &self.provider)
            .field("attribution", &self.attribution)
            .field("model", &self.model)
            .field("quality", &self.quality)
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

/// What a configuration lacks — said to the model, which tells the person where
/// to put it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Missing {
    /// No model named in the provider's section (the gateway has no default).
    Model,
    /// No stored key and no variable that holds one.
    Key,
}

/// The slot as the tool receives it: the switches the tool applies itself, and
/// the provider's configuration or what it lacks.
#[derive(Debug, Clone)]
pub struct ImageGenSlot {
    pub provider: ImageGenProvider,
    /// The model is shown the picture it drew (fork F6).
    pub show_to_model: bool,
    /// Pictures one turn may draw; `0` — no ceiling (F7).
    pub max_per_turn: u32,
    pub config: std::result::Result<ImageGenConfig, Missing>,
}

/// Resolves the settings and the stored key of **the provider they name** (ADR
/// 0008, [`ImageGenSettings::secret_key`]). `attribution` is the gateway's
/// provider-wide switch. Not asked about in advance, as the speech and video
/// slots are not: a refused key is the first picture's answer, in the
/// provider's words (§6.1).
pub fn resolve(
    settings: &ImageGenSettings,
    stored_key: Option<String>,
    attribution: bool,
) -> ImageGenSlot {
    let section = settings.section();
    let config = non_empty(section.model_name.clone())
        .ok_or(Missing::Model)
        .and_then(|model| {
            let key = stored_key
                .filter(|k| !k.trim().is_empty())
                .or_else(|| env_key(section.api_key_env.as_deref()))
                .ok_or(Missing::Key)?;
            let base_url = non_empty(section.url.clone()).unwrap_or_else(|| {
                settings
                    .provider
                    .cloud_provider()
                    .chat_base_url()
                    .to_string()
            });
            Ok(ImageGenConfig {
                provider: settings.provider,
                attribution: attribution && settings.provider == ImageGenProvider::OpenRouter,
                model,
                quality: non_empty(section.quality.clone()),
                base_url: base_url.trim_end_matches('/').to_string(),
                api_key: key,
            })
        });
    ImageGenSlot {
        provider: settings.provider,
        show_to_model: settings.show_to_model,
        max_per_turn: settings.max_per_turn,
        config,
    }
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

/// The bytes of a `b64_json` the OpenAI shape answers with, and what they are.
/// A picture whose first bytes are no image type is not a picture.
fn picture_bytes(subject: &str, b64: &str) -> Result<(Vec<u8>, &'static str)> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| anyhow::anyhow!("{subject}: the picture is not base64: {e}"))?;
    let mime = crate::entities::chat_file::sniff_image(&bytes)
        .ok_or_else(|| anyhow::anyhow!("{subject}: what came back is not a picture"))?;
    Ok((bytes, mime))
}

/// A provider's body shortened for an error — never the whole of a body that
/// may hold megabytes of base64.
fn excerpt(body: &str) -> String {
    body.trim().chars().take(500).collect()
}

/// Sends a request and reads the whole answer, both under `cancel`. An answer
/// whose body cannot be read is an error when it was a success, and its status
/// alone otherwise.
async fn send(
    subject: &str,
    rb: reqwest::RequestBuilder,
    cancel: &CancellationToken,
) -> Result<(reqwest::StatusCode, String)> {
    use anyhow::Context as _;
    let resp = tokio::select! {
        biased;
        _ = cancel.cancelled() => anyhow::bail!("{subject}: cancelled"),
        r = rb.send() => r.with_context(|| format!("{subject}: the request did not reach the provider"))?,
    };
    let status = resp.status();
    let text = tokio::select! {
        biased;
        _ = cancel.cancelled() => anyhow::bail!("{subject}: cancelled"),
        t = resp.text() => t,
    };
    match text {
        Ok(text) => Ok((status, text)),
        Err(err) if status.is_success() => {
            Err(err).with_context(|| format!("{subject}: reading the answer"))
        }
        Err(_) => Ok((status, String::new())),
    }
}

#[cfg(test)]
pub(crate) mod mock {
    use super::*;
    use std::sync::Mutex;

    /// Records each request and answers from a script — lets the tool be tested
    /// without a network or a key.
    pub struct MockImages {
        pub taken: Mutex<Vec<ImageRequest>>,
        answers: Mutex<Vec<std::result::Result<Drawing, String>>>,
    }

    impl MockImages {
        /// Answers each call with the next of `answers`, the last one again once
        /// they run out.
        pub fn answering(answers: Vec<std::result::Result<Drawing, String>>) -> Self {
            Self {
                taken: Mutex::new(Vec::new()),
                answers: Mutex::new(answers),
            }
        }

        /// A provider that draws `bytes` every time.
        pub fn drawing(bytes: &[u8]) -> Self {
            Self::answering(vec![Ok(Drawing::Picture(Picture {
                bytes: bytes.to_vec(),
                mime: crate::entities::chat_file::sniff_image(bytes).unwrap_or("image/png"),
                revised_prompt: None,
                spent: Spent::default(),
            }))])
        }

        pub fn refusing(reason: &str) -> Self {
            Self::answering(vec![Ok(Drawing::Refused {
                reason: reason.to_string(),
                spent: Spent::default(),
            })])
        }

        pub fn calls(&self) -> Vec<ImageRequest> {
            self.taken.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl ImageGenerator for MockImages {
        async fn draw(&self, req: ImageRequest, _cancel: &CancellationToken) -> Result<Drawing> {
            self.taken.lock().unwrap().push(req);
            let mut answers = self.answers.lock().unwrap();
            let answer = if answers.len() > 1 {
                answers.remove(0)
            } else {
                answers[0].clone()
            };
            answer.map_err(|e| anyhow::anyhow!("{e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::config::{CloudProvider, ImageGenSection};

    #[test]
    fn every_aspect_round_trips_and_nothing_else_parses() {
        for aspect in Aspect::ALL {
            assert_eq!(Aspect::parse(aspect.ratio()), Some(aspect));
        }
        assert_eq!(Aspect::parse(" 16:9 "), Some(Aspect::Wide));
        assert_eq!(Aspect::parse("4:3"), None);
        assert_eq!(Aspect::parse(""), None);
    }

    /// OpenAI's 2.x models take any size divisible by 16 between 1:3 and 3:1
    /// (§3.1); every size here is one, and its sides agree with its ratio's.
    #[test]
    fn every_openai_size_is_one_openai_takes_and_has_its_aspect() {
        for aspect in Aspect::ALL {
            let (w, h) = aspect.openai_size().split_once('x').unwrap();
            let (w, h): (u32, u32) = (w.parse().unwrap(), h.parse().unwrap());
            assert_eq!((w % 16, h % 16), (0, 0), "{aspect:?}");
            assert!(w * 3 >= h && h * 3 >= w, "{aspect:?}");
            let (rw, rh) = aspect.ratio().split_once(':').unwrap();
            let (rw, rh): (f64, f64) = (rw.parse().unwrap(), rh.parse().unwrap());
            let off = (f64::from(w) / f64::from(h)) / (rw / rh);
            assert!((0.98..1.02).contains(&off), "{aspect:?}: {off}");
        }
    }

    /// The switch off is the default (F7), OpenAI the provider (F3′), four
    /// pictures a turn, and the gateway names no model.
    #[test]
    fn the_defaults_are_the_owners_decisions() {
        let s = ImageGenSettings::default();
        assert!(!s.enabled);
        assert_eq!(s.provider, ImageGenProvider::OpenAi);
        assert!(s.show_to_model);
        assert_eq!(s.max_per_turn, 4);
        assert_eq!(s.openai.model_name.as_deref(), Some("gpt-image-2.5-flare"));
        assert_eq!(s.openai.quality.as_deref(), Some("medium"));
        assert_eq!(
            s.gemini.model_name.as_deref(),
            Some("gemini-nano-banana-2.1")
        );
        assert_eq!(s.openrouter.model_name, None);
    }

    #[test]
    fn a_slot_says_what_it_lacks() {
        let mut s = ImageGenSettings::default();
        assert_eq!(resolve(&s, None, true).config.unwrap_err(), Missing::Key);
        assert_eq!(
            resolve(&s, Some("  ".into()), true).config.unwrap_err(),
            Missing::Key,
            "a blank stored key is no key"
        );
        let cfg = resolve(&s, Some("k".into()), true).config.unwrap();
        assert_eq!(cfg.model, "gpt-image-2.5-flare");
        assert_eq!(cfg.quality.as_deref(), Some("medium"));
        assert_eq!(cfg.base_url, CloudProvider::OpenAi.chat_base_url());
        assert!(!cfg.attribution, "the switch is the gateway's");

        s.provider = ImageGenProvider::OpenRouter;
        assert_eq!(
            resolve(&s, Some("k".into()), true).config.unwrap_err(),
            Missing::Model,
            "the gateway has no default model"
        );
        s.openrouter.model_name = Some(" black-forest-labs/flux-3-image ".into());
        let cfg = resolve(&s, Some("k".into()), true).config.unwrap();
        assert_eq!(cfg.model, "black-forest-labs/flux-3-image");
        assert_eq!(cfg.base_url, CloudProvider::OpenRouter.chat_base_url());
        assert!(cfg.attribution);
        assert!(
            !resolve(&s, Some("k".into()), false)
                .config
                .unwrap()
                .attribution
        );
    }

    /// Each provider is read from its own section — the model, the quality,
    /// the address, the variable — and a switch loses nothing.
    #[test]
    fn each_provider_is_resolved_from_its_own_section() {
        let mut s = ImageGenSettings {
            provider: ImageGenProvider::Gemini,
            openai: ImageGenSection {
                url: Some("https://openai.example/v1".into()),
                ..ImageGenSettings::default().openai
            },
            ..Default::default()
        };
        s.gemini.url = Some("https://gemini.example/v1beta/".into());
        s.gemini.quality = Some("  ".into());
        let cfg = resolve(&s, Some("k".into()), true).config.unwrap();
        assert_eq!(cfg.model, "gemini-nano-banana-2.1");
        assert_eq!(cfg.base_url, "https://gemini.example/v1beta");
        assert_eq!(cfg.quality, None, "blank → the provider's own default");
        // The key's variable is the section's own: `PATH` is read, never sent.
        s.gemini.api_key_env = Some("PATH".into());
        assert!(resolve(&s, None, true).config.is_ok());
        s.provider = ImageGenProvider::OpenAi;
        assert_eq!(
            resolve(&s, None, true).config.unwrap_err(),
            Missing::Key,
            "Gemini's variable is not OpenAI's"
        );
        assert_eq!(
            resolve(&s, Some("k".into()), true).config.unwrap().base_url,
            "https://openai.example/v1"
        );
    }

    /// xAI is read from its own section, at its own address, with the engine's
    /// stored key for the same provider.
    #[test]
    fn xai_draws_from_its_own_section_and_address() {
        let s = ImageGenSettings {
            provider: ImageGenProvider::Grok,
            ..Default::default()
        };
        let cfg = resolve(&s, Some("k".into()), true).config.unwrap();
        assert_eq!(cfg.model, "grok-imagine-image-2.0");
        assert_eq!(cfg.quality.as_deref(), Some("auto"));
        assert_eq!(cfg.base_url, "https://api.x.ai/v1");
        assert!(!cfg.attribution, "the switch is the gateway's");
        assert_eq!(
            s.secret_key(),
            crate::shared::secrets::SecretKey::Provider(CloudProvider::Grok)
        );
    }

    #[test]
    fn debug_redacts_the_key() {
        let cfg = resolve(
            &ImageGenSettings::default(),
            Some("secret-key".into()),
            true,
        )
        .config
        .unwrap();
        let dump = format!("{cfg:?}");
        assert!(!dump.contains("secret-key"), "key leaked: {dump}");
        assert!(dump.contains("redacted"));
    }

    #[test]
    fn a_picture_is_read_by_its_bytes() {
        let png = crate::shared::image_gen::tests::tiny_png();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let (bytes, mime) = picture_bytes("t", &b64).unwrap();
        assert_eq!((bytes, mime), (png, "image/png"));
        let text = base64::engine::general_purpose::STANDARD.encode(b"hello");
        assert!(picture_bytes("t", &text).is_err());
        assert!(picture_bytes("t", "%%%").is_err());
    }

    /// A real 2×1 PNG — the fixture every client's tests decode.
    pub(crate) fn tiny_png() -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbImage::from_pixel(2, 1, image::Rgb([200, 30, 30]))
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }
}

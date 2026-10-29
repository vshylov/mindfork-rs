//! The OpenRouter gateway's Image API, `POST /images`
//! (docs/research/image-generation.md §3.4, §6.2): one key, some sixty models
//! of a dozen vendors, billing that is all-or-nothing — a failed generation is
//! a `502` and is not charged — and an answer that states its own price.
//!
//! **What a model takes is its catalogue entry's to say** (`GET /images/models`,
//! `supported_parameters`): FLUX.2, Recraft and MAI list no `resolution`, the
//! aspects differ from model to model, and `input_references` has a maximum per
//! model — zero for a model that cannot edit. The client asks once and sends
//! only what the entry lists; a model the list does not name, or a list that
//! cannot be had, gets the prompt and the aspect, and the gateway's answer.
//!
//! A refusal has no code of its own: the probe's was a `400` whose message is
//! the vendor's — *"Black Forest Labs blocked this request: it was flagged for
//! graphic violence."* — with `metadata.provider_name` (§11.1). An upstream
//! provider's error that speaks of blocking, flagging, moderation or policy is
//! read as a refusal; anything else is an error in the gateway's words. A
//! misreading costs little here: the gateway charges for neither.

use std::sync::{Mutex, PoisonError};

use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use super::{
    Drawing, ImageGenConfig, ImageGenerator, ImageRequest, Picture, Spent, excerpt, picture_bytes,
    send,
};
use crate::shared::api::openai::attributed;

/// What the errors of this client are said about.
const SUBJECT: &str = "OpenRouter images";

/// Words an upstream refusal is told by, lowercase.
const REFUSAL_WORDS: [&str; 8] = [
    "blocked",
    "flagged",
    "moderation",
    "safety",
    "policy",
    "prohibited",
    "not allowed",
    "inappropriate",
];

/// What the gateway's list says one model takes (`supported_parameters`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Entry {
    params: Map<String, Value>,
}

impl Entry {
    /// Whether the model takes `param` at `value`: listed, and the value among
    /// its enum's when it has one.
    fn takes(&self, param: &str, value: &str) -> bool {
        let Some(spec) = self.params.get(param) else {
            return false;
        };
        match spec.get("values").and_then(Value::as_array) {
            Some(values) => values.iter().any(|v| v.as_str() == Some(value)),
            None => true,
        }
    }

    /// How many pictures the model takes to change: `0` when the entry lists
    /// none.
    fn max_references(&self) -> u64 {
        self.params
            .get("input_references")
            .and_then(|r| r.get("max"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    }
}

/// Why a request was not sent: the model cannot take what it was asked to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NotSent {
    /// The model's entry lists no pictures to change.
    NoEdits,
    /// More pictures than the model's entry takes.
    TooMany(u64),
}

pub struct GatewayImages {
    http: reqwest::Client,
    cfg: ImageGenConfig,
    /// The model's catalogue entry, once the list has answered: `Some(None)` —
    /// the list does not name it. An outage sets nothing, and the next picture
    /// asks again.
    entry: Mutex<Option<Option<Entry>>>,
}

impl GatewayImages {
    pub fn new(cfg: ImageGenConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            cfg: ImageGenConfig {
                model: cfg.model.trim().to_string(),
                ..cfg
            },
            entry: Mutex::new(None),
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

    /// The model's entry in the gateway's list of picture models, asked once.
    async fn entry(&self, cancel: &CancellationToken) -> Option<Entry> {
        if let Some(known) = self
            .entry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            return known;
        }
        let url = format!("{}/images/models", self.cfg.base_url);
        let answer = match send(SUBJECT, self.signed(self.http.get(&url)), cancel).await {
            Ok((status, text)) if status.is_success() => text,
            Ok((status, _)) => {
                tracing::debug!(%url, %status, "the gateway's list of picture models is unavailable");
                return None;
            }
            Err(err) => {
                tracing::debug!(%url, error = %err, "no answer about the picture models");
                return None;
            }
        };
        let Ok(list) = serde_json::from_str::<Value>(&answer) else {
            return None;
        };
        let found = entry_in(&list, &self.cfg.model);
        *self.entry.lock().unwrap_or_else(PoisonError::into_inner) = Some(found.clone());
        found
    }

    /// The request body. Pure: what the entry lists decides what is sent.
    pub(crate) fn body(&self, req: &ImageRequest, entry: Option<&Entry>) -> Result<Value, NotSent> {
        let mut body = json!({ "model": self.cfg.model, "prompt": req.prompt });
        let ratio = req.aspect.ratio();
        // A model nobody described gets the aspect, which every family takes;
        // one the list describes, only an aspect it lists.
        if entry.is_none_or(|e| e.takes("aspect_ratio", ratio)) {
            body["aspect_ratio"] = json!(ratio);
        }
        if let (Some(entry), Some(resolution)) = (entry, &self.cfg.quality)
            && entry.takes("resolution", resolution)
        {
            body["resolution"] = json!(resolution);
        }
        if !req.inputs.is_empty() {
            if let Some(entry) = entry {
                let max = entry.max_references();
                if max == 0 {
                    return Err(NotSent::NoEdits);
                }
                if req.inputs.len() as u64 > max {
                    return Err(NotSent::TooMany(max));
                }
            }
            body["input_references"] = req
                .inputs
                .iter()
                .map(|i| json!({ "type": "image_url", "image_url": { "url": i.data_url() } }))
                .collect();
        }
        Ok(body)
    }
}

/// The entry for `model` in the gateway's list, or `None` when the list does not
/// name it.
pub(crate) fn entry_in(list: &Value, model: &str) -> Option<Entry> {
    list.get("data")?
        .as_array()?
        .iter()
        .find(|m| m.get("id").and_then(Value::as_str) == Some(model))
        .map(|m| Entry {
            params: m
                .get("supported_parameters")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
        })
}

fn spent_in(body: &Value) -> Spent {
    let usage = body.get("usage");
    let n = |key: &str| usage.and_then(|u| u.get(key)).and_then(Value::as_u64);
    Spent {
        input_tokens: n("prompt_tokens"),
        output_tokens: n("completion_tokens"),
        cost_usd: usage.and_then(|u| u.get("cost")).and_then(Value::as_f64),
    }
}

/// Whether an error the gateway passed on is a vendor's refusal: OpenAI's code
/// carried through, or an upstream message in the words a refusal is said in.
fn is_refusal(status: reqwest::StatusCode, error: &Value) -> bool {
    let raw_code = error
        .pointer("/metadata/raw")
        .and_then(Value::as_str)
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|raw| raw.pointer("/error/code")?.as_str().map(str::to_string));
    if raw_code.as_deref() == Some("moderation_blocked") {
        return true;
    }
    if !matches!(status.as_u16(), 400 | 403 | 422) {
        return false;
    }
    let said = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    REFUSAL_WORDS.iter().any(|w| said.contains(w))
}

/// The picture, or the refusal, out of what the gateway answered. Pure. The
/// body's `error` is read before the status, since the gateway's error
/// envelope may come with any status.
pub(crate) fn answer_from(status: reqwest::StatusCode, text: &str) -> anyhow::Result<Drawing> {
    let Ok(body) = serde_json::from_str::<Value>(text) else {
        anyhow::bail!("{SUBJECT}: status {status}: {}", excerpt(text));
    };
    if let Some(error) = body.get("error").filter(|e| !e.is_null()) {
        let said = error
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map_or_else(|| excerpt(text), str::to_string);
        if is_refusal(status, error) {
            return Ok(Drawing::Refused {
                reason: said,
                spent: spent_in(&body),
            });
        }
        let vendor = error
            .pointer("/metadata/provider_name")
            .and_then(Value::as_str);
        match vendor {
            Some(vendor) => anyhow::bail!("{SUBJECT}: status {status}: {said} ({vendor})"),
            None => anyhow::bail!("{SUBJECT}: status {status}: {said}"),
        }
    }
    anyhow::ensure!(
        status.is_success(),
        "{SUBJECT}: status {status}: {}",
        excerpt(text)
    );
    let Some(b64) = body.pointer("/data/0/b64_json").and_then(Value::as_str) else {
        anyhow::bail!("{SUBJECT}: the answer holds no picture");
    };
    let (bytes, mime) = picture_bytes(SUBJECT, b64)?;
    Ok(Drawing::Picture(Picture {
        bytes,
        mime,
        revised_prompt: None,
        spent: spent_in(&body),
    }))
}

#[async_trait::async_trait]
impl ImageGenerator for GatewayImages {
    async fn draw(&self, req: ImageRequest, cancel: &CancellationToken) -> anyhow::Result<Drawing> {
        let entry = self.entry(cancel).await;
        let body = match self.body(&req, entry.as_ref()) {
            Ok(body) => body,
            Err(NotSent::NoEdits) => anyhow::bail!(
                "{SUBJECT}: the model {} takes no pictures to change — it can only draw a new one",
                self.cfg.model
            ),
            Err(NotSent::TooMany(max)) => anyhow::bail!(
                "{SUBJECT}: the model {} takes at most {max} picture(s) to change",
                self.cfg.model
            ),
        };
        let url = format!("{}/images", self.cfg.base_url);
        let rb = self.signed(self.http.post(&url)).json(&body);
        let (status, text) = send(SUBJECT, rb, cancel).await?;
        let answer = answer_from(status, &text);
        match &answer {
            Ok(Drawing::Refused { reason, .. }) => {
                tracing::info!(%status, %reason, "the gateway refused to draw");
            }
            Err(err) => tracing::warn!(%status, error = %err, "the gateway did not draw"),
            Ok(Drawing::Picture(p)) => {
                tracing::info!(bytes = p.bytes.len(), spent = ?p.spent, "the gateway drew");
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

    fn client(model: &str, quality: Option<&str>) -> GatewayImages {
        GatewayImages::new(ImageGenConfig {
            provider: ImageGenProvider::OpenRouter,
            attribution: true,
            model: model.into(),
            quality: quality.map(str::to_string),
            base_url: "https://openrouter.ai/api/v1".into(),
            api_key: "k".into(),
        })
    }

    /// Two entries as `GET /images/models` lists them (2026-10-29): FLUX-3 takes
    /// a resolution and ten pictures; FLUX.2 Pro takes no resolution.
    fn catalogue() -> Value {
        json!({"data": [
            {"id": "black-forest-labs/flux-3-image", "supported_parameters": {
                "resolution": {"type": "enum", "values": ["768", "1K", "1.5K", "2K", "4K"]},
                "aspect_ratio": {"type": "enum", "values": ["21:9", "16:9", "3:2", "1:1", "2:3", "9:16"]},
                "n": {"type": "range", "min": 1, "max": 1},
                "input_references": {"type": "range", "min": 0, "max": 10},
            }},
            {"id": "black-forest-labs/flux.2-pro", "supported_parameters": {
                "aspect_ratio": {"type": "enum", "values": ["1:1", "4:3", "3:2", "2:3", "16:9"]},
                "input_references": {"type": "range", "min": 0, "max": 8},
            }},
            {"id": "acme/draw-only", "supported_parameters": {
                "aspect_ratio": {"type": "enum", "values": ["1:1"]},
            }},
        ]})
    }

    fn request(aspect: Aspect, inputs: usize) -> ImageRequest {
        let png = crate::shared::image_gen::tests::tiny_png();
        ImageRequest {
            prompt: "a fox".into(),
            aspect,
            inputs: (0..inputs)
                .map(|_| InputImage {
                    mime: "image/png".into(),
                    bytes: png.clone(),
                })
                .collect(),
        }
    }

    /// What the entry lists is sent; what it does not, is not — the
    /// resolution FLUX.2 has none of, the aspect a model does not take.
    #[test]
    fn only_what_the_entry_lists_is_sent() {
        let list = catalogue();
        let flux3 = entry_in(&list, "black-forest-labs/flux-3-image");
        let c = client("black-forest-labs/flux-3-image", Some("1K"));
        assert_eq!(
            c.body(&request(Aspect::Wide, 0), flux3.as_ref()).unwrap(),
            json!({"model": "black-forest-labs/flux-3-image", "prompt": "a fox",
                   "aspect_ratio": "16:9", "resolution": "1K"})
        );

        let flux2 = entry_in(&list, "black-forest-labs/flux.2-pro");
        let c = client("black-forest-labs/flux.2-pro", Some("1K"));
        assert_eq!(
            c.body(&request(Aspect::Tall, 0), flux2.as_ref()).unwrap(),
            json!({"model": "black-forest-labs/flux.2-pro", "prompt": "a fox"}),
            "no resolution listed, and 9:16 is not among its aspects"
        );
    }

    /// A model the list does not name — or no list at all — gets the prompt
    /// and the aspect, never a resolution it may refuse.
    #[test]
    fn an_undescribed_model_gets_the_aspect_alone() {
        let list = catalogue();
        assert_eq!(entry_in(&list, "acme/unknown"), None);
        let c = client("acme/unknown", Some("1K"));
        assert_eq!(
            c.body(&request(Aspect::Portrait, 1), None).unwrap()["aspect_ratio"],
            "2:3"
        );
        let body = c.body(&request(Aspect::Portrait, 1), None).unwrap();
        assert!(body.get("resolution").is_none());
        assert_eq!(body["input_references"].as_array().unwrap().len(), 1);
    }

    /// Pictures to change go as `input_references` — refused by name where the
    /// model takes none, or fewer.
    #[test]
    fn pictures_to_change_are_within_the_models_maximum() {
        let list = catalogue();
        let c = client("acme/draw-only", None);
        let draw_only = entry_in(&list, "acme/draw-only");
        assert_eq!(
            c.body(&request(Aspect::Square, 1), draw_only.as_ref()),
            Err(NotSent::NoEdits)
        );
        assert!(
            c.body(&request(Aspect::Square, 0), draw_only.as_ref())
                .is_ok()
        );

        let flux2 = entry_in(&list, "black-forest-labs/flux.2-pro");
        let c = client("black-forest-labs/flux.2-pro", None);
        assert_eq!(
            c.body(&request(Aspect::Square, 9), flux2.as_ref()),
            Err(NotSent::TooMany(8))
        );
        let body = c.body(&request(Aspect::Square, 2), flux2.as_ref()).unwrap();
        let refs = body["input_references"].as_array().unwrap();
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0]["type"], "image_url");
        assert!(
            refs[0]["image_url"]["url"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
    }

    /// The answer of §11.1: the picture with its type, and the price stated.
    #[test]
    fn a_picture_is_read_with_its_price() {
        let png = crate::shared::image_gen::tests::tiny_png();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let text = json!({"created": 0, "data": [{"b64_json": b64, "media_type": "image/png"}],
            "usage": {"prompt_tokens": 19, "completion_tokens": 4175, "total_tokens": 4194, "cost": 0.048}})
        .to_string();
        let Drawing::Picture(p) = answer_from(reqwest::StatusCode::OK, &text).unwrap() else {
            panic!("a picture");
        };
        assert_eq!(p.bytes, png);
        assert_eq!(p.spent.cost_usd, Some(0.048));
        assert_eq!(p.spent.output_tokens, Some(4175));
    }

    /// The probe's refusal, word for word, is an answer; a parameter the gateway
    /// will not take, or a failed generation, is an error in its words.
    #[test]
    fn the_vendors_block_is_a_refusal_and_a_fault_is_not() {
        let refused = json!({"error": {
            "message": "Black Forest Labs blocked this request: it was flagged for graphic violence.",
            "code": 400, "metadata": {"provider_name": "Black Forest Labs"}}});
        let Drawing::Refused { reason, .. } =
            answer_from(reqwest::StatusCode::BAD_REQUEST, &refused.to_string()).unwrap()
        else {
            panic!("a refusal");
        };
        assert!(reason.starts_with("Black Forest Labs blocked"));

        let invalid = json!({"error": {"message": "Invalid aspect_ratio: 7:3", "code": 400}});
        let err = answer_from(reqwest::StatusCode::BAD_REQUEST, &invalid.to_string())
            .unwrap_err()
            .to_string();
        assert!(err.contains("Invalid aspect_ratio"), "{err}");

        let failed = json!({"error": {"message": "Provider returned error", "code": 502,
            "metadata": {"provider_name": "Black Forest Labs"}}});
        let err = answer_from(reqwest::StatusCode::BAD_GATEWAY, &failed.to_string())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("502") && err.contains("(Black Forest Labs)"),
            "{err}"
        );
    }

    /// OpenAI's own refusal, passed on through the gateway, is OpenAI's.
    #[test]
    fn openais_code_passed_on_is_a_refusal() {
        let raw =
            json!({"error": {"code": "moderation_blocked", "message": "rejected"}}).to_string();
        let body = json!({"error": {"message": "Provider returned error", "code": 400,
            "metadata": {"provider_name": "OpenAI", "raw": raw}}});
        assert!(matches!(
            answer_from(reqwest::StatusCode::BAD_REQUEST, &body.to_string()).unwrap(),
            Drawing::Refused { .. }
        ));
    }
}

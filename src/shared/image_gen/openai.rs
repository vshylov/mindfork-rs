//! OpenAI's Images API: `POST /images/generations` for a new picture,
//! `POST /images/edits` (JSON, the pictures as `data:` URLs) for a changed one
//! (docs/history/image-generation.md §3.1).
//!
//! The answer is **base64 only** for the GPT image models — no URL to fetch
//! before it expires. A refusal is a `400` with a stable code,
//! `moderation_blocked`, in about a second and free (§11.1); every other error is
//! said in OpenAI's words, the unverified organisation's `403` among them (§3.1).

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{
    Drawing, ImageGenConfig, ImageGenerator, ImageRequest, Picture, Spent, excerpt, picture_bytes,
    send,
};

/// What the errors of this client are said about.
const SUBJECT: &str = "OpenAI images";

/// The codes OpenAI refuses a picture by: the GPT image models' own, and the
/// one the DALL·E generation used.
const REFUSAL_CODES: [&str; 2] = ["moderation_blocked", "content_policy_violation"];

pub struct OpenAiImages {
    http: reqwest::Client,
    cfg: ImageGenConfig,
}

impl OpenAiImages {
    pub fn new(cfg: ImageGenConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            cfg,
        }
    }

    /// The route: an edit when there are pictures to change.
    pub(crate) fn url(&self, req: &ImageRequest) -> String {
        let route = if req.inputs.is_empty() {
            "generations"
        } else {
            "edits"
        };
        format!("{}/images/{route}", self.cfg.base_url)
    }

    /// The request body. Pure — the shape is what the tests assert on. One
    /// picture (`n`), the size the aspect maps to, the section's quality; the
    /// format is the provider's default, PNG.
    pub(crate) fn body(&self, req: &ImageRequest) -> Value {
        let mut body = json!({
            "model": self.cfg.model,
            "prompt": req.prompt,
            "size": req.aspect.openai_size(),
            "n": 1,
        });
        if let Some(quality) = &self.cfg.quality {
            body["quality"] = json!(quality);
        }
        if !req.inputs.is_empty() {
            body["images"] = req
                .inputs
                .iter()
                .map(|i| json!({ "image_url": i.data_url() }))
                .collect();
        }
        body
    }
}

/// The refusal in an error envelope, or `None` for any other error: OpenAI's
/// message, and the stage and categories it names (§3.1).
fn refusal_in(error: &Value) -> Option<String> {
    let code = error.get("code").and_then(Value::as_str)?;
    if !REFUSAL_CODES.contains(&code) {
        return None;
    }
    let said = error
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or(code);
    let details = error.get("moderation_details");
    let categories: Vec<&str> = details
        .and_then(|d| d.get("categories"))
        .and_then(Value::as_array)
        .map(|c| c.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    Some(if categories.is_empty() {
        said.to_string()
    } else {
        format!("{said} (categories: {})", categories.join(", "))
    })
}

fn spent_in(body: &Value) -> Spent {
    let usage = body.get("usage");
    let n = |key: &str| usage.and_then(|u| u.get(key)).and_then(Value::as_u64);
    Spent {
        input_tokens: n("input_tokens"),
        output_tokens: n("output_tokens"),
        cost_usd: None,
    }
}

/// The picture, or the refusal, out of what OpenAI answered. Pure — tested on
/// the bodies the probe recorded.
pub(crate) fn answer_from(status: reqwest::StatusCode, text: &str) -> anyhow::Result<Drawing> {
    let Ok(body) = serde_json::from_str::<Value>(text) else {
        anyhow::bail!("{SUBJECT}: status {status}: {}", excerpt(text));
    };
    if let Some(error) = body.get("error").filter(|e| !e.is_null()) {
        if let Some(reason) = refusal_in(error) {
            return Ok(Drawing::Refused {
                reason,
                spent: spent_in(&body),
            });
        }
        let said = error
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map_or_else(|| excerpt(text), str::to_string);
        anyhow::bail!("{SUBJECT}: status {status}: {said}");
    }
    anyhow::ensure!(
        status.is_success(),
        "{SUBJECT}: status {status}: {}",
        excerpt(text)
    );
    let first = body.pointer("/data/0");
    let Some(b64) = first
        .and_then(|d| d.get("b64_json"))
        .and_then(Value::as_str)
    else {
        anyhow::bail!("{SUBJECT}: the answer holds no picture");
    };
    let (bytes, mime) = picture_bytes(SUBJECT, b64)?;
    let revised_prompt = first
        .and_then(|d| d.get("revised_prompt"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string);
    Ok(Drawing::Picture(Picture {
        bytes,
        mime,
        revised_prompt,
        spent: spent_in(&body),
    }))
}

#[async_trait::async_trait]
impl ImageGenerator for OpenAiImages {
    async fn draw(&self, req: ImageRequest, cancel: &CancellationToken) -> anyhow::Result<Drawing> {
        let url = self.url(&req);
        let rb = self
            .http
            .post(&url)
            .bearer_auth(&self.cfg.api_key)
            .json(&self.body(&req));
        let (status, text) = send(SUBJECT, rb, cancel).await?;
        let answer = answer_from(status, &text);
        match &answer {
            Ok(Drawing::Refused { reason, .. }) => {
                tracing::info!(%status, %reason, "OpenAI refused to draw");
            }
            Err(err) => tracing::warn!(%status, error = %err, "OpenAI did not draw"),
            Ok(Drawing::Picture(p)) => {
                tracing::info!(bytes = p.bytes.len(), spent = ?p.spent, "OpenAI drew");
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

    fn client(quality: Option<&str>) -> OpenAiImages {
        OpenAiImages::new(ImageGenConfig {
            provider: ImageGenProvider::OpenAi,
            attribution: false,
            model: "gpt-image-2.5-flare".into(),
            quality: quality.map(str::to_string),
            base_url: "https://api.openai.com/v1".into(),
            api_key: "k".into(),
        })
    }

    fn request(inputs: Vec<InputImage>) -> ImageRequest {
        ImageRequest {
            prompt: "a red fox in the snow".into(),
            aspect: Aspect::Wide,
            inputs,
        }
    }

    /// A new picture goes to `generations` with the mapped size and the
    /// section's quality; no format, no response format — base64 is all the
    /// GPT image models answer with.
    #[test]
    fn a_new_picture_is_a_generation_of_the_mapped_size() {
        let c = client(Some("medium"));
        let req = request(Vec::new());
        assert_eq!(c.url(&req), "https://api.openai.com/v1/images/generations");
        assert_eq!(
            c.body(&req),
            json!({
                "model": "gpt-image-2.5-flare",
                "prompt": "a red fox in the snow",
                "size": "1536x864",
                "n": 1,
                "quality": "medium",
            })
        );
        assert!(
            client(None).body(&req).get("quality").is_none(),
            "no quality → OpenAI's own default"
        );
    }

    /// A picture to change makes it an edit, the pictures as `data:` URLs —
    /// the JSON form, which takes up to sixteen (§3.1).
    #[test]
    fn pictures_to_change_make_it_an_edit() {
        let c = client(Some("medium"));
        let png = crate::shared::image_gen::tests::tiny_png();
        let req = request(vec![InputImage {
            mime: "image/png".into(),
            bytes: png.clone(),
        }]);
        assert_eq!(c.url(&req), "https://api.openai.com/v1/images/edits");
        let body = c.body(&req);
        let url = body["images"][0]["image_url"].as_str().unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        assert_eq!(url, format!("data:image/png;base64,{b64}"));
        assert_eq!(body["size"], "1536x864");
    }

    /// The answer of §11.1: the picture, and the tokens OpenAI counted.
    #[test]
    fn a_picture_is_read_with_its_usage() {
        let png = crate::shared::image_gen::tests::tiny_png();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let text = json!({
            "created": 1_793_000_000u64,
            "data": [{ "b64_json": b64 }],
            "usage": { "input_tokens": 24, "output_tokens": 439, "total_tokens": 463 },
        })
        .to_string();
        let Drawing::Picture(p) = answer_from(reqwest::StatusCode::OK, &text).unwrap() else {
            panic!("a picture");
        };
        assert_eq!(p.bytes, png);
        assert_eq!(p.mime, "image/png");
        assert_eq!(p.revised_prompt, None);
        assert_eq!(p.spent.input_tokens, Some(24));
        assert_eq!(p.spent.output_tokens, Some(439));
        assert_eq!(p.spent.cost_usd, None);
    }

    /// The refusal the probe recorded: a `400` with `moderation_blocked` is an
    /// answer, in OpenAI's words, with the categories it names.
    #[test]
    fn a_moderation_block_is_a_refusal() {
        let text = json!({"error": {
            "message": "Your request was rejected by the safety system.",
            "type": "image_generation_user_error",
            "param": null,
            "code": "moderation_blocked",
            "moderation_details": { "moderation_stage": "input", "categories": ["violence"] },
        }})
        .to_string();
        let Drawing::Refused { reason, .. } =
            answer_from(reqwest::StatusCode::BAD_REQUEST, &text).unwrap()
        else {
            panic!("a refusal");
        };
        assert_eq!(
            reason,
            "Your request was rejected by the safety system. (categories: violence)"
        );
    }

    /// Everything else is an error in OpenAI's words — the unverified
    /// organisation above all, which a key that chats may meet (§3.1).
    #[test]
    fn any_other_error_is_said_in_openais_words() {
        let text = json!({"error": {
            "message": "Your organization must be verified to use the model `gpt-image-2.5-flare`.",
            "type": "invalid_request_error",
            "code": null,
        }})
        .to_string();
        let err = answer_from(reqwest::StatusCode::FORBIDDEN, &text).unwrap_err();
        let said = err.to_string();
        assert!(said.contains("403"), "{said}");
        assert!(said.contains("must be verified"), "{said}");

        let err = answer_from(reqwest::StatusCode::BAD_GATEWAY, "<html>bad</html>").unwrap_err();
        assert!(err.to_string().contains("<html>bad</html>"));
        let err = answer_from(reqwest::StatusCode::OK, r#"{"data": []}"#).unwrap_err();
        assert!(err.to_string().contains("no picture"));
    }

    /// A rewritten prompt is kept when the answer carries one.
    #[test]
    fn a_rewritten_prompt_is_kept() {
        let png = crate::shared::image_gen::tests::tiny_png();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let text = json!({"data": [{ "b64_json": b64, "revised_prompt": " a fox, at dusk " }]})
            .to_string();
        let Drawing::Picture(p) = answer_from(reqwest::StatusCode::OK, &text).unwrap() else {
            panic!("a picture");
        };
        assert_eq!(p.revised_prompt.as_deref(), Some("a fox, at dusk"));
    }
}

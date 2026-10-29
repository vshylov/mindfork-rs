//! xAI's Images API: `POST /v1/images/generations` for a new picture,
//! `POST /v1/images/edits` (JSON only) for a changed one
//! (docs/research/image-generation.md §3.3, §6.2).
//!
//! The OpenAI wire in a dialect of its own:
//!
//! - **No pixel size.** The shape goes as `aspect_ratio`, on both routes; the
//!   resolution is xAI's default, `1k`.
//! - **`quality`** — `low`, `medium` or `auto` — is what the price is set by,
//!   and `grok-imagine-image-2.0` alone takes it. `auto` is xAI's own default, so
//!   it is never sent: a model without the parameter draws as asked.
//! - **`b64_json` asked for**, so no temporary link has to be fetched before it
//!   expires. The picture is a JPEG.
//! - **The pictures to change** go as `image` — one — or `images`, several, and a
//!   prompt names them `<IMAGE_0>`, `<IMAGE_1>`; the client says which is which.
//! - **The answer states its price**: `usage.cost_in_usd_ticks`, ten billion to
//!   the dollar.
//! - **A refusal comes after the drawing, and is charged** — measured: a `400`
//!   after 15.5 s, `{"code": "imagine:content-moderated", "error": "Generated
//!   image rejected by content moderation.", "usage": {"cost_in_usd_ticks":
//!   400000000}}` (§11.1). Its price goes into the refusal, so the model reads
//!   what the refused picture cost.

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{
    Drawing, ImageGenConfig, ImageGenerator, ImageRequest, Picture, Spent, excerpt, picture_bytes,
    send,
};

/// What the errors of this client are said about.
const SUBJECT: &str = "xAI images";

/// The code xAI refuses a picture by (§11.1).
const REFUSAL_CODE: &str = "imagine:content-moderated";

/// Words a refusal without that code is told by, lowercase — xAI's codes are
/// not documented, and a renamed one must not turn a refusal into an error.
const REFUSAL_WORDS: [&str; 3] = ["moderation", "moderated", "policy"];

/// Ticks in a US dollar: "one USD cent equals 100,000,000 ticks".
const TICKS_PER_USD: f64 = 1e10;

pub struct XaiImages {
    http: reqwest::Client,
    cfg: ImageGenConfig,
}

impl XaiImages {
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

    /// The request body. Pure — the shape is what the tests assert on.
    pub(crate) fn body(&self, req: &ImageRequest) -> Value {
        let mut body = json!({
            "model": self.cfg.model,
            "prompt": prompt_for(req),
            "aspect_ratio": req.aspect.ratio(),
            "response_format": "b64_json",
            "n": 1,
        });
        if let Some(quality) = self.cfg.quality.as_deref().filter(|q| *q != "auto") {
            body["quality"] = json!(quality);
        }
        let reference = |i: &super::InputImage| json!({ "url": i.data_url(), "type": "image_url" });
        match req.inputs.as_slice() {
            [] => {}
            [one] => body["image"] = reference(one),
            many => body["images"] = many.iter().map(reference).collect(),
        }
        body
    }
}

/// The prompt as xAI reads it: with several pictures to change, a first line
/// that names each the way xAI's prompts do — the model wrote "the first
/// picture", not `<IMAGE_0>`. One picture needs no name.
fn prompt_for(req: &ImageRequest) -> String {
    if req.inputs.len() < 2 {
        return req.prompt.clone();
    }
    let names: Vec<String> = (0..req.inputs.len())
        .map(|i| format!("<IMAGE_{i}>"))
        .collect();
    format!(
        "The pictures, in the order given: {}.\n\n{}",
        names.join(", "),
        req.prompt
    )
}

fn spent_in(body: &Value) -> Spent {
    let usage = body.get("usage");
    let n = |key: &str| usage.and_then(|u| u.get(key)).and_then(Value::as_u64);
    Spent {
        input_tokens: n("input_tokens"),
        output_tokens: n("output_tokens"),
        cost_usd: n("cost_in_usd_ticks").map(|ticks| ticks as f64 / TICKS_PER_USD),
    }
}

/// What xAI said went wrong: `error` is a string on its answers, and an object
/// with a `message` is read too.
fn error_text(body: &Value) -> Option<String> {
    let error = body.get("error").filter(|e| !e.is_null())?;
    let said = match error {
        Value::String(s) => s.as_str(),
        other => other
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    };
    Some(said.trim().to_string()).filter(|s| !s.is_empty())
}

/// Whether an error answer is a refusal: the code xAI refuses by, or a `400`
/// whose words say moderation.
fn is_refusal(status: reqwest::StatusCode, body: &Value, said: &str) -> bool {
    if body.get("code").and_then(Value::as_str) == Some(REFUSAL_CODE) {
        return true;
    }
    let said = said.to_lowercase();
    status == reqwest::StatusCode::BAD_REQUEST && REFUSAL_WORDS.iter().any(|w| said.contains(w))
}

/// The picture, or the refusal, out of what xAI answered. Pure — tested on the
/// bodies the probe recorded.
pub(crate) fn answer_from(status: reqwest::StatusCode, text: &str) -> anyhow::Result<Drawing> {
    let Ok(body) = serde_json::from_str::<Value>(text) else {
        anyhow::bail!("{SUBJECT}: status {status}: {}", excerpt(text));
    };
    if let Some(said) = error_text(&body) {
        if is_refusal(status, &body, &said) {
            return Ok(Drawing::Refused {
                reason: said,
                spent: spent_in(&body),
            });
        }
        anyhow::bail!("{SUBJECT}: status {status}: {said}");
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
    let revised_prompt = body
        .pointer("/data/0/revised_prompt")
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
impl ImageGenerator for XaiImages {
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
            Ok(Drawing::Refused { reason, spent }) => {
                tracing::info!(%status, %reason, ?spent, "xAI refused to draw");
            }
            Err(err) => tracing::warn!(%status, error = %err, "xAI did not draw"),
            Ok(Drawing::Picture(p)) => {
                tracing::info!(bytes = p.bytes.len(), spent = ?p.spent, "xAI drew");
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

    fn client(model: &str, quality: Option<&str>) -> XaiImages {
        XaiImages::new(ImageGenConfig {
            provider: ImageGenProvider::Grok,
            attribution: false,
            model: model.into(),
            quality: quality.map(str::to_string),
            base_url: "https://api.x.ai/v1".into(),
            api_key: "k".into(),
        })
    }

    fn request(inputs: usize) -> ImageRequest {
        let png = crate::shared::image_gen::tests::tiny_png();
        ImageRequest {
            prompt: "a red fox in the snow".into(),
            aspect: Aspect::Wide,
            inputs: (0..inputs)
                .map(|_| InputImage {
                    mime: "image/png".into(),
                    bytes: png.clone(),
                })
                .collect(),
        }
    }

    /// A new picture: the aspect as a ratio, base64 asked for, the quality
    /// sent — and `auto`, xAI's own default, never, so a model that takes no
    /// quality still draws.
    #[test]
    fn a_new_picture_asks_for_the_ratio_and_base64() {
        let c = client("grok-imagine-image-2.0", Some("medium"));
        let req = request(0);
        assert_eq!(c.url(&req), "https://api.x.ai/v1/images/generations");
        assert_eq!(
            c.body(&req),
            json!({
                "model": "grok-imagine-image-2.0",
                "prompt": "a red fox in the snow",
                "aspect_ratio": "16:9",
                "response_format": "b64_json",
                "n": 1,
                "quality": "medium",
            })
        );
        for quality in [Some("auto"), None] {
            let body = client("grok-imagine-image", quality).body(&req);
            assert!(body.get("quality").is_none(), "{quality:?}: {body}");
        }
    }

    /// One picture to change goes as `image`, the form the probe measured;
    /// several as `images`, named in the prompt the way xAI's prompts name them.
    #[test]
    fn pictures_to_change_go_as_image_or_images() {
        let c = client("grok-imagine-image-2.0", None);
        let one = c.body(&request(1));
        assert_eq!(c.url(&request(1)), "https://api.x.ai/v1/images/edits");
        assert_eq!(one["image"]["type"], "image_url");
        assert!(
            one["image"]["url"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
        assert!(one.get("images").is_none());
        assert_eq!(one["prompt"], "a red fox in the snow", "one needs no name");

        let two = c.body(&request(2));
        assert!(two.get("image").is_none());
        assert_eq!(two["images"].as_array().unwrap().len(), 2);
        assert_eq!(
            two["prompt"],
            "The pictures, in the order given: <IMAGE_0>, <IMAGE_1>.\n\na red fox in the snow"
        );
    }

    /// The answer of §11.1: a JPEG, and the price xAI stated — 400 000 000
    /// ticks are four cents.
    #[test]
    fn a_picture_is_read_with_its_price_in_ticks() {
        let mut jpeg = Vec::new();
        image::RgbImage::from_pixel(2, 2, image::Rgb([10, 20, 30]))
            .write_to(
                &mut std::io::Cursor::new(&mut jpeg),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&jpeg);
        let text = json!({"data": [{"b64_json": b64, "mime_type": "image/jpeg"}],
            "usage": {"cost_in_usd_ticks": 400_000_000u64}})
        .to_string();
        let Drawing::Picture(p) = answer_from(reqwest::StatusCode::OK, &text).unwrap() else {
            panic!("a picture");
        };
        assert_eq!(p.bytes, jpeg);
        assert_eq!(p.mime, "image/jpeg");
        assert_eq!(p.spent.cost_usd, Some(0.04));
        assert_eq!(p.spent.input_tokens, None);
    }

    /// The refusal the probe recorded, word for word: an answer, with the four
    /// cents it was charged.
    #[test]
    fn the_moderated_picture_is_a_refusal_with_its_price() {
        let text = json!({"code": "imagine:content-moderated",
            "error": "Generated image rejected by content moderation.",
            "usage": {"cost_in_usd_ticks": 400_000_000u64}})
        .to_string();
        let Drawing::Refused { reason, spent } =
            answer_from(reqwest::StatusCode::BAD_REQUEST, &text).unwrap()
        else {
            panic!("a refusal");
        };
        assert_eq!(reason, "Generated image rejected by content moderation.");
        assert_eq!(spent.cost_usd, Some(0.04));

        // A renamed code still reads as a refusal by its words.
        let renamed =
            json!({"code": "imagine:blocked", "error": "Rejected by content moderation."});
        assert!(matches!(
            answer_from(reqwest::StatusCode::BAD_REQUEST, &renamed.to_string()).unwrap(),
            Drawing::Refused { .. }
        ));
    }

    /// Every other failure is an error in xAI's words: a key, a model, a body
    /// that is not JSON, an answer with no picture.
    #[test]
    fn any_other_error_is_said_in_xais_words() {
        let key = json!({"code": "Client specified an invalid argument",
            "error": "Incorrect API key provided: xa***. You can obtain an API key from https://console.x.ai."});
        let err = answer_from(reqwest::StatusCode::BAD_REQUEST, &key.to_string())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("400") && err.contains("Incorrect API key"),
            "{err}"
        );

        let object = json!({"error": {"message": "The model grok-imagine-image-9 does not exist"}});
        let err = answer_from(reqwest::StatusCode::NOT_FOUND, &object.to_string())
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not exist"), "{err}");

        let err = answer_from(reqwest::StatusCode::BAD_GATEWAY, "<html>bad</html>").unwrap_err();
        assert!(err.to_string().contains("<html>bad</html>"));
        let err = answer_from(reqwest::StatusCode::OK, r#"{"data": []}"#).unwrap_err();
        assert!(err.to_string().contains("no picture"));
    }
}

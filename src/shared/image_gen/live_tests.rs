//! Live smokes of the four picture clients against the real providers.
//! `#[ignore]` — not in CI; silently skipped without their key. Each draws one
//! picture and changes it once: about $0.03 on OpenAI, $0.08 on Gemini,
//! $0.10 on the gateway's FLUX-3 and $0.11 on xAI, which also changes two at
//! once (docs/history/image-generation.md §11.1, §14). No refusal is provoked:
//! the probe recorded each provider's once, and repeated violations are what an
//! account gets flagged for.
//!
//! A file of its own, named as a test file: the coverage report leaves such
//! files out (docs/journal/quality.md).
//!
//! `MINDFORK_OPENAI_KEY=… MINDFORK_GEMINI_KEY=… MINDFORK_OPENROUTER_KEY=…
//! MINDFORK_GROK_KEY=… cargo test image_gen::live_tests -- --ignored --nocapture
//! --test-threads=1`;
//! `MINDFORK_LIVE_IMAGE_GATEWAY_MODEL` picks the gateway's model
//! (`black-forest-labs/flux-3-image` when unset).

use std::time::Instant;

use tokio_util::sync::CancellationToken;

use super::{Aspect, Drawing, ImageGenSettings, ImageRequest, InputImage, client, resolve};
use crate::shared::config::ImageGenProvider;

/// The provider's client from the default settings and the key in `var`, with
/// `model` in place of the section's when given.
fn configured(
    provider: ImageGenProvider,
    var: &str,
    model: Option<String>,
) -> Option<std::sync::Arc<dyn super::ImageGenerator>> {
    let Ok(key) = std::env::var(var) else {
        eprintln!("skip: {var} not set");
        return None;
    };
    let mut settings = ImageGenSettings {
        provider,
        ..Default::default()
    };
    if let Some(model) = model {
        settings.section_mut().model_name = Some(model);
    }
    let cfg = resolve(&settings, Some(key), false)
        .config
        .expect("a key and a model configure the slot");
    Some(client(cfg))
}

/// Draws a picture, then changes it, and says what each took. Returns both.
async fn draw_and_change(
    label: &str,
    images: std::sync::Arc<dyn super::ImageGenerator>,
) -> (super::Picture, super::Picture) {
    let cancel = CancellationToken::new();
    let started = Instant::now();
    let first = images
        .draw(
            ImageRequest {
                prompt: "A watercolor painting of a red fox sitting in fresh snow, \
                         soft morning light, pine trees in the background."
                    .into(),
                aspect: Aspect::Wide,
                inputs: Vec::new(),
            },
            &cancel,
        )
        .await
        .unwrap_or_else(|e| panic!("{label}: the first picture: {e:#}"));
    let Drawing::Picture(first) = first else {
        panic!("{label}: the first picture was refused: {first:?}");
    };
    let (w, h) = image::ImageReader::new(std::io::Cursor::new(&first.bytes))
        .with_guessed_format()
        .unwrap()
        .into_dimensions()
        .expect("the picture decodes");
    eprintln!(
        "{label}: drew {}x{} {} ({} bytes) in {:.1}s, spent {:?}",
        w,
        h,
        first.mime,
        first.bytes.len(),
        started.elapsed().as_secs_f64(),
        first.spent
    );
    assert!(w > h, "{label}: a wide picture, got {w}x{h}");

    let started = Instant::now();
    let second = images
        .draw(
            ImageRequest {
                prompt: "Make it night: a full moon and blue moonlight on the snow. \
                         Keep the fox exactly as it is."
                    .into(),
                aspect: Aspect::Wide,
                inputs: vec![InputImage {
                    mime: first.mime.to_string(),
                    bytes: first.bytes.clone(),
                }],
            },
            &cancel,
        )
        .await
        .unwrap_or_else(|e| panic!("{label}: the change: {e:#}"));
    let Drawing::Picture(second) = second else {
        panic!("{label}: the change was refused: {second:?}");
    };
    eprintln!(
        "{label}: changed it ({} bytes, {}) in {:.1}s, spent {:?}",
        second.bytes.len(),
        second.mime,
        started.elapsed().as_secs_f64(),
        second.spent
    );
    assert_ne!(second.bytes, first.bytes, "{label}: a new picture");
    (first, second)
}

#[tokio::test]
#[ignore = "requires MINDFORK_OPENAI_KEY; spends about $0.03"]
async fn openai_draws_and_changes_a_picture_live() {
    if let Some(images) = configured(ImageGenProvider::OpenAi, "MINDFORK_OPENAI_KEY", None) {
        draw_and_change("openai", images).await;
    }
}

#[tokio::test]
#[ignore = "requires MINDFORK_GEMINI_KEY; spends about $0.08"]
async fn gemini_draws_and_changes_a_picture_live() {
    if let Some(images) = configured(ImageGenProvider::Gemini, "MINDFORK_GEMINI_KEY", None) {
        draw_and_change("gemini", images).await;
    }
}

#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY; spends about $0.10 on the default model"]
async fn the_gateway_draws_and_changes_a_picture_live() {
    let model = std::env::var("MINDFORK_LIVE_IMAGE_GATEWAY_MODEL")
        .unwrap_or_else(|_| "black-forest-labs/flux-3-image".into());
    if let Some(images) = configured(
        ImageGenProvider::OpenRouter,
        "MINDFORK_OPENROUTER_KEY",
        Some(model),
    ) {
        draw_and_change("openrouter", images).await;
    }
}

/// xAI's dialect (§14): the ratio and base64 on both routes, `quality` left to
/// xAI's own `auto`, the price read from its ticks — and a change of **two**
/// pictures at once, which goes as `images` and names them `<IMAGE_0>`,
/// `<IMAGE_1>` in the prompt.
#[tokio::test]
#[ignore = "requires MINDFORK_GROK_KEY; spends about $0.18"]
async fn xai_draws_and_changes_a_picture_live() {
    let Some(images) = configured(ImageGenProvider::Grok, "MINDFORK_GROK_KEY", None) else {
        return;
    };
    let (first, second) = draw_and_change("xai", images.clone()).await;
    for p in [&first, &second] {
        assert_eq!(p.mime, "image/jpeg", "xAI answers in JPEG");
        assert!(
            p.spent.cost_usd.is_some_and(|c| c > 0.0),
            "xAI states the price: {:?}",
            p.spent
        );
    }
    let cancel = CancellationToken::new();
    let started = Instant::now();
    let both = images
        .draw(
            ImageRequest {
                prompt: "Put the fox from the first picture into the moonlit night of the                          second, side by side with the fox already there."
                    .into(),
                aspect: Aspect::Wide,
                inputs: [&first, &second]
                    .iter()
                    .map(|p| InputImage {
                        mime: p.mime.to_string(),
                        bytes: p.bytes.clone(),
                    })
                    .collect(),
            },
            &cancel,
        )
        .await
        .unwrap_or_else(|e| panic!("xai: the change of two: {e:#}"));
    let Drawing::Picture(both) = both else {
        panic!("xai: the change of two was refused: {both:?}");
    };
    eprintln!(
        "xai: changed two ({} bytes) in {:.1}s, spent {:?}",
        both.bytes.len(),
        started.elapsed().as_secs_f64(),
        both.spent
    );
}

//! `image_generate` against a scripted provider: what is kept, what is said,
//! what is shown, and the three bounds of a turn (spec §9.15,
//! docs/research/image-generation.md §6.3–§6.6).

use std::sync::Arc;

use base64::Engine as _;
use uuid::Uuid;

use super::{IMAGE_GENERATE_ID, ImageGenerate, MAX_INPUTS};
use crate::entities::chat_file::{ChatFile, FileOrigin};
use crate::features::tools::testkit::ctx_with_storage_lang;
use crate::features::tools::{ChatEffect, Tool, ToolContext, ToolOutcome};
use crate::shared::config::{ImageGenProvider, ImageGenSettings, ImageSettings};
use crate::shared::i18n::Lang;
use crate::shared::image_gen::mock::MockImages;
use crate::shared::image_gen::{Drawing, ImageGenConfig, ImageGenSlot, Picture, resolve};

/// A real PNG of `w`×`h`.
fn png(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    image::RgbImage::from_pixel(w, h, image::Rgb([10, 120, 200]))
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

/// A configured slot of the default settings, with `edit` applied.
fn slot(edit: impl FnOnce(&mut ImageGenSettings)) -> ImageGenSlot {
    let mut settings = ImageGenSettings {
        enabled: true,
        ..Default::default()
    };
    edit(&mut settings);
    resolve(&settings, Some("k".into()), false)
}

fn tool(slot: ImageGenSlot, images: Arc<MockImages>) -> ImageGenerate {
    ImageGenerate::new(Some(slot), Some(images), ImageSettings::default())
}

/// A context on an English profile whose chat keeps its files in a temp folder.
fn ctx() -> (tempfile::TempDir, tempfile::TempDir, ToolContext) {
    let (dir, _storage, mut ctx) = ctx_with_storage_lang(Uuid::new_v4(), Lang::En);
    let folder = tempfile::tempdir().unwrap();
    ctx.files_dir = Some(folder.path().to_path_buf());
    (dir, folder, ctx)
}

fn args(prompt: &str) -> serde_json::Value {
    serde_json::json!({ "prompt": prompt })
}

fn stored(out: &ToolOutcome) -> Vec<ChatFile> {
    out.effects
        .iter()
        .filter_map(|e| match e {
            ChatEffect::AddChatFile(f) => Some((**f).clone()),
            _ => None,
        })
        .collect()
}

/// The original is kept as the provider sent it, under `image-1.png`, as a
/// generated file; the result names it by the `#N` `/file list` will show and
/// hands the loop the same picture to show, the entry line beside it.
#[tokio::test]
async fn a_picture_is_kept_as_a_generated_file_and_offered_to_the_model() {
    let (_d, folder, ctx) = ctx();
    let picture = png(48, 27);
    let images = Arc::new(MockImages::drawing(&picture));
    let out = tool(slot(|_| {}), images.clone())
        .invoke(
            &ctx,
            serde_json::json!({"prompt": " a fox ", "aspect": "16:9"}),
        )
        .await
        .unwrap();

    let calls = images.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].prompt, "a fox");
    assert_eq!(calls[0].aspect, crate::shared::image_gen::Aspect::Wide);
    assert!(calls[0].inputs.is_empty());

    let files = stored(&out);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].name, "image-1.png");
    assert_eq!(files[0].origin, FileOrigin::Generated);
    assert_eq!(
        std::fs::read(folder.path().join("image-1.png")).unwrap(),
        picture
    );

    let first = out.result.lines().next().unwrap();
    assert!(
        first.starts_with("Saved as #1 image-1.png (48×27, "),
        "{first}"
    );
    assert!(first.contains("/file open #1"), "{first}");
    assert_eq!(out.images.len(), 1);
    assert_eq!(out.images[0].mime, "image/png");
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(&out.images[0].data)
            .unwrap(),
        picture
    );
    assert_eq!(out.images[0].entry.as_deref(), Some(first));
    assert_eq!(ctx.draws.drawn(), 1);
}

/// The next picture takes the next free number, past any extension the chat
/// already holds it under; a JPEG is kept as `.jpg`.
#[tokio::test]
async fn the_next_picture_takes_the_first_free_number() {
    let (_d, folder, mut ctx) = ctx();
    std::fs::write(folder.path().join("image-1.png"), b"x").unwrap();
    let listed = ChatFile::new("IMAGE-2.webp", FileOrigin::Generated, b"y");
    ctx.files = Arc::from(vec![listed]);
    let mut jpeg = Vec::new();
    image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3]))
        .write_to(
            &mut std::io::Cursor::new(&mut jpeg),
            image::ImageFormat::Jpeg,
        )
        .unwrap();
    let out = tool(slot(|_| {}), Arc::new(MockImages::drawing(&jpeg)))
        .invoke(&ctx, args("a fox"))
        .await
        .unwrap();
    let files = stored(&out);
    assert_eq!(files[0].name, "image-3.jpg");
    assert!(
        out.result.starts_with("Saved as #2 image-3.jpg"),
        "the listed file is #1: {}",
        out.result
    );
}

/// With showing switched off the picture is kept and nothing goes to the
/// model, which is told in the measured words (§11.2's control arm).
#[tokio::test]
async fn shown_off_keeps_the_picture_and_says_it_is_not_seen() {
    let (_d, _folder, ctx) = ctx();
    let out = tool(
        slot(|s| s.show_to_model = false),
        Arc::new(MockImages::drawing(&png(8, 8))),
    )
    .invoke(&ctx, args("a fox"))
    .await
    .unwrap();
    assert!(out.images.is_empty());
    assert_eq!(stored(&out).len(), 1);
    assert!(
        out.result.contains("You cannot see this picture"),
        "{}",
        out.result
    );
}

/// A refusal is an answer in the directive form, and after it the turn draws
/// nothing more — the next call never reaches the provider (F10).
#[tokio::test]
async fn a_refusal_stops_drawing_for_the_turn() {
    let (_d, _folder, ctx) = ctx();
    let images = Arc::new(MockImages::refusing("violence"));
    let t = tool(slot(|_| {}), images.clone());
    let out = t.invoke(&ctx, args("something grim")).await.unwrap();
    assert!(
        out.result
            .starts_with("The provider refused to draw this: «violence»."),
        "{}",
        out.result
    );
    assert!(out.result.contains("ask what to change"));
    assert!(out.effects.is_empty() && out.images.is_empty());
    assert!(ctx.draws.refused());

    let again = t.invoke(&ctx, args("something softer")).await.unwrap();
    assert!(
        again
            .result
            .starts_with("Nothing was drawn: drawing is stopped"),
        "{}",
        again.result
    );
    assert_eq!(images.calls().len(), 1, "the provider was asked once");

    // A new turn's context starts afresh.
    let (_d2, _folder2, next_turn) = self::ctx();
    assert!(!next_turn.draws.refused());
}

/// The ceiling counts pictures drawn in the turn; `0` removes it (F7); a
/// sub-agent's clone of the context counts against the same ceiling.
#[tokio::test]
async fn the_ceiling_is_the_turns() {
    let (_d, _folder, ctx) = ctx();
    let images = Arc::new(MockImages::drawing(&png(2, 2)));
    let t = tool(slot(|s| s.max_per_turn = 2), images.clone());
    t.invoke(&ctx, args("one")).await.unwrap();
    let child = ctx.clone();
    t.invoke(&child, args("two")).await.unwrap();
    let third = t.invoke(&ctx, args("three")).await.unwrap();
    assert!(
        third
            .result
            .starts_with("Nothing was drawn: this turn has already drawn 2 picture(s)"),
        "{}",
        third.result
    );
    assert_eq!(images.calls().len(), 2);

    let (_d, _folder, ctx) = self::ctx();
    let t = tool(slot(|s| s.max_per_turn = 0), images.clone());
    for n in 0..6 {
        let out = t.invoke(&ctx, args(&format!("picture {n}"))).await.unwrap();
        assert!(out.result.starts_with("Saved as"), "{}", out.result);
    }
}

/// An error is not a refusal: it is said, it does not stop the turn, and it
/// does not count against the ceiling.
#[tokio::test]
async fn an_error_is_said_and_stops_nothing() {
    let (_d, _folder, ctx) = ctx();
    let images = Arc::new(MockImages::answering(vec![
        Err("OpenAI images: status 403 Forbidden: organization must be verified".into()),
        Ok(Drawing::Picture(Picture {
            bytes: png(2, 2),
            mime: "image/png",
            revised_prompt: Some("a fox, at dusk".into()),
            spent: crate::shared::image_gen::Spent {
                input_tokens: Some(12),
                output_tokens: Some(439),
                cost_usd: None,
            },
        })),
    ]));
    let t = tool(slot(|s| s.max_per_turn = 1), images);
    let failed = t.invoke(&ctx, args("a fox")).await.unwrap();
    assert!(
        failed
            .result
            .starts_with("The picture was not drawn: OpenAI images: status 403"),
        "{}",
        failed.result
    );
    assert!(!ctx.draws.refused());
    assert_eq!(ctx.draws.drawn(), 0);
    let drawn = t.invoke(&ctx, args("a fox")).await.unwrap();
    assert!(
        drawn
            .result
            .contains("The provider rewrote the prompt as: «a fox, at dusk»")
    );
    assert!(
        drawn
            .result
            .contains("The provider counted 12 input and 439 output tokens.")
    );
}

/// A slot that lacks a key or a model says which, and names the provider.
#[tokio::test]
async fn a_missing_key_or_model_is_said() {
    let (_d, _folder, ctx) = ctx();
    let no_key = resolve(
        &ImageGenSettings {
            enabled: true,
            ..Default::default()
        },
        None,
        false,
    );
    let out = ImageGenerate::new(Some(no_key), None, ImageSettings::default())
        .invoke(&ctx, args("a fox"))
        .await
        .unwrap();
    assert!(
        out.result
            .starts_with("Nothing was drawn: image generation has no API key for OpenAI"),
        "{}",
        out.result
    );

    let no_model = resolve(
        &ImageGenSettings {
            enabled: true,
            provider: ImageGenProvider::OpenRouter,
            ..Default::default()
        },
        Some("k".into()),
        false,
    );
    let out = ImageGenerate::new(Some(no_model), None, ImageSettings::default())
        .invoke(&ctx, args("a fox"))
        .await
        .unwrap();
    assert!(
        out.result.contains("no model chosen for OpenRouter"),
        "{}",
        out.result
    );
}

/// The arguments are checked before anything is asked: an empty prompt and a
/// shape the tool does not offer are the model's to fix.
#[tokio::test]
async fn bad_arguments_are_refused_before_the_provider() {
    let (_d, _folder, ctx) = ctx();
    let images = Arc::new(MockImages::drawing(&png(2, 2)));
    let t = tool(slot(|_| {}), images.clone());
    assert!(t.invoke(&ctx, args("  ")).await.is_err());
    let err = t
        .invoke(
            &ctx,
            serde_json::json!({"prompt": "a fox", "aspect": "4:3"}),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("4:3"), "{err}");
    assert!(images.calls().is_empty());
}

/// A picture to change is named as the chat names it — a stored file by its
/// `#N` or its name, an image of the conversation by its name with or without
/// the extension — and reaches the provider as its bytes.
#[tokio::test]
async fn pictures_to_change_are_named_as_the_chat_names_them() {
    let (_d, folder, mut ctx) = ctx();
    let stored_png = png(6, 6);
    std::fs::write(folder.path().join("image-1.png"), &stored_png).unwrap();
    let listed = ChatFile::new("image-1.png", FileOrigin::Generated, &stored_png);
    ctx.files = Arc::from(vec![listed]);
    let shown = png(3, 3);
    ctx.images = Arc::from(vec![crate::entities::message_image::MessageImage::new(
        "tool-image-2.png",
        "tool:x",
        "image/png",
        3,
        3,
        base64::engine::general_purpose::STANDARD.encode(&shown),
    )]);
    let images = Arc::new(MockImages::drawing(&png(2, 2)));
    let t = tool(slot(|s| s.max_per_turn = 0), images.clone());
    for handle in ["#1", "image-1.png", "IMAGE-1.PNG"] {
        t.invoke(
            &ctx,
            serde_json::json!({"prompt": "darker", "images": [handle]}),
        )
        .await
        .unwrap();
    }
    t.invoke(
        &ctx,
        serde_json::json!({"prompt": "darker", "images": ["tool-image-2"]}),
    )
    .await
    .unwrap();
    let calls = images.calls();
    for call in &calls[..3] {
        assert_eq!(call.inputs.len(), 1);
        assert_eq!(call.inputs[0].bytes, stored_png);
        assert_eq!(call.inputs[0].mime, "image/png");
    }
    assert_eq!(calls[3].inputs[0].bytes, shown);

    let err = t
        .invoke(
            &ctx,
            serde_json::json!({"prompt": "darker", "images": ["cat.png"]}),
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("the chat has no picture by this name"),
        "{err}"
    );
}

/// A stored file that is not a picture cannot be changed, and too many
/// pictures are refused before any is read.
#[tokio::test]
async fn only_pictures_and_only_so_many() {
    let (_d, folder, mut ctx) = ctx();
    std::fs::write(folder.path().join("notes-1.txt"), b"hello").unwrap();
    ctx.files = Arc::from(vec![ChatFile::new(
        "notes-1.txt",
        FileOrigin::Sandbox,
        b"hello",
    )]);
    let t = tool(slot(|_| {}), Arc::new(MockImages::drawing(&png(2, 2))));
    let err = t
        .invoke(
            &ctx,
            serde_json::json!({"prompt": "x", "images": ["notes-1.txt"]}),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("is not a picture"), "{err}");

    let many: Vec<String> = (0..=MAX_INPUTS).map(|n| format!("#{n}")).collect();
    let err = t
        .invoke(&ctx, serde_json::json!({"prompt": "x", "images": many}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("at most 8"), "{err}");
}

/// A picture larger than the loop's ceiling is handed over as a prepared copy
/// — the loop drops an oversized tool image before downscaling it (§6.4) — and
/// the original is kept whole.
#[tokio::test]
async fn an_oversized_picture_is_handed_over_prepared() {
    let (_d, folder, ctx) = ctx();
    let big = png(64, 32);
    let limits = ImageSettings {
        max_bytes: (big.len() - 1) as u64,
        downscale_px: 16,
        ..ImageSettings::default()
    };
    let out = ImageGenerate::new(
        Some(slot(|_| {})),
        Some(Arc::new(MockImages::drawing(&big))),
        limits,
    )
    .invoke(&ctx, args("wide"))
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(folder.path().join("image-1.png")).unwrap(),
        big
    );
    let handed = base64::engine::general_purpose::STANDARD
        .decode(&out.images[0].data)
        .unwrap();
    let (w, h) = image::ImageReader::new(std::io::Cursor::new(&handed))
        .with_guessed_format()
        .unwrap()
        .into_dimensions()
        .unwrap();
    assert_eq!((w, h), (16, 8));
}

/// The description and the parameters exist in every scaffold language, and
/// the shapes the schema offers are the ones the tool parses.
#[test]
fn the_schema_offers_the_shapes_the_tool_takes() {
    let t = ImageGenerate::new(None, None, ImageSettings::default());
    for lang in Lang::ALL {
        let loc = crate::shared::i18n::locale(*lang);
        assert!(t.description(loc).contains("prompt"), "{lang:?}");
        let params = t.parameters(loc);
        let offered: Vec<&str> = params["properties"]["aspect"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(offered, ["1:1", "3:2", "2:3", "16:9", "9:16"]);
        assert_eq!(params["required"], serde_json::json!(["prompt"]));
    }
    assert_eq!(t.gate(), Some(super::super::meta::ToolGate::ImageGen));
    assert!(!t.danger(), "a messenger's turn keeps it (ADR 0013, D3)");
}

/// The gateway's model is configured like any other: the slot is resolved,
/// the client built — the registry's path.
#[test]
fn the_registry_builds_the_tool_from_the_slot() {
    let cfg = crate::features::tools::ToolConfig {
        image_gen: Some(slot(|_| {})),
        ..Default::default()
    };
    let reg = crate::features::tools::standard_registry(&cfg);
    assert!(reg.get(IMAGE_GENERATE_ID).is_some());
    let resolved: ImageGenConfig = slot(|_| {}).config.unwrap();
    assert_eq!(resolved.model, "gpt-image-2.5-flare");
}

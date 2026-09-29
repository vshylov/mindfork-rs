//! Live smokes of video through the OpenRouter gateway — `#[ignore]`, run by
//! hand against the gateway before a change to this mode is merged
//! (AGENTS.md §3). Part of the mode's live gate: `cargo test gateway_live --
//! --ignored --nocapture --test-threads=1`.
//!
//! Declared by `MINDFORK_OPENROUTER_KEY`; without it every smoke skips, saying
//! so. The model is the one the smokes were measured on, or what
//! `MINDFORK_OPENROUTER_VIDEO_MODEL` names — which has to be of the family that
//! takes a YouTube link, or the smokes fail rather than skip
//! (docs/lessons.md §9).
//!
//! **The video is one no model can describe from memory**: 67 seconds,
//! published on 2026-09-08, after every model's cutoff. What is asserted of it
//! is a line of what is said in it — and, by the client itself, that the
//! gateway counted video tokens: an answer without them is an error, which is
//! the third smoke.
//!
//! The file is named `…_tests` like every test file of the crate: the coverage
//! report leaves files so named out, and one it does not recognise is
//! production code to it.

use tokio_util::sync::CancellationToken;

use super::gateway::GatewayVideo;
use super::{VideoAnswer, VideoConfig, VideoRequest, VideoUnderstanding};
use crate::shared::api::catalogue::{self, CatalogueRequest, CatalogueShape, ModelSlot};
use crate::shared::config::{CloudProvider, MediaResolution, VideoProvider};

/// Published 2026-09-08, 67 s: NASA, about going back to the moon.
pub(crate) const FRESH_VIDEO: &str = "https://www.youtube.com/watch?v=IwZVXmQdX1E";
/// A line of what is said in it, as three routes transcribed it.
pub(crate) const A_LINE_OF_IT: &str = "bound for the moon";
/// Measured on 2026-09-29: 6 119 prompt tokens for the video, $0.0019.
const MEASURED_ON: &str = "google/gemini-3.5-flash-lite";

pub(crate) fn key() -> Option<String> {
    let key = std::env::var("MINDFORK_OPENROUTER_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty());
    if key.is_none() {
        eprintln!("skip: MINDFORK_OPENROUTER_KEY not set");
    }
    key
}

/// The slot's config as the orchestrator resolves it, for this key.
pub(crate) fn config(key: &str) -> VideoConfig {
    let model = std::env::var("MINDFORK_OPENROUTER_VIDEO_MODEL")
        .ok()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| MEASURED_ON.to_string());
    VideoConfig {
        provider: VideoProvider::OpenRouter,
        attribution: true,
        model,
        base_url: CloudProvider::OpenRouter.chat_base_url().to_string(),
        api_key: key.to_string(),
        media_resolution: MediaResolution::Low,
        max_minutes: 30,
    }
}

async fn asked(client: &GatewayVideo, url: &str, prompt: &str) -> anyhow::Result<VideoAnswer> {
    let request = VideoRequest {
        url: url.to_string(),
        prompt: prompt.to_string(),
        start_secs: None,
        end_secs: None,
        max_output_tokens: 1200,
    };
    let started = std::time::Instant::now();
    let answer = client.describe(request, &CancellationToken::new()).await;
    match &answer {
        Ok(answer) => eprintln!(
            "   answered in {:.1?}, cut: {}: {}",
            started.elapsed(),
            answer.truncated,
            answer.text.replace('\n', " ")
        ),
        Err(err) => eprintln!("   refused in {:.1?}: {err}", started.elapsed()),
    }
    answer
}

/// The video row's list, as the picker asks for it.
async fn video_list(key: Option<&str>) -> Vec<catalogue::CatalogModel> {
    let request = CatalogueRequest {
        shape: CatalogueShape::OpenRouterVideo,
        base: CloudProvider::OpenRouter.chat_base_url().to_string(),
        key: key.map(str::to_string),
        attribution: true,
    };
    let listed = catalogue::fetch(&request).await.expect("the list");
    eprintln!("   the gateway sent {} entries", listed.len());
    catalogue::for_slot(listed, ModelSlot::Video)
}

/// The list behind the video row is the models that claim video, whichever
/// way it was asked: the public list comes narrowed by the gateway, and the
/// account's — which the gateway does not narrow — is narrowed here to about
/// as many. Gemini opens both, and the model of these smokes is in them.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY"]
async fn the_gateways_video_list_answers_live() {
    let Some(key) = key() else { return };
    let public = video_list(None).await;
    let mine = video_list(Some(&key)).await;
    eprintln!(
        "   offered: {} without a key, {} with one; the first: {:?}, {:?}",
        public.len(),
        mine.len(),
        public.first().map(|m| &m.id),
        mine.first().map(|m| &m.id)
    );
    for (name, list) in [("public", &public), ("the account's", &mine)] {
        assert!(list.len() >= 20, "{name}: {}", list.len());
        assert!(
            list.iter().all(|m| m.facts.video == Some(true)),
            "{name}: every entry says it takes video"
        );
        assert!(
            list.iter().all(|m| !m.id.ends_with(":batch")),
            "{name}: no batch twin"
        );
        let gemini = |m: &catalogue::CatalogModel| m.id.contains("google/gemini");
        let family = list.iter().filter(|m| gemini(m)).count();
        assert!(family >= 5, "{name}: {family} of the family");
        assert!(
            list.iter().take(family).all(gemini),
            "{name}: the family opens the list"
        );
        assert!(list.iter().any(|m| m.id == MEASURED_ON), "{name}");
    }
    let apart = public.len().abs_diff(mine.len());
    assert!(
        apart <= public.len() / 4,
        "the two lists are of one size, give or take the account's own settings: \
         {} and {}",
        public.len(),
        mine.len()
    );
}

/// The go/no-go: the video of 2026-09-08 is watched — the client, which
/// refuses an answer without video tokens, hands one over — and the answer
/// holds a line of what is said in the video.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY"]
async fn a_video_is_watched_through_the_gateway_live() {
    let Some(key) = key() else { return };
    let client = GatewayVideo::new(config(&key));
    let answer = asked(
        &client,
        FRESH_VIDEO,
        "Transcribe the first three sentences spoken in this video, verbatim.",
    )
    .await
    .expect("the video is watched");
    assert!(
        answer.text.to_lowercase().contains(A_LINE_OF_IT),
        "{}",
        answer.text
    );
    assert!(!answer.truncated);
}

/// The go/no-go's other half: a link the gateway takes and does not read as
/// a video is an error, not a description. The gateway answers this one
/// `200`, with a text — and no video tokens.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY"]
async fn a_link_the_gateway_does_not_read_is_an_error_not_a_description_live() {
    let Some(key) = key() else { return };
    let client = GatewayVideo::new(config(&key));
    let err = asked(
        &client,
        "https://example.com/",
        "Say what this video shows in one sentence.",
    )
    .await
    .expect_err("nothing was watched");
    let text = err.to_string();
    assert!(text.contains("without reading the video"), "{text}");
}

/// A video that is not there is the provider's refusal, passed on by the
/// gateway and said in both their words.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY"]
async fn a_video_that_is_not_there_is_a_refusal_in_the_providers_words_live() {
    let Some(key) = key() else { return };
    let client = GatewayVideo::new(config(&key));
    let err = asked(
        &client,
        "https://www.youtube.com/watch?v=aaaaaaaaaaa",
        "Say what this video shows in one sentence.",
    )
    .await
    .expect_err("there is no such video");
    let text = err.to_string();
    assert!(text.contains("Provider returned error"), "{text}");
    assert!(
        !text.contains("metadata"),
        "the sentence, not the JSON: {text}"
    );
}

/// A key the gateway refuses is said in the gateway's words, by the request
/// that needed it: the slot has no status to say it in earlier.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY"]
async fn a_refused_key_is_said_in_the_gateways_words_live() {
    if key().is_none() {
        return;
    }
    // A key of the right shape that is nobody's: nothing of this machine's is
    // sent in its place.
    let nobody = format!("sk-or-v1-{}", "0".repeat(64));
    let client = GatewayVideo::new(config(&nobody));
    let err = asked(&client, FRESH_VIDEO, "Say what this video shows.")
        .await
        .expect_err("refused");
    let text = err.to_string();
    assert!(
        text.contains("401") && text.contains("User not found"),
        "{text}"
    );
}

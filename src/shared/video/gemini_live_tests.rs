//! Live smokes of the Gemini video client's muted thinking against the real
//! API. `#[ignore]` — not in CI; silently skipped without their variables.
//!
//! A file of its own, named as a test file: the coverage report leaves such
//! files out (docs/journal/quality.md).
//!
//! Run both arms (the variables are the chat client's — the same two models
//! refuse and take the level on both request paths):
//! `MINDFORK_GEMINI_KEY=… MINDFORK_LIVE_NO_MINIMAL_LEVEL_MODEL=gemini-3.8-flash
//! MINDFORK_LIVE_MINIMAL_LEVEL_MODEL=gemini-3.5-flash cargo test
//! gemini_live_tests -- --ignored --nocapture --test-threads=1`.

use tokio_util::sync::CancellationToken;

use super::gemini::GeminiVideo;
use super::{VideoConfig, VideoRequest, VideoUnderstanding, resolve_config};
use crate::shared::config::VideoSettings;

/// A client for the model `var` declares, or `None` when the key or the
/// declaration is missing.
fn client_for(var: &str) -> Option<(String, GeminiVideo)> {
    let Ok(key) = std::env::var("MINDFORK_GEMINI_KEY") else {
        eprintln!("skip: MINDFORK_GEMINI_KEY not set");
        return None;
    };
    let Ok(model) = std::env::var(var) else {
        eprintln!("skip: {var} not set");
        return None;
    };
    let cfg = resolve_config(&VideoSettings::default(), Some(key), false)
        .expect("a key and the default settings are enough to configure the slot");
    let client = GeminiVideo::new(VideoConfig {
        model: model.clone(),
        ..cfg
    });
    Some((model, client))
}

/// Ten seconds of a video, the cheapest request that is still a video's.
fn ten_seconds() -> VideoRequest {
    VideoRequest {
        url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into(),
        prompt: "In one sentence: what is shown on screen?".into(),
        start_secs: Some(0),
        end_secs: Some(10),
        max_output_tokens: 300,
    }
}

/// The defect, live: a Gemini 3.x model that answers `thinkingLevel:
/// "minimal"` with a `400` and is not a Pro —
/// `MINDFORK_LIVE_NO_MINIMAL_LEVEL_MODEL` declares one (`gemini-3.7-flash`,
/// `gemini-3.8-flash`; measured on this request 2026-10-01). The video client
/// mutes thinking at that level, so with such a model in the video slot every
/// `youtube_watch` was the `400` itself.
///
/// Declared rather than guessed, so the smoke **fails** rather than passes when
/// the recovery never ran.
#[tokio::test]
#[ignore = "requires MINDFORK_GEMINI_KEY + MINDFORK_LIVE_NO_MINIMAL_LEVEL_MODEL (a Gemini 3.x model, not a Pro, that refuses thinkingLevel \"minimal\")"]
async fn a_video_is_described_by_a_model_without_the_minimal_level() {
    let Some((model, client)) = client_for("MINDFORK_LIVE_NO_MINIMAL_LEVEL_MODEL") else {
        return;
    };
    // Twice through one client: the first request meets the refusal, the
    // second must ask for the learned level at once.
    for nth in 1..=2 {
        let answer = client
            .describe(ten_seconds(), &CancellationToken::new())
            .await
            .expect("a video request must not be refused for its thinking level");
        println!("{model} request {nth}: {}", answer.text.trim());
        assert!(
            !answer.text.trim().is_empty(),
            "{model}: an empty description"
        );
    }
    let learned = client.learned_level();
    println!("{model}: asked instead of \"minimal\": {learned:?}");
    assert!(
        learned.is_some(),
        "{model} was declared to refuse \"minimal\", yet nothing was refused — the recovery did not run"
    );
}

/// The control arm: a model that does take `minimal` —
/// `MINDFORK_LIVE_MINIMAL_LEVEL_MODEL` (`gemini-3.5-flash`) — is asked once, at
/// the level it was always asked at.
#[tokio::test]
#[ignore = "requires MINDFORK_GEMINI_KEY + MINDFORK_LIVE_MINIMAL_LEVEL_MODEL (a Gemini 3.x model that takes thinkingLevel \"minimal\")"]
async fn a_video_on_a_model_with_the_minimal_level_is_asked_as_before() {
    let Some((model, client)) = client_for("MINDFORK_LIVE_MINIMAL_LEVEL_MODEL") else {
        return;
    };
    let answer = client
        .describe(ten_seconds(), &CancellationToken::new())
        .await
        .expect("the video request is accepted");
    println!("{model}: {}", answer.text.trim());
    assert!(
        !answer.text.trim().is_empty(),
        "{model}: an empty description"
    );
    assert_eq!(
        client.learned_level(),
        None,
        "{model} was declared to take \"minimal\": nothing should have been learned"
    );
}

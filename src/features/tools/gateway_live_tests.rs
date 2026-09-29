//! Live smokes of `youtube_watch` through the OpenRouter gateway — stage 4 of
//! docs/research/openrouter-mode.md (§7), `#[ignore]`. Part of the mode's live
//! gate: `cargo test gateway_live -- --ignored --nocapture --test-threads=1`.
//!
//! The tool as the agentic loop calls it, on the gateway's client as the
//! registry builds it: the link a person pastes, the video's length read from
//! YouTube, the ceiling, the segment in words, the transcript. No chat engine
//! is needed — the tool is called without a model to decide that it is.
//!
//! Declared by `MINDFORK_OPENROUTER_KEY`, like the client's own smokes, whose
//! video and model these use.

use uuid::Uuid;

use super::youtube::YoutubeWatch;
use super::{Tool, ToolContext};
use crate::shared::i18n::{Lang, locale};
use crate::shared::video::gateway_live_tests::{A_LINE_OF_IT, FRESH_VIDEO, config, key};

fn ctx() -> (tempfile::TempDir, ToolContext) {
    let (dir, _s, ctx) = super::testkit::ctx_with_storage(Uuid::new_v4());
    (dir, ctx)
}

/// The tool on the gateway's client, under a ceiling of this many minutes.
fn tool(key: &str, max_minutes: u32) -> YoutubeWatch {
    let client = crate::shared::video::client(config(key));
    assert!(!client.reads_segments(), "the gateway's client");
    YoutubeWatch::new(Some(client), max_minutes)
}

async fn called(tool: &YoutubeWatch, ctx: &ToolContext, args: serde_json::Value) -> String {
    let started = std::time::Instant::now();
    let out = tool.invoke(ctx, args).await.expect("the tool answers");
    eprintln!("   in {:.1?}:\n{}", started.elapsed(), out.result);
    out.result
}

/// The seconds of every line that opens with a `[m:ss]` stamp.
fn stamps(text: &str) -> Vec<u32> {
    let secs = |line: &str| {
        let stamp = line.trim().strip_prefix('[')?.split(']').next()?;
        let parts: Option<Vec<u32>> = stamp.split(':').map(|p| p.trim().parse().ok()).collect();
        Some(parts?.iter().fold(0, |acc, n| acc * 60 + n))
    };
    text.lines().filter_map(secs).collect()
}

/// The link as a person pastes it — with the second they stopped at and what
/// the share button adds — is watched: what reaches the gateway is the
/// video's id and nothing else, where one more parameter is a page read at
/// ninety times the price. The answer holds the header YouTube gave and a line
/// of what is said in the video.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY and network"]
async fn a_pasted_link_is_watched_through_the_gateway_live() {
    let Some(key) = key() else { return };
    let (_d, ctx) = ctx();
    let pasted = format!("{FRESH_VIDEO}&t=20s&feature=share");
    let said = called(
        &tool(&key, 30),
        &ctx,
        serde_json::json!({"url": pasted, "transcript": true}),
    )
    .await;
    assert!(said.to_lowercase().contains(A_LINE_OF_IT), "{said}");
    assert!(said.contains("1:07"), "the length YouTube gave: {said}");
    assert!(!stamps(&said).is_empty(), "a transcript with its stamps");
    let whole = locale(Lang::default()).tf(
        "tool.youtube_watch.result.whole_video_read",
        &[("range", "")],
    );
    assert!(
        !said.contains(&whole[..whole.len().min(24)]),
        "no segment was asked for, so there is nothing to say about one: {said}"
    );
}

/// A segment is named in words: the transcript is of that part — its stamps
/// lie in it, counted from the video's start — and the result says that the
/// whole video was read and charged.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY and network"]
async fn a_segment_named_in_words_is_what_the_answer_is_about_live() {
    let Some(key) = key() else { return };
    let (_d, ctx) = ctx();
    let said = called(
        &tool(&key, 30),
        &ctx,
        serde_json::json!({"url": FRESH_VIDEO, "start": 20, "end": 40, "transcript": true}),
    )
    .await;
    let note = locale(Lang::default()).tf(
        "tool.youtube_watch.result.whole_video_read",
        &[("range", "0:20-0:40")],
    );
    assert!(said.contains(&note), "{said}");
    let stamps = stamps(&said);
    assert!(!stamps.is_empty(), "a transcript with its stamps: {said}");
    assert!(
        stamps.iter().all(|s| (15..=45).contains(s)),
        "every line is of the part asked for: {stamps:?}"
    );
    assert!(
        !said.to_lowercase().contains(A_LINE_OF_IT),
        "the video's first words are outside the part: {said}"
    );
}

/// The ceiling measures the whole video, since the whole video is what is
/// charged: a video of 67 seconds under a ceiling of a minute is refused though
/// the part asked for is thirty seconds — and nothing is spent on it.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY and network"]
async fn a_video_over_the_ceiling_is_refused_whatever_part_is_asked_live() {
    let Some(key) = key() else { return };
    let (_d, ctx) = ctx();
    let said = called(
        &tool(&key, 1),
        &ctx,
        serde_json::json!({"url": FRESH_VIDEO, "start": 0, "end": 30}),
    )
    .await;
    let refusal = locale(Lang::default()).tf(
        "tool.youtube_watch.result.too_long_whole",
        &[("length", "1:07"), ("cap", "1:00")],
    );
    assert_eq!(said, refusal);
}

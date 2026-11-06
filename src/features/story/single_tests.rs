//! Tests of B0, one agent in one session (docs/fabula.md §6, T21): through
//! the engine adapter over a scripted engine, so every turn is in the call
//! log with its tokens, as a live session's is.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::{
    CRITIQUE_ASK, Ended, SingleConfig, Target, draft_ask, revise_ask, system, would_overflow, write,
};
use crate::entities::sampling::SamplingConfig;
use crate::entities::story::{VersionOrigin, fixture_canon};
use crate::features::story::engine::{EngineAdapter, SharedLog};
use crate::features::story::scene::tests::fresh_story;
use crate::shared::api::contract::{ChatChunk, FinishReason, TokenUsage};
use crate::shared::api::mock::MockBackend;
use crate::shared::storage::story::Spend;

/// A reply of 120 prompt and 30 reply tokens.
fn reply(text: &str) -> Vec<ChatChunk> {
    vec![
        ChatChunk::Text(text.to_owned()),
        ChatChunk::Usage(TokenUsage {
            prompt_tokens: 120,
            completion_tokens: 30,
            ..TokenUsage::default()
        }),
        ChatChunk::Finished(FinishReason::Stop),
    ]
}

fn failure() -> Vec<ChatChunk> {
    vec![
        ChatChunk::Error {
            message: "the endpoint is gone".to_owned(),
            transient: false,
        },
        ChatChunk::Finished(FinishReason::Error),
    ]
}

fn port(scripts: Vec<Vec<ChatChunk>>, log: &SharedLog) -> EngineAdapter {
    EngineAdapter::sharing(
        Arc::new(MockBackend::sequence(scripts)),
        log.clone(),
        "mock-model",
        SamplingConfig::default(),
        CancellationToken::new(),
    )
}

fn turns(n: usize) -> Vec<Vec<ChatChunk>> {
    (1..=n)
        .map(|i| {
            if i % 2 == 1 {
                reply(&format!("## Version {}\n\nText {i}.", i.div_ceil(2)))
            } else {
                reply(&format!("Critique {}.", i / 2))
            }
        })
        .collect()
}

fn config(tokens: u64) -> SingleConfig {
    SingleConfig {
        target: Target { usd: None, tokens },
        ..SingleConfig::default()
    }
}

#[tokio::test]
async fn a_session_writes_reads_and_writes_again_until_it_has_spent_its_target() {
    let (mut story, log) = fresh_story().await;
    let log: SharedLog = Arc::new(tokio::sync::Mutex::new(log));
    // 150 tokens a turn: the draft and two pairs reach 750, past 700.
    let port = port(turns(9), &log);
    let written = write(&story.db, &mut story.author, &log, &port, &config(700))
        .await
        .unwrap();
    assert_eq!(written.ended, Ended::Spent);
    assert_eq!(written.versions, 3);
    assert_eq!(written.spent.tokens(), 750);
    let versions = story.db.chapter_versions(1).await.unwrap();
    let origins: Vec<VersionOrigin> = versions.iter().map(|v| v.origin).collect();
    assert_eq!(
        origins,
        [
            VersionOrigin::Draft,
            VersionOrigin::Revision,
            VersionOrigin::Revision
        ]
    );
    assert_eq!(versions[2].text, "## Version 3\n\nText 5.");
    // Each turn sees the whole session before it, in order.
    let run = story.db.runs().await.unwrap().pop().unwrap();
    let last = story.db.call(&run, "b0/t5", 1).await.unwrap().unwrap();
    let messages = last.call.request["messages"].as_array().unwrap().clone();
    let said: Vec<&str> = messages
        .iter()
        .map(|m| m["content"].as_str().unwrap())
        .collect();
    let canon = fixture_canon();
    let words = SingleConfig::default().words;
    assert_eq!(
        said,
        [
            draft_ask(&canon, words).as_str(),
            "## Version 1\n\nText 1.",
            CRITIQUE_ASK,
            "Critique 1.",
            revise_ask(words).as_str(),
            "## Version 2\n\nText 3.",
            CRITIQUE_ASK,
            "Critique 2.",
            revise_ask(words).as_str(),
        ]
    );
    assert_eq!(last.call.request["system"], system(&canon));
    assert_eq!(last.call.role, "author");
}

#[tokio::test]
async fn a_session_cut_by_a_failure_goes_on_from_its_call_log() {
    let (mut story, log) = fresh_story().await;
    let log: SharedLog = Arc::new(tokio::sync::Mutex::new(log));
    let mut first = turns(2);
    first.push(failure());
    let broken = port(first, &log);
    let written = write(&story.db, &mut story.author, &log, &broken, &config(700))
        .await
        .unwrap();
    assert!(
        matches!(&written.ended, Ended::Failed(at, _) if at == "b0/t3"),
        "{:?}",
        written.ended
    );
    assert_eq!(written.versions, 1);
    drop(broken);

    // The next run asks turn 3 again, with turns 1 and 2 from the log.
    let rest: Vec<Vec<ChatChunk>> = turns(9).into_iter().skip(2).collect();
    let resumed = port(rest, &log);
    let written = write(&story.db, &mut story.author, &log, &resumed, &config(700))
        .await
        .unwrap();
    assert_eq!(written.ended, Ended::Spent);
    assert_eq!(written.versions, 3);
    let run = story.db.runs().await.unwrap().pop().unwrap();
    let again = story.db.call(&run, "b0/t3", 1).await.unwrap().unwrap();
    let messages = again.call.request["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 5);
    assert_eq!(messages[1]["content"], "## Version 1\n\nText 1.");
    assert_eq!(messages[3]["content"], "Critique 1.");
    assert_eq!(story.db.chapter_versions(1).await.unwrap().len(), 3);
}

#[tokio::test]
async fn a_version_that_breaks_off_stays_in_the_session_and_out_of_the_story() {
    let (mut story, log) = fresh_story().await;
    let log: SharedLog = Arc::new(tokio::sync::Mutex::new(log));
    let cut = "## Version 2\n\nText 3, and then she did not tu";
    let mut first = turns(2);
    first.push(reply(cut));
    first.push(failure());
    let broken = port(first, &log);
    let written = write(&story.db, &mut story.author, &log, &broken, &config(700))
        .await
        .unwrap();
    assert!(
        matches!(&written.ended, Ended::Failed(at, _) if at == "b0/t4"),
        "{:?}",
        written.ended
    );
    assert_eq!(written.versions, 1);
    drop(broken);

    // Resumed: the critique reads the broken version, the next revision is
    // kept, and the broken one is never stored, on this run or the next.
    let rest: Vec<Vec<ChatChunk>> = turns(9).into_iter().skip(3).collect();
    let resumed = port(rest, &log);
    let written = write(&story.db, &mut story.author, &log, &resumed, &config(700))
        .await
        .unwrap();
    assert_eq!(written.ended, Ended::Spent);
    assert_eq!(written.versions, 2);
    let texts: Vec<String> = story
        .db
        .chapter_versions(1)
        .await
        .unwrap()
        .into_iter()
        .map(|v| v.text)
        .collect();
    assert_eq!(
        texts,
        ["## Version 1\n\nText 1.", "## Version 3\n\nText 5."]
    );
    let run = story.db.runs().await.unwrap().pop().unwrap();
    let critique = story.db.call(&run, "b0/t4", 1).await.unwrap().unwrap();
    let messages = critique.call.request["messages"].as_array().unwrap();
    assert_eq!(messages[5]["content"], cut);
}

#[tokio::test]
async fn the_window_stops_a_session_before_a_turn_that_would_not_fit() {
    let (mut story, log) = fresh_story().await;
    let log: SharedLog = Arc::new(tokio::sync::Mutex::new(log));
    let port = port(turns(5), &log);
    let mut tight = config(u64::MAX);
    tight.window = 3_000;
    let written = write(&story.db, &mut story.author, &log, &port, &tight)
        .await
        .unwrap();
    assert_eq!(written.ended, Ended::Window);
    assert_eq!(written.versions, 1);
    assert!(would_overflow(3_000, 150, "short"));
    assert!(!would_overflow(131_072, 150, "short"));
    // A long version counts twice: its reading and its rewriting.
    assert!(would_overflow(30_000, 1_000, &"word ".repeat(12_000)));
}

#[test]
fn the_target_is_dollars_where_both_sides_have_them_and_tokens_otherwise() {
    let spent = |tokens: u64, usd: Option<f64>| Spend {
        calls: 1,
        input_tokens: tokens,
        output_tokens: 0,
        cost_usd: usd,
    };
    let both = Target {
        usd: Some(1.0),
        tokens: 1_000,
    };
    assert!(
        !both.reached(&spent(5_000, Some(0.5))),
        "the dollars decide"
    );
    assert!(both.reached(&spent(10, Some(1.0))));
    assert!(
        both.reached(&spent(1_000, None)),
        "no dollars spent: tokens"
    );
    let tokens = Target {
        usd: None,
        tokens: 1_000,
    };
    assert!(!tokens.reached(&spent(999, Some(9.0))));
}

#[test]
fn the_writer_is_given_the_canon_and_the_prose_rules_and_no_record() {
    let canon = fixture_canon();
    let system = system(&canon);
    assert!(system.starts_with("You are a writer."));
    assert!(system.contains("The cast, with what only you and each of them know"));
    assert!(system.contains("She took a bribe from the smuggler."));
    assert!(!system.contains("[SECRET-"), "the tokens stay out");
    assert!(!system.contains("World Master"));
    let draft = draft_ask(&canon, (1500, 3000));
    assert!(draft.contains("The shipment's secret reaches the wrong person."));
    assert!(draft.contains("between 1500 and 3000 words"));
    assert!(draft.contains("Invent what happens"));
    assert!(draft.contains("The canon's keys are not the story's words"));
    assert!(!draft.contains("record"), "{draft}");
    assert!(revise_ask((1500, 3000)).contains("between 1500 and 3000 words"));
}

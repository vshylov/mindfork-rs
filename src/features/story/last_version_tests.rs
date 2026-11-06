//! The last version checked (docs/fabula.md §4, *The last version checked*):
//! the last revision reviewed and triaged and never revised, the text before
//! it going on when the Author accepts more high-severity notes on it (F1),
//! a resume of that at any call, and the engine's own count of the words
//! (F2).

use std::sync::Arc;

use futures_util::StreamExt;
use serde_json::json;
use tokio::sync::Notify;

use super::tests::{Answer, Open, Reply, Script, config, paused_at, story};
use super::{ChapterConfig, length_note};
use crate::entities::story::{NoteKind, NoteSeverity, QuestionBody, VersionOrigin};
use crate::features::story::port::{PortError, Prompt};

fn checkpoint(version: u32) -> QuestionBody {
    QuestionBody::Chapter {
        chapter: 1,
        version,
    }
}

/// One revision: the draft's accepted note asks for it, and it is the last.
fn one_revision() -> ChapterConfig {
    ChapterConfig {
        max_revisions: 1,
        ..config()
    }
}

/// Every label the triage shows, accepted.
fn accept_all(prompt: &Prompt) -> Reply {
    let decisions: Vec<serde_json::Value> = prompt
        .user
        .lines()
        .filter_map(|line| line.strip_prefix("- n"))
        .filter_map(|rest| rest.split(' ').next())
        .map(|n| json!({"note": format!("n{n}"), "critique": "Right.", "decision": "accepted"}))
        .collect();
    Reply::Json(json!({ "decisions": decisions }))
}

/// The story, but the revision is worse than the draft: the Reader raises two
/// high notes on it, and the Author accepts both.
fn worse_last(address: &str, prompt: &Prompt) -> Result<Reply, PortError> {
    if address == "ch1/v2/reader" {
        return Ok(Reply::Json(json!({
            "confusions": [
                {"anchor": "Revised", "note": "Revised what?", "severity": "high"},
                {"anchor": "at ch1", "note": "An address is not a word.", "severity": "high"},
            ],
            "slow_passages": [],
            "predictions": [],
            "best_moment": "None.",
        })));
    }
    if address == "ch1/v2/triage" {
        return Ok(accept_all(prompt));
    }
    story(address, prompt)
}

/// The story, but the Author accepts the Reader's one high note on the
/// revision as on the draft: as bad, no worse.
fn as_bad_last(address: &str, prompt: &Prompt) -> Result<Reply, PortError> {
    if address == "ch1/v2/triage" {
        return story("ch1/v1/triage", prompt);
    }
    story(address, prompt)
}

#[tokio::test]
async fn a_last_revision_worse_than_the_draft_gives_way_to_the_drafts_text() {
    let mut open = Open::approved().await;
    let port = Script::new(Arc::new(worse_last));
    let outcome = open.advance(&port, &one_revision()).await;
    assert_eq!(paused_at(&outcome), &checkpoint(3));
    let calls = port.addresses();
    for reviewed in [
        "ch1/v2/reader",
        "ch1/v2/continuity",
        "ch1/v2/staging",
        "ch1/v2/triage",
    ] {
        assert!(calls.iter().any(|a| a == reviewed), "{reviewed}: {calls:?}");
    }
    assert!(
        !calls.iter().any(|a| a.starts_with("ch1/v3/revise")),
        "the last revision is never revised, and the draft's text is put back by no call"
    );
    let db = &open.db;
    let versions = db.chapter_versions(1).await.unwrap();
    assert_eq!(
        versions.iter().map(|v| v.origin).collect::<Vec<_>>(),
        [
            VersionOrigin::Draft,
            VersionOrigin::Revision,
            VersionOrigin::Revision
        ]
    );
    assert_eq!(versions[2].text, versions[0].text);
    assert!(db.notes(&versions[2].id).await.unwrap().is_empty());
    assert!(db.summary(&versions[2].id).await.unwrap().is_some());
    assert!(db.summary(&versions[1].id).await.unwrap().is_none());
    let steps = db.all_steps().await.unwrap();
    assert!(steps.contains_key("ch1/v3/restore"));
    // A second run asks nothing: the question waits.
    let again = Script::new(Arc::new(worse_last));
    assert_eq!(open.advance(&again, &one_revision()).await, outcome);
    assert!(again.addresses().is_empty());
}

#[tokio::test]
async fn a_last_revision_no_worse_than_the_one_before_goes_on_reviewed() {
    let mut open = Open::approved().await;
    let port = Script::new(Arc::new(as_bad_last));
    let outcome = open.advance(&port, &one_revision()).await;
    assert_eq!(paused_at(&outcome), &checkpoint(2));
    let db = &open.db;
    let versions = db.chapter_versions(1).await.unwrap();
    assert_eq!(versions.len(), 2);
    assert!(!db.notes(&versions[1].id).await.unwrap().is_empty());
    assert!(!db.decisions(&versions[1].id).await.unwrap().is_empty());
    assert!(
        !port.addresses().iter().any(|a| a.starts_with("ch1/v3/")),
        "one accepted high note on each: the revision stands"
    );
}

#[tokio::test]
async fn a_draft_that_may_not_be_revised_is_not_reviewed() {
    let mut open = Open::approved().await;
    let port = Script::new(Arc::new(story));
    let cfg = ChapterConfig {
        max_revisions: 0,
        ..config()
    };
    let outcome = open.advance(&port, &cfg).await;
    assert_eq!(paused_at(&outcome), &checkpoint(1));
    let calls = port.addresses();
    assert!(
        !calls.iter().any(|a| a.starts_with("ch1/v1/reader")),
        "{calls:?}"
    );
    assert!(!calls.iter().any(|a| a.starts_with("ch1/v1/triage")));
}

#[tokio::test]
async fn a_run_killed_at_any_call_of_the_text_resumes_to_the_drafts_text() {
    let answer: Arc<Answer> = Arc::new(worse_last);
    let (total, from) = {
        let mut open = Open::approved().await;
        let port = Script::new(answer.clone());
        open.advance(&port, &one_revision()).await;
        let calls = port.addresses();
        let from = calls.iter().position(|a| a.starts_with("ch1/v1/")).unwrap() + 1;
        (calls.len(), from)
    };
    futures_util::stream::iter(from..=total)
        .map(|cut| {
            let answer = answer.clone();
            async move {
                let mut open = Open::approved().await;
                let blown = Arc::new(Notify::new());
                let fused = Script::fused(answer.clone(), cut, blown.clone());
                let cfg = one_revision();
                tokio::select! {
                    outcome = open.advance(&fused, &cfg) => {
                        panic!("cut {cut} ran through: {outcome:?}")
                    }
                    () = blown.notified() => {}
                }
                let port = Script::new(answer);
                let outcome = open.advance(&port, &one_revision()).await;
                assert_eq!(paused_at(&outcome), &checkpoint(3), "cut {cut}");
                let versions = open.db.chapter_versions(1).await.unwrap();
                assert_eq!(versions.len(), 3, "cut {cut}: the text put back once");
                assert_eq!(versions[2].text, versions[0].text, "cut {cut}");
                let questions = open.db.questions().await.unwrap();
                assert_eq!(questions.len(), 2, "cut {cut}: P1, then P2 once");
            }
        })
        .buffer_unordered(16)
        .collect::<Vec<()>>()
        .await;
}

#[test]
fn a_version_outside_the_words_asked_is_the_engines_high_note() {
    let words = |n: usize| vec!["слово"; n].join(" ");
    assert_eq!(length_note(&words(1200), (1200, 2500)), None);
    assert_eq!(length_note(&words(2500), (1200, 2500)), None);
    let short = length_note(&format!("## Title\n\n— {}", words(1076)), (1200, 2500)).unwrap();
    assert_eq!(short.kind, NoteKind::Length);
    assert_eq!(short.severity, NoteSeverity::High);
    assert!(short.anchor.is_none() && short.evidence.is_empty());
    // The heading's `##` and the dialogue's dash are no words.
    assert!(
        short
            .body
            .starts_with("The chapter has 1077 words, under the 1200 to 2500 asked"),
        "{}",
        short.body
    );
    assert!(short.body.contains("rather than cutting another"));
    let long = length_note(&words(2501), (1200, 2500)).unwrap();
    assert!(
        long.body
            .starts_with("The chapter has 2501 words, over the 1200 to 2500 asked")
    );
    assert!(long.body.contains("keep each scene's turn"));
}

#[tokio::test]
async fn the_review_counts_the_words_beside_the_checkers_notes() {
    let mut open = Open::approved().await;
    let port = Script::new(Arc::new(story));
    let cfg = ChapterConfig {
        words: (1200, 2500),
        ..config()
    };
    open.advance(&port, &cfg).await;
    let versions = open.db.chapter_versions(1).await.unwrap();
    let notes = open.db.notes(&versions[0].id).await.unwrap();
    let length: Vec<_> = notes
        .iter()
        .filter(|n| n.note.kind == NoteKind::Length)
        .collect();
    assert_eq!(length.len(), 1, "{notes:?}");
    assert!(
        length[0]
            .note
            .body
            .starts_with("The chapter has 4 words, under the 1200 to 2500 asked")
    );
    // The reviser is told the count of the text it revises.
    let revise = port.prompt_at("ch1/v2/revise");
    assert!(revise.user.contains("The chapter as it stands, 4 words:"));
}

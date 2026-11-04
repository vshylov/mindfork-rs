//! The staging pass in the chapter's loop, on a scripted port (docs/fabula.md
//! §4, PR 12): it reads each version's text alone, every chapter; its notes
//! ride with the Continuity checker's, so the triage reads them as the
//! checker's; a later chapter's pass reads the published chapters'
//! summaries; and a run killed at it asks the checker's batch again and
//! nothing the run had stored.

use std::sync::Arc;

use serde_json::json;

use super::tests::{Open, Reply, Script, config, paused_at, story, two_chapters};
use super::{Outcome, Stop};
use crate::entities::story::{ChapterId, Decision, NoteKind, NoteSeverity, QuestionBody, Reviewer};
use crate::features::story::port::{PortError, Prompt};

/// The pass's findings on the first draft: a kin a reader cannot place, and
/// a reply that opens on thanks.
fn staged(address: &str, prompt: &Prompt) -> Result<Reply, PortError> {
    if address == "ch1/v1/staging" {
        return Ok(Reply::Json(json!({"findings": [
            {
                "anchor": " Mara waits ",
                "note": "Whose daughter is Mara? The text names two mothers.",
                "kind": "referent",
                "severity": "high",
            },
            {
                "anchor": "Thank you, Silas",
                "note": "Opens on thanks the moment does not call for.",
                "kind": "thanks",
                "severity": "low",
            },
        ]})));
    }
    story(address, prompt)
}

#[tokio::test]
async fn the_staging_pass_reads_the_text_alone_and_its_notes_ride_with_the_checkers() {
    let mut open = Open::approved().await;
    let port = Script::new(Arc::new(staged));
    let outcome = open.advance(&port, &config()).await;
    assert_eq!(
        paused_at(&outcome),
        &QuestionBody::Chapter {
            chapter: 1,
            version: 2
        }
    );
    let canon = open.db.canon().await.unwrap().unwrap();
    for (at, text) in [
        ("ch1/v1/staging", "Mara waits."),
        ("ch1/v2/staging", "Revised at ch1/v2/revise."),
    ] {
        let prompt = port.prompt_at(at);
        assert!(prompt.user.contains(text), "{at} reads its version");
        for part in [&prompt.system, &prompt.user] {
            for banned in [
                canon.characters[0].secrets[0].text.as_str(),
                canon.arc.premise.as_str(),
                "ch1/sc1/",
                "Author of a story",
            ] {
                assert!(!part.contains(banned), "{at}: {banned}");
            }
        }
    }
    let notes = open.db.notes(&ChapterId::of(1, 1)).await.unwrap();
    let staged: Vec<_> = notes
        .iter()
        .filter(|n| matches!(n.note.kind, NoteKind::Referent | NoteKind::Thanks))
        .collect();
    assert_eq!(staged.len(), 2);
    assert!(staged.iter().all(|n| n.reviewer == Reviewer::Continuity));
    assert_eq!(staged[0].note.kind, NoteKind::Referent);
    assert_eq!(staged[0].note.severity, NoteSeverity::High);
    assert_eq!(staged[0].note.anchor.as_deref(), Some("Mara waits"));
    assert!(
        notes.iter().any(|n| n.note.kind == NoteKind::Contradiction),
        "the checker's own note is in the same batch"
    );
    let triage = port.prompt_at("ch1/v1/triage");
    assert!(triage.user.contains(
        "· continuity · referent, high: \"Mara waits\": Whose daughter is Mara? The text names \
         two mothers."
    ));
    assert!(triage.user.contains("· continuity · thanks, low:"));
}

#[tokio::test]
async fn a_run_killed_at_the_staging_pass_asks_the_checkers_batch_again_and_nothing_twice() {
    let mut open = Open::approved().await;
    let cut = |address: &str, prompt: &Prompt| -> Result<Reply, PortError> {
        if address == "ch1/v1/staging" {
            return Err(PortError::Cancelled {
                address: address.to_owned(),
            });
        }
        story(address, prompt)
    };
    let outcome = open.advance(&Script::new(Arc::new(cut)), &config()).await;
    assert_eq!(
        outcome,
        Outcome::Stopped {
            step: "ch1/v1/staging".to_owned(),
            why: Stop::Cancelled,
        }
    );
    let first = open.db.notes(&ChapterId::of(1, 1)).await.unwrap();
    assert!(
        first.iter().all(|n| n.reviewer == Reviewer::Reader),
        "the checker's batch is not written without the pass"
    );
    let port = Script::new(Arc::new(story));
    let outcome = open.advance(&port, &config()).await;
    assert!(matches!(paused_at(&outcome), QuestionBody::Chapter { .. }));
    let again = port.addresses();
    assert_eq!(
        &again[..3],
        ["ch1/v1/continuity", "ch1/v1/staging", "ch1/v1/triage"],
        "{again:?}"
    );
    assert!(!again.iter().any(|a| a == "ch1/v1/reader"));
    let notes = open.db.notes(&ChapterId::of(1, 1)).await.unwrap();
    assert_eq!(
        notes
            .iter()
            .filter(|n| n.reviewer == Reviewer::Reader)
            .count(),
        first.len(),
        "the Reader's notes are written once"
    );
}

#[tokio::test]
async fn the_staging_pass_of_a_later_chapter_reads_the_published_summaries() {
    let mut open = two_chapters().await;
    let port = Script::new(Arc::new(story));
    open.advance(&port, &config()).await;
    assert!(
        port.prompt_at("ch1/v1/staging")
            .user
            .starts_with("This is the story's first chapter.")
    );
    open.answer(Decision::Approve, "").await;
    open.advance(&port, &config()).await;
    let second = port.prompt_at("ch2/v1/staging");
    assert!(second.user.starts_with(
        "The story so far, a summary of each published chapter:\n- Chapter 1: Mara keeps the \
         harbour shut.\n\nThe new chapter:\n\n## The storm"
    ));
}

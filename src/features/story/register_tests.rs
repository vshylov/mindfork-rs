//! The story's register on a scripted port (docs/fabula.md §4, PR 11; §6,
//! T16): the Canon keeper reads a version's text alone; the next chapter's
//! prompts and its Continuity checker read what it noted; a thread the last
//! chapter leaves unpaid is the Author's to pay off or keep, with its reason;
//! a summary the text does not bear out is written again; and a story begun
//! before the register is given one on the way.

use std::sync::Arc;

use serde_json::json;

use super::checker_notes;
use super::tests::{Open, Reply, Script, config, paused_at, story};
use crate::entities::story::NoteSeverity;
use crate::entities::story::{
    ChapterArc, ChapterId, Decision, Fact, NoteKind, QuestionBody, Register, Thread, fixture_canon,
};
use crate::features::story::author::{ContinuityReply, Finding, FindingKind};
use crate::features::story::keeper::{LeftOpen, left_open};
use crate::features::story::port::{PortError, Prompt};
use crate::features::story::schema::CanonKeys;

/// What the scripted keeper notes in chapter 1, as the next prompts show it.
const THREAD: &str = "- `ch1.thread1` (chapter 1): What is Mara waiting for? (\"Mara waits\")";
const FACT: &str = "- `ch1.fact1` (chapter 1, stated by narration): The storm has blown for three \
                    days. (\"the storm\")";

/// A story whose arc has two chapters, its canon approved.
async fn two_chapters() -> Open {
    let mut open = Open::open_only().await;
    let mut canon = fixture_canon();
    canon.arc.chapters.push(ChapterArc {
        goal: "Ines decides whom to trust.".into(),
    });
    open.author.import_canon(&canon).await.unwrap();
    let port = Script::new(Arc::new(story));
    let outcome = open.advance(&port, &config()).await;
    assert_eq!(paused_at(&outcome), &QuestionBody::Canon);
    open.answer(Decision::Approve, "").await;
    open
}

fn checkpoint(chapter: u32, version: u32) -> QuestionBody {
    QuestionBody::Chapter { chapter, version }
}

#[tokio::test]
async fn the_keeper_reads_the_text_alone_and_the_next_chapter_reads_its_register() {
    let mut open = two_chapters().await;
    let port = Script::new(Arc::new(story));
    let outcome = open.advance(&port, &config()).await;
    assert_eq!(paused_at(&outcome), &checkpoint(1, 2));
    let canon = open.db.canon().await.unwrap().unwrap();
    let secret = canon.characters[0].secrets[0].text.clone();
    for at in ["ch1/v2/register", "ch1/v2/summary", "ch1/v2/summary-check"] {
        let prompt = port.prompt_at(at);
        assert!(prompt.user.contains("Revised at ch1/v2/revise."), "{at}");
        for banned in [
            secret.as_str(),
            canon.arc.premise.as_str(),
            "We wait out the storm.",
            "Author of a story",
        ] {
            assert!(
                !prompt.system.contains(banned) && !prompt.user.contains(banned),
                "{at} shows {banned}"
            );
        }
    }
    assert!(
        !port.schema_at("ch1/v2/register").contains("\"paid\""),
        "nothing open before chapter 1"
    );
    assert_eq!(
        open.db.register(&ChapterId::of(1, 2)).await.unwrap(),
        Some(Register {
            opened: vec![Thread {
                id: "ch1.thread1".into(),
                question: "What is Mara waiting for?".into(),
                quote: "Mara waits".into(),
            }],
            paid: Vec::new(),
            facts: vec![Fact {
                id: "ch1.fact1".into(),
                fact: "The storm has blown for three days.".into(),
                stated_by: "narration".into(),
                quote: "the storm".into(),
            }],
        })
    );
    assert!(
        !port
            .schema_at("ch1/v1/register-check")
            .contains("open_thread"),
        "not the last chapter"
    );

    open.answer(Decision::Approve, "").await;
    let outcome = open.advance(&port, &config()).await;
    assert_eq!(paused_at(&outcome), &checkpoint(2, 2));
    for at in [
        "ch2/outline",
        "ch2/sc1/plan",
        "ch2/v1/write",
        "ch2/v2/revise",
    ] {
        let prompt = port.prompt_at(at);
        assert!(prompt.user.contains(THREAD), "{at}");
        assert!(prompt.user.contains(FACT), "{at}");
        assert!(
            prompt
                .user
                .contains("This is the arc's last chapter: pay off each open thread"),
            "{at}"
        );
        assert!(prompt.system.contains("by arithmetic"), "{at}");
    }
    assert!(
        !port.prompt_at("ch2/v1/continuity").user.contains(THREAD),
        "the record's checker reads the record"
    );
    let checker = port.prompt_at("ch2/v1/register-check");
    assert!(checker.user.contains(THREAD) && checker.user.contains(FACT));
    assert!(checker.system.contains("by arithmetic"));
    assert!(checker.user.contains("This is the story's last chapter"));
    assert!(
        port.schema_at("ch2/v1/register-check")
            .contains("open_thread")
    );
    let register = port.prompt_at("ch2/v2/register");
    assert!(register.user.contains(THREAD));
    assert!(
        port.schema_at("ch2/v2/register")
            .contains("\"enum\":[\"ch1.thread1\"]")
    );
    assert!(
        port.prompt_at("ch2/v1/triage")
            .user
            .contains("Write each critique in Russian")
    );
    let v2 = open.db.chapter_versions(2).await.unwrap().pop().unwrap();
    assert_eq!(
        left_open(&open.db, 2, &v2).await.unwrap(),
        Vec::<LeftOpen>::new(),
        "chapter 2 paid the thread off"
    );
}

/// The lines of a triage prompt that show an open thread's note: `n2`.
fn open_thread_labels(prompt: &Prompt) -> Vec<String> {
    prompt
        .user
        .lines()
        .filter(|l| l.contains("· open_thread,"))
        .filter_map(|l| l.trim_start_matches("- ").split(' ').next())
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn a_thread_the_last_chapter_leaves_unpaid_is_the_authors_to_keep_with_its_reason() {
    let mut open = two_chapters().await;
    let keep = |address: &str, prompt: &Prompt| -> Result<Reply, PortError> {
        if !address.starts_with("ch2/") {
            return story(address, prompt);
        }
        if address.ends_with("/register") {
            return Ok(Reply::Json(json!({"paid": [], "opened": [], "facts": []})));
        }
        if address == "ch2/v1/register-check" {
            return Ok(Reply::Json(json!({"findings": [
                {
                    "anchor": " `ch1.thread1` ",
                    "evidence": [],
                    "note": "Nothing says what Mara waited for.",
                    "kind": "open_thread",
                    "severity": "high",
                },
                {
                    "anchor": "ch9.thread9",
                    "evidence": [],
                    "note": "No such thread.",
                    "kind": "open_thread",
                    "severity": "high",
                },
            ]})));
        }
        if address == "ch2/v1/triage" {
            let left = open_thread_labels(prompt);
            assert_eq!(
                left.len(),
                1,
                "the thread the story does not have is dropped"
            );
            let Reply::Json(mut reply) = story(address, prompt)? else {
                unreachable!()
            };
            for d in reply["decisions"].as_array_mut().unwrap() {
                if left.contains(&d["note"].as_str().unwrap().to_owned()) {
                    d["decision"] = json!("rejected");
                    d["critique"] = json!("Left for the reader to wonder.");
                }
            }
            return Ok(Reply::Json(reply));
        }
        story(address, prompt)
    };
    let port = Script::new(Arc::new(keep));
    open.advance(&port, &config()).await;
    open.answer(Decision::Approve, "").await;
    let outcome = open.advance(&port, &config()).await;
    assert_eq!(paused_at(&outcome), &checkpoint(2, 2));
    let notes = open.db.notes(&ChapterId::of(2, 1)).await.unwrap();
    let left: Vec<_> = notes
        .iter()
        .filter(|n| n.note.kind == NoteKind::OpenThread)
        .collect();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].note.anchor.as_deref(), Some("ch1.thread1"));
    let v2 = open.db.chapter_versions(2).await.unwrap().pop().unwrap();
    assert_eq!(
        left_open(&open.db, 2, &v2).await.unwrap(),
        [LeftOpen {
            question: "What is Mara waiting for?".into(),
            reason: Some("Left for the reader to wonder.".into()),
        }]
    );
}

#[test]
fn a_thread_left_open_is_noted_only_when_the_story_has_it() {
    let finding = |anchor: &str, kind| Finding {
        anchor: anchor.to_owned(),
        evidence: Vec::new(),
        note: "Unpaid.".to_owned(),
        kind,
        severity: NoteSeverity::High,
    };
    let reply = ContinuityReply {
        findings: vec![
            finding("`ch1.thread2`", FindingKind::OpenThread),
            finding("ch1.thread3", FindingKind::OpenThread),
            finding("her own hand", FindingKind::Contradiction),
        ],
    };
    let notes = checker_notes(reply, &[], &["ch1.thread2".to_owned()]);
    let anchors: Vec<_> = notes.iter().map(|n| n.anchor.as_deref()).collect();
    assert_eq!(anchors, [Some("ch1.thread2"), Some("her own hand")]);
    assert_eq!(notes[0].kind, NoteKind::OpenThread);
    let keys = CanonKeys::of(&fixture_canon());
    assert!(
        ContinuityReply::schema_for(&keys, true)
            .to_json()
            .contains("open_thread")
    );
    assert!(
        !ContinuityReply::schema_for(&keys, false)
            .to_json()
            .contains("open_thread")
    );
}

#[tokio::test]
async fn a_summary_the_text_does_not_bear_out_is_written_again() {
    let mut open = Open::approved().await;
    let doubted = |address: &str, prompt: &Prompt| -> Result<Reply, PortError> {
        match address.rsplit('/').next().unwrap() {
            "summary-check" => Ok(Reply::Json(json!({"faults": [
                {"sentence": " ", "text_says": "A blank is no fault."},
                {
                    "sentence": "Mara keeps the harbour shut.",
                    "text_says": "The text says only that Mara waits.",
                },
            ]}))),
            "summary-again" => Ok(Reply::Text("Mara waits out the storm.".into())),
            _ => story(address, prompt),
        }
    };
    let port = Script::new(Arc::new(doubted));
    let outcome = open.advance(&port, &config()).await;
    assert_eq!(paused_at(&outcome), &checkpoint(1, 2));
    let calls = port.addresses();
    assert_eq!(
        &calls[calls.len() - 4..],
        [
            "ch1/v2/register",
            "ch1/v2/summary",
            "ch1/v2/summary-check",
            "ch1/v2/summary-again"
        ]
    );
    let again = port.prompt_at("ch1/v2/summary-again");
    assert!(
        again
            .user
            .contains("A first summary of it:\n\nMara keeps the harbour shut.")
    );
    assert!(
        again
            .user
            .contains("- \"Mara keeps the harbour shut.\": The text says only that Mara waits.")
    );
    assert!(!again.user.contains("A blank is no fault."));
    assert_eq!(
        open.db
            .summary(&ChapterId::of(1, 2))
            .await
            .unwrap()
            .as_deref(),
        Some("Mara waits out the storm.")
    );
}

#[tokio::test]
async fn a_story_begun_before_the_register_is_given_one_on_the_way() {
    let mut open = two_chapters().await;
    let port = Script::new(Arc::new(story));
    open.advance(&port, &config()).await;
    open.answer(Decision::Approve, "").await;
    // An earlier build's file: no register table, so no register.
    let mut open = open
        .reopened(|story| {
            rusqlite::Connection::open(story)
                .unwrap()
                .execute_batch("DROP TABLE chapter_register;")
                .unwrap();
        })
        .await;
    assert_eq!(open.db.register(&ChapterId::of(1, 2)).await.unwrap(), None);
    let port = Script::new(Arc::new(story));
    let outcome = open.advance(&port, &config()).await;
    assert_eq!(paused_at(&outcome), &checkpoint(2, 2));
    let calls = port.addresses();
    assert_eq!(calls[..2], ["ch1/v2/register", "ch2/outline"]);
    assert!(
        port.prompt_at("ch1/v2/register")
            .user
            .contains("Revised at ch1/v2/revise."),
        "made from the accepted text"
    );
    assert!(port.prompt_at("ch2/outline").user.contains(THREAD));
    assert!(
        open.db
            .register(&ChapterId::of(1, 2))
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        calls
            .iter()
            .filter(|a| a.as_str() == "ch1/v2/register")
            .count(),
        1,
        "made once"
    );
}

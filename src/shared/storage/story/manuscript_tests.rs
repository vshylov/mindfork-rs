//! The manuscript's stores and the pauses (docs/fabula.md §4, PR 6): the
//! outline, the chapter's versions in their order, a review written whole
//! and once, the decisions bound to their version's notes, one question at a
//! time, an answer that carries its approval, the assumptions, every run's
//! steps, and a chapter's spend.

use serde_json::json;

use super::{
    AuthorWriter, CallStatus, CallUsage, Mechanism, NewCall, StepStatus, StoryDb, StoryDbError,
};
use crate::entities::story::{
    Assumption, ChapterOutline, Decision, EventId, NewNote, NoteDecision, NoteId, NoteKind,
    NoteSeverity, QuestionBody, VersionOrigin, fixture_canon,
};
use crate::entities::story::{ChapterId, Decided, QuestionId, Resolution, Reviewer, RunId};

async fn open() -> (tempfile::TempDir, StoryDb, AuthorWriter) {
    let dir = tempfile::tempdir().unwrap();
    let db = StoryDb::open(
        &dir.path().join("story.db"),
        &dir.path().join("story.calls.db"),
    )
    .await
    .unwrap();
    let mut author = db.author_writer().await.unwrap();
    author.import_canon(&fixture_canon()).await.unwrap();
    (dir, db, author)
}

fn note(kind: NoteKind, severity: NoteSeverity, body: &str) -> NewNote {
    NewNote {
        kind,
        severity,
        anchor: Some("the ledger lay open".into()),
        body: body.into(),
        evidence: vec![EventId::new("ch1/sc1/r0/k0/e3")],
    }
}

fn resolution(decision: Decision, notes: &str) -> Resolution {
    Resolution {
        decision,
        notes: notes.into(),
        evidence: json!({"messages": ["m7"]}),
    }
}

#[tokio::test]
async fn an_outline_is_written_once() {
    let (_dir, db, mut author) = open().await;
    assert_eq!(db.outline(1).await.unwrap(), None);
    let outline = ChapterOutline {
        goals: vec!["The secret reaches Ines.".into()],
    };
    author.add_outline(1, &outline).await.unwrap();
    assert_eq!(db.outline(1).await.unwrap(), Some(outline.clone()));
    let err = author.add_outline(1, &outline).await.unwrap_err();
    assert!(matches!(err, StoryDbError::Exists(_)), "{err}");
    assert_eq!(db.outline(2).await.unwrap(), None);
    let err = db.execute_raw("DELETE FROM outline").await.unwrap_err();
    assert!(err.to_string().contains("is append-only"), "{err}");
}

#[tokio::test]
async fn a_chapters_versions_keep_their_order() {
    let (_dir, db, mut author) = open().await;
    let err = author
        .add_chapter(1, VersionOrigin::Revision, "no draft yet")
        .await
        .unwrap_err();
    assert!(matches!(err, StoryDbError::OutOfOrder(_)), "{err}");
    let v1 = author
        .add_chapter(1, VersionOrigin::Draft, "## One\n\nDraft.")
        .await
        .unwrap();
    assert_eq!((v1.id.as_str(), v1.version), ("ch1/v1", 1));
    let err = author
        .add_chapter(1, VersionOrigin::Draft, "a second draft")
        .await
        .unwrap_err();
    assert!(matches!(err, StoryDbError::OutOfOrder(_)), "{err}");
    let v2 = author
        .add_chapter(1, VersionOrigin::Revision, "## One\n\nRevised.")
        .await
        .unwrap();
    let v3 = author
        .add_chapter(1, VersionOrigin::Human, "## One\n\nAs asked.")
        .await
        .unwrap();
    assert_eq!((v2.version, v3.version), (2, 3));
    let versions = db.chapter_versions(1).await.unwrap();
    assert_eq!(versions, vec![v1, v2, v3.clone()]);
    assert!(db.chapter_versions(2).await.unwrap().is_empty());

    author
        .add_summary(&v3.id, "Mara is found out.")
        .await
        .unwrap();
    assert_eq!(
        db.summary(&v3.id).await.unwrap().as_deref(),
        Some("Mara is found out.")
    );
    let err = author.add_summary(&v3.id, "again").await.unwrap_err();
    assert!(matches!(err, StoryDbError::Exists(_)), "{err}");
    let err = author
        .add_summary(&ChapterId::of(4, 1), "nothing")
        .await
        .unwrap_err();
    assert!(matches!(err, StoryDbError::Unknown(_)), "{err}");
    for sql in [
        "UPDATE chapter SET text_md = ''",
        "DELETE FROM chapter_summary",
    ] {
        let err = db.execute_raw(sql).await.unwrap_err();
        assert!(err.to_string().contains("is append-only"), "{sql}: {err}");
    }
}

#[tokio::test]
async fn a_review_is_written_whole_and_once_by_its_own_writer() {
    let (_dir, db, mut author) = open().await;
    let v1 = author
        .add_chapter(1, VersionOrigin::Draft, "text")
        .await
        .unwrap();
    let mut reader = db.review_writer(&v1.id, Reviewer::Reader).await.unwrap();
    let held = db.review_writer(&v1.id, Reviewer::Reader).await;
    assert!(matches!(held, Err(StoryDbError::WriterHeld(_))));
    let mut continuity = db
        .review_writer(&v1.id, Reviewer::Continuity)
        .await
        .unwrap();
    let ids = reader
        .write(&[
            note(NoteKind::Confusion, NoteSeverity::High, "Who speaks here?"),
            note(NoteKind::Prediction, NoteSeverity::Low, "Silas runs."),
        ])
        .await
        .unwrap();
    assert_eq!(
        ids.iter().map(NoteId::as_str).collect::<Vec<_>>(),
        ["ch1/v1/reader/n1", "ch1/v1/reader/n2"]
    );
    let err = reader.write(&[]).await.unwrap_err();
    assert!(matches!(err, StoryDbError::Exists(_)), "{err}");
    continuity
        .write(&[note(
            NoteKind::KnowledgeLeak,
            NoteSeverity::High,
            "Ines names the bribe before she learns it.",
        )])
        .await
        .unwrap();
    let notes = db.notes(&v1.id).await.unwrap();
    assert_eq!(notes.len(), 3);
    assert_eq!(notes[0].reviewer, Reviewer::Reader);
    assert_eq!(notes[2].note.kind, NoteKind::KnowledgeLeak);
    assert_eq!(
        notes[2].note.evidence,
        [EventId::new("ch1/sc1/r0/k0/e3")],
        "the evidence reads back"
    );
    drop(reader);
    // A dropped writer frees its store, and its review stays written once.
    let mut again = db.review_writer(&v1.id, Reviewer::Reader).await.unwrap();
    assert!(again.write(&[]).await.is_err());
    let unknown = db
        .review_writer(&ChapterId::of(1, 9), Reviewer::Reader)
        .await;
    assert!(matches!(unknown, Err(StoryDbError::Unknown(_))));
}

#[tokio::test]
async fn a_reviewers_empty_review_writes_nothing() {
    let (_dir, db, mut author) = open().await;
    let v1 = author
        .add_chapter(1, VersionOrigin::Draft, "text")
        .await
        .unwrap();
    let mut reader = db.review_writer(&v1.id, Reviewer::Reader).await.unwrap();
    assert!(reader.write(&[]).await.unwrap().is_empty());
    assert!(db.notes(&v1.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_decision_binds_to_its_versions_note_once() {
    let (_dir, db, mut author) = open().await;
    let v1 = author
        .add_chapter(1, VersionOrigin::Draft, "text")
        .await
        .unwrap();
    let v2 = author
        .add_chapter(1, VersionOrigin::Revision, "text 2")
        .await
        .unwrap();
    let ids = db
        .review_writer(&v1.id, Reviewer::Reader)
        .await
        .unwrap()
        .write(&[
            note(NoteKind::Confusion, NoteSeverity::High, "a"),
            note(NoteKind::SlowPassage, NoteSeverity::Low, "b"),
        ])
        .await
        .unwrap();
    let decided = |note: &NoteId, decision| Decided {
        note: note.clone(),
        decision,
        reason: "Because.".into(),
    };
    // Another version's note is refused, and nothing of the batch is kept.
    let err = author
        .record_decisions(&v2.id, &[decided(&ids[0], NoteDecision::Accepted)])
        .await
        .unwrap_err();
    assert!(matches!(err, StoryDbError::Unknown(_)), "{err}");
    let twice = [
        decided(&ids[0], NoteDecision::Accepted),
        decided(&ids[0], NoteDecision::Rejected),
    ];
    let err = author.record_decisions(&v1.id, &twice).await.unwrap_err();
    assert!(matches!(err, StoryDbError::Exists(_)), "{err}");
    assert!(db.decisions(&v1.id).await.unwrap().is_empty());

    let batch = [
        decided(&ids[0], NoteDecision::Accepted),
        decided(&ids[1], NoteDecision::Deferred),
    ];
    author.record_decisions(&v1.id, &batch).await.unwrap();
    assert_eq!(db.decisions(&v1.id).await.unwrap(), batch);
    let err = author
        .record_decisions(&v1.id, &[decided(&ids[1], NoteDecision::Rejected)])
        .await
        .unwrap_err();
    assert!(matches!(err, StoryDbError::Exists(_)), "{err}");
}

#[tokio::test]
async fn one_question_waits_at_a_time_and_an_approval_settles_the_canon() {
    let (_dir, db, mut author) = open().await;
    let mut log = db.run_log_writer().await.unwrap();
    let run = log.start_run(&json!({})).await.unwrap();
    assert!(!db.canon_approved().await.unwrap());
    let p1 = author
        .add_question(&run, "p1", &QuestionBody::Canon)
        .await
        .unwrap();
    assert_eq!(p1.id.as_str(), "q1");
    assert_eq!(db.pending_question().await.unwrap(), Some(p1.clone()));
    let err = author
        .add_question(&run, "p1", &QuestionBody::Canon)
        .await
        .unwrap_err();
    assert!(
        matches!(err, StoryDbError::QuestionPending(ref q) if q == &p1.id),
        "{err}"
    );

    // A change approves nothing.
    author
        .resolve(&p1.id, &resolution(Decision::Change, "Make Ines older."))
        .await
        .unwrap();
    assert!(!db.canon_approved().await.unwrap());
    assert_eq!(db.pending_question().await.unwrap(), None);
    let err = author
        .resolve(&p1.id, &resolution(Decision::Approve, ""))
        .await
        .unwrap_err();
    assert!(matches!(err, StoryDbError::Exists(_)), "{err}");

    let again = author
        .add_question(&run, "p1", &QuestionBody::Canon)
        .await
        .unwrap();
    author
        .resolve(&again.id, &resolution(Decision::Approve, ""))
        .await
        .unwrap();
    assert!(db.canon_approved().await.unwrap());
    let questions = db.questions().await.unwrap();
    assert_eq!(questions.len(), 2);
    let answer = questions[0].1.as_ref().unwrap();
    assert_eq!(
        (answer.decision, answer.notes.as_str()),
        (Decision::Change, "Make Ines older.")
    );
    assert_eq!(answer.evidence, json!({"messages": ["m7"]}));

    // A changed canon asks for its approval again.
    let mut canon = fixture_canon();
    canon.world.rules.push("The bell rings at midnight.".into());
    author.import_canon(&canon).await.unwrap();
    assert!(!db.canon_approved().await.unwrap());
    let unknown = author
        .resolve(&QuestionId::nth(9), &resolution(Decision::Approve, ""))
        .await;
    assert!(matches!(unknown, Err(StoryDbError::Unknown(_))));
}

#[tokio::test]
async fn an_approved_chapter_is_accepted_and_a_change_is_not() {
    let (_dir, db, mut author) = open().await;
    let mut log = db.run_log_writer().await.unwrap();
    let run = log.start_run(&json!({})).await.unwrap();
    let v1 = author
        .add_chapter(1, VersionOrigin::Draft, "text")
        .await
        .unwrap();
    let body = QuestionBody::Chapter {
        chapter: 1,
        version: 1,
    };
    let q = author
        .add_question(&run, "ch1/checkpoint", &body)
        .await
        .unwrap();
    author
        .resolve(&q.id, &resolution(Decision::Change, "Shorter."))
        .await
        .unwrap();
    assert!(!db.accepted(&v1.id).await.unwrap());
    let v2 = author
        .add_chapter(1, VersionOrigin::Human, "short")
        .await
        .unwrap();
    let body = QuestionBody::Chapter {
        chapter: 1,
        version: 2,
    };
    let q = author
        .add_question(&run, "ch1/checkpoint", &body)
        .await
        .unwrap();
    author
        .resolve(
            &q.id,
            &resolution(Decision::Approve, "More rain next time."),
        )
        .await
        .unwrap();
    assert!(db.accepted(&v2.id).await.unwrap());
    assert!(!db.accepted(&v1.id).await.unwrap());
}

#[tokio::test]
async fn assumptions_are_listed_by_their_steps_prefix() {
    let (_dir, db, mut author) = open().await;
    let mut log = db.run_log_writer().await.unwrap();
    let run = log.start_run(&json!({})).await.unwrap();
    let doubt = |n: u32| Assumption {
        doubt: format!("doubt {n}"),
        choice: format!("choice {n}"),
    };
    author
        .add_assumption(&run, "ch1/outline", "author", &doubt(1))
        .await
        .unwrap();
    author
        .add_assumption(&run, "ch1/sc2/plan", "author", &doubt(2))
        .await
        .unwrap();
    let id = author
        .add_assumption(&run, "ch10/outline", "author", &doubt(3))
        .await
        .unwrap();
    assert_eq!(id, "a3");
    let ch1 = db.assumptions("ch1/").await.unwrap();
    assert_eq!(
        ch1.iter().map(|a| a.step.as_str()).collect::<Vec<_>>(),
        ["ch1/outline", "ch1/sc2/plan"],
        "ch10 is not under ch1/"
    );
    assert_eq!(ch1[1].assumption, doubt(2));
    assert_eq!(ch1[1].run, run);
    let err = author
        .add_assumption(&RunId::nth(7), "ch1/outline", "author", &doubt(4))
        .await
        .unwrap_err();
    assert!(matches!(err, StoryDbError::UnknownRun(_)), "{err}");
}

#[tokio::test]
async fn a_step_any_run_finished_is_done() {
    let (_dir, db, _author) = open().await;
    let mut log = db.run_log_writer().await.unwrap();
    let first = log.start_run(&json!({})).await.unwrap();
    let second = log.start_run(&json!({})).await.unwrap();
    log.finish_step(&first, "ch1/outline", StepStatus::Done)
        .await
        .unwrap();
    log.finish_step(&first, "ch1/sc1/r0/k0", StepStatus::Failed)
        .await
        .unwrap();
    log.finish_step(&second, "ch1/sc1/r0/k0", StepStatus::Done)
        .await
        .unwrap();
    log.finish_step(&second, "ch1/sc1/r0/k1", StepStatus::Failed)
        .await
        .unwrap();
    let steps = db.all_steps().await.unwrap();
    assert_eq!(
        steps.into_iter().collect::<Vec<_>>(),
        [
            ("ch1/outline".to_owned(), StepStatus::Done),
            ("ch1/sc1/r0/k0".to_owned(), StepStatus::Done),
            ("ch1/sc1/r0/k1".to_owned(), StepStatus::Failed),
        ]
    );
}

#[tokio::test]
async fn a_chapters_spend_sums_every_run_and_attempt_under_it() {
    let (_dir, db, _author) = open().await;
    let mut log = db.run_log_writer().await.unwrap();
    let first = log.start_run(&json!({})).await.unwrap();
    let second = log.start_run(&json!({})).await.unwrap();
    let call = |run: &RunId, address: &str, attempt, tokens: (u64, u64), cost| NewCall {
        run: run.clone(),
        address: address.to_owned(),
        attempt,
        role: "author".to_owned(),
        model: "m".to_owned(),
        params: json!({}),
        mechanism: Mechanism::Text,
        request: json!({}),
        prompt_sha256: "h".to_owned(),
        response: None,
        status: CallStatus::Ok,
        usage: CallUsage {
            input_tokens: Some(tokens.0),
            output_tokens: Some(tokens.1),
            cached_tokens: None,
            cost_usd: cost,
        },
        latency_ms: None,
    };
    log.record_call(call(&first, "ch1/outline", 1, (100, 10), None))
        .await
        .unwrap();
    log.record_call(call(&first, "ch1/v1/write", 1, (200, 20), Some(0.5)))
        .await
        .unwrap();
    log.record_call(call(&second, "ch1/v1/write", 2, (300, 30), Some(0.25)))
        .await
        .unwrap();
    log.record_call(call(&second, "ch10/outline", 1, (9_000, 900), Some(9.0)))
        .await
        .unwrap();
    let spend = db.spend("ch1/").await.unwrap();
    assert_eq!(
        (spend.calls, spend.input_tokens, spend.output_tokens),
        (3, 600, 60)
    );
    assert_eq!(spend.tokens(), 660);
    assert_eq!(spend.cost_usd, Some(0.75));
    let none = db.spend("ch2/").await.unwrap();
    assert_eq!((none.calls, none.tokens(), none.cost_usd), (0, 0, None));
}

/// A call of `tokens` prompt tokens at `address`, attempt 1, under `run`.
fn priced(run: &RunId, address: &str, tokens: u64) -> NewCall {
    NewCall {
        run: run.clone(),
        address: address.to_owned(),
        attempt: 1,
        role: "author".to_owned(),
        model: "m".to_owned(),
        params: json!({}),
        mechanism: Mechanism::Text,
        request: json!({}),
        prompt_sha256: "h".to_owned(),
        response: None,
        status: CallStatus::Ok,
        usage: CallUsage {
            input_tokens: Some(tokens),
            output_tokens: Some(0),
            cached_tokens: None,
            cost_usd: None,
        },
        latency_ms: None,
    }
}

/// A backup carries the story database and leaves its call log out (spec §6,
/// §15), so a restore can put back a story behind its log. The run the log
/// knows and the story does not is another timeline: the next run is numbered
/// past it, since run ids are positional and a reused one collides with its
/// calls, and the chapter's spend leaves it out (docs/fabula.md §4, PR 7).
#[tokio::test]
async fn a_story_restored_behind_its_call_log_runs_on_and_counts_its_own_calls() {
    let dir = tempfile::tempdir().unwrap();
    let (story, calls) = (dir.path().join("story.db"), dir.path().join("calls.db"));
    let db = StoryDb::open(&story, &calls).await.unwrap();
    let mut log = db.run_log_writer().await.unwrap();
    let first = log.start_run(&json!({})).await.unwrap();
    log.record_call(priced(&first, "ch1/outline", 100))
        .await
        .unwrap();
    drop(log);
    db.close().await.unwrap();
    let backup = dir.path().join("story.backup.db");
    tokio::fs::copy(&story, &backup).await.unwrap();

    let db = StoryDb::open(&story, &calls).await.unwrap();
    let mut log = db.run_log_writer().await.unwrap();
    let lost = log.start_run(&json!({})).await.unwrap();
    assert_eq!(lost, RunId::nth(2));
    log.record_call(priced(&lost, "ch1/sc1/plan", 1000))
        .await
        .unwrap();
    drop(log);
    db.close().await.unwrap();

    tokio::fs::copy(&backup, &story).await.unwrap();
    let db = StoryDb::open(&story, &calls).await.unwrap();
    let mut log = db.run_log_writer().await.unwrap();
    let next = log.start_run(&json!({})).await.unwrap();
    assert_eq!(next, RunId::nth(3));
    log.record_call(priced(&next, "ch1/sc1/plan", 10))
        .await
        .unwrap();
    let spend = db.spend("ch1/").await.unwrap();
    assert_eq!((spend.calls, spend.input_tokens), (2, 110));
}

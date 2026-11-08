//! Tests of a story's state as `story_status` reads it (docs/fabula.md §4,
//! PR 8): written by the store's own writers, read beside a store that is
//! still open, and left as it was.

use std::path::{Path, PathBuf};

use serde_json::json;

use super::{ChapterState, story_status_of};
use crate::entities::story::{
    Canon, ChapterArc, Decision, QuestionBody, Resolution, RunId, VersionOrigin, fixture_canon,
};
use crate::shared::storage::schema::STORY_SCHEMA;
use crate::shared::storage::story::{
    CallStatus, CallUsage, Mechanism, NewCall, StoryDb, StoryDbError,
};

fn call(run: &RunId, address: &str, tokens: u64) -> NewCall {
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
            output_tokens: Some(1),
            cached_tokens: None,
            cost_usd: Some(0.01),
        },
        latency_ms: None,
    }
}

/// The fixture's canon with an arc of three chapters.
fn canon() -> Canon {
    let mut canon = fixture_canon();
    canon.arc.chapters.truncate(1);
    for goal in ["The debt is called in.", "The harbour decides."] {
        canon.arc.chapters.push(ChapterArc {
            goal: goal.to_owned(),
        });
    }
    canon
}

fn answer(decision: Decision) -> Resolution {
    Resolution {
        decision,
        notes: String::new(),
        evidence: json!(null),
    }
}

/// The paths of a story in `dir`.
fn files(dir: &Path) -> (PathBuf, PathBuf) {
    (dir.join("s1.db"), dir.join("s1.calls.db"))
}

/// A story whose canon is approved, whose chapter 1 is accepted in its second
/// version and whose chapter 2 waits at its checkpoint, with calls under both
/// chapters. The store is handed back open.
async fn written(dir: &Path) -> StoryDb {
    let (story, calls) = files(dir);
    let db = StoryDb::open(&story, &calls).await.unwrap();
    let mut author = db.author_writer().await.unwrap();
    author.import_canon(&canon()).await.unwrap();
    let mut log = db.run_log_writer().await.unwrap();
    let run = log.start_run(&json!({})).await.unwrap();
    let canon = author
        .add_question(&run, "canon", &QuestionBody::Canon)
        .await
        .unwrap();
    author
        .resolve(&canon.id, &answer(Decision::Approve))
        .await
        .unwrap();
    log.record_call(call(&run, "ch1/outline", 500))
        .await
        .unwrap();
    author
        .add_chapter(1, VersionOrigin::Draft, "## One\n\nFirst.")
        .await
        .unwrap();
    author
        .add_chapter(1, VersionOrigin::Revision, "## One\n\nSecond.")
        .await
        .unwrap();
    let first = author
        .add_question(
            &run,
            "ch1/v2/checkpoint",
            &QuestionBody::Chapter {
                chapter: 1,
                version: 2,
            },
        )
        .await
        .unwrap();
    author
        .resolve(&first.id, &answer(Decision::Approve))
        .await
        .unwrap();
    log.record_call(call(&run, "ch2/outline", 70))
        .await
        .unwrap();
    log.record_call(call(&run, "ch2/v1/write", 30))
        .await
        .unwrap();
    author
        .add_chapter(2, VersionOrigin::Draft, "Two.")
        .await
        .unwrap();
    author
        .add_question(
            &run,
            "ch2/v1/checkpoint",
            &QuestionBody::Chapter {
                chapter: 2,
                version: 1,
            },
        )
        .await
        .unwrap();
    drop((author, log));
    db
}

/// The status reads the story while its store is open — as a run holds it —
/// and says where it stands: the arc, the chapters with their versions and
/// acceptance, the pending question, and what the open chapter cost, its own
/// calls only.
#[tokio::test]
async fn the_status_reads_a_story_its_store_holds_open() {
    let dir = tempfile::tempdir().unwrap();
    let db = written(dir.path()).await;
    let (story, calls) = files(dir.path());

    let status = story_status_of(&story, &calls).unwrap();

    let canon = canon();
    assert_eq!(status.premise.as_deref(), Some(canon.arc.premise.as_str()));
    assert_eq!(status.planned, 3);
    assert!(status.canon_approved);
    assert_eq!(
        status.chapters,
        vec![
            ChapterState {
                number: 1,
                versions: 2,
                accepted: true,
            },
            ChapterState {
                number: 2,
                versions: 1,
                accepted: false,
            },
        ]
    );
    let pending = status.pending.expect("chapter 2 waits");
    assert_eq!(
        pending.body,
        QuestionBody::Chapter {
            chapter: 2,
            version: 1
        }
    );
    assert_eq!(status.open_chapter, Some(2));
    assert_eq!(status.spend.input_tokens, 100);
    assert_eq!(status.spend.calls, 2);
    db.close().await.unwrap();
}

/// Reading changes nothing: the files are byte for byte what they were, and
/// the store opens them again afterwards.
#[tokio::test]
async fn the_status_leaves_the_files_as_they_were() {
    let dir = tempfile::tempdir().unwrap();
    written(dir.path()).await.close().await.unwrap();
    let (story, calls) = files(dir.path());
    let before = (
        tokio::fs::read(&story).await.unwrap(),
        tokio::fs::read(&calls).await.unwrap(),
    );

    story_status_of(&story, &calls).unwrap();

    let after = (
        tokio::fs::read(&story).await.unwrap(),
        tokio::fs::read(&calls).await.unwrap(),
    );
    assert!(before == after, "a read wrote to the files");
    StoryDb::open(&story, &calls)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
}

/// The call log stays outside backups, so a restored story may have none: it
/// reads all the same, with nothing spent.
#[tokio::test]
async fn a_story_without_its_call_log_reads_with_nothing_spent() {
    let dir = tempfile::tempdir().unwrap();
    written(dir.path()).await.close().await.unwrap();
    let (story, calls) = files(dir.path());
    tokio::fs::remove_file(&calls).await.unwrap();

    let status = story_status_of(&story, &calls).unwrap();

    assert_eq!(status.open_chapter, Some(2));
    assert_eq!(status.spend.calls, 0);
    assert!(!calls.exists(), "nothing is created");
}

/// A story without its canon yet plans nothing; one of a newer schema is
/// refused as the store refuses it; a file that is not a database too.
#[tokio::test]
async fn an_empty_story_plans_nothing_and_a_newer_one_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (story, calls) = files(dir.path());
    StoryDb::open(&story, &calls)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();

    let empty = story_status_of(&story, &calls).unwrap();
    assert_eq!(empty.premise, None);
    assert_eq!(empty.planned, 0);
    assert_eq!(empty.open_chapter, None);
    assert!(!empty.canon_approved);
    assert!(empty.chapters.is_empty() && empty.pending.is_none());

    rusqlite::Connection::open(&story)
        .unwrap()
        .execute_batch(&format!("PRAGMA user_version = {};", STORY_SCHEMA + 1))
        .unwrap();
    assert!(matches!(
        story_status_of(&story, &calls),
        Err(StoryDbError::NewerSchema { .. })
    ));

    let text = dir.path().join("junk.db");
    tokio::fs::write(&text, b"not a database").await.unwrap();
    assert!(matches!(
        story_status_of(&text, &calls),
        Err(StoryDbError::NotADatabase { .. })
    ));
}

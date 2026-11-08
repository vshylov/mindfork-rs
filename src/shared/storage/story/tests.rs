//! The story database against the harbour fixture (docs/fabula.md §4, PR 1):
//! what the schema refuses, the single writer, the canon's versions, the run
//! log, and the files' versions.

use std::path::PathBuf;

use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::entities::story::{QuestionBody, VersionOrigin, fixture_canon};

/// An open story in a directory of its own.
struct Story {
    dir: TempDir,
    db: StoryDb,
}

impl Story {
    fn story_path(&self) -> PathBuf {
        self.dir.path().join("story.db")
    }

    fn calls_path(&self) -> PathBuf {
        self.dir.path().join("story.calls.db")
    }
}

async fn open() -> Story {
    let dir = tempfile::tempdir().unwrap();
    let db = StoryDb::open(
        &dir.path().join("story.db"),
        &dir.path().join("story.calls.db"),
    )
    .await
    .unwrap();
    Story { dir, db }
}

/// A story with the fixture's canon imported, and the Author's writer.
async fn with_canon() -> (Story, AuthorWriter) {
    let story = open().await;
    let mut author = story.db.author_writer().await.unwrap();
    author.import_canon(&fixture_canon()).await.unwrap();
    (story, author)
}

// ---------- what the schema refuses ----------

#[tokio::test]
async fn update_and_delete_on_the_manuscript_are_refused() {
    let (story, mut author) = with_canon().await;
    author
        .add_chapter(1, VersionOrigin::Draft, "## One\n\nText.")
        .await
        .unwrap();
    for sql in [
        "UPDATE canon_doc SET body_json = NULL",
        "DELETE FROM canon_doc",
        "UPDATE chapter SET text_md = 'Nothing.'",
        "DELETE FROM chapter",
    ] {
        let err = story.db.execute_raw(sql).await.unwrap_err();
        assert!(err.to_string().contains("is append-only"), "{sql}: {err}");
    }
    let versions = story.db.chapter_versions(1).await.unwrap();
    assert_eq!(versions[0].text, "## One\n\nText.");
}

#[tokio::test]
async fn every_append_only_table_carries_both_triggers() {
    let story = open().await;
    let count = |table: &str| {
        format!(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' \
             AND name IN ('{table}_no_update', '{table}_no_delete')"
        )
    };
    let conn = rusqlite::Connection::open(story.story_path()).unwrap();
    for table in schema::STORY_APPEND_ONLY {
        let n: i64 = conn.query_row(&count(table), [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2, "{table}");
    }
    let conn = rusqlite::Connection::open(story.calls_path()).unwrap();
    for table in schema::CALLS_APPEND_ONLY {
        let n: i64 = conn.query_row(&count(table), [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2, "{table}");
    }
}

// ---------- the single writer ----------

#[tokio::test]
async fn every_store_has_one_writer_at_a_time() {
    let (story, author) = with_canon().await;
    let held = |r: Result<(), StoryDbError>| matches!(r, Err(StoryDbError::WriterHeld(_)));
    assert!(held(story.db.author_writer().await.map(drop)));
    drop(author);
    assert!(story.db.author_writer().await.is_ok());

    let run_log = story.db.run_log_writer().await.unwrap();
    assert!(held(story.db.run_log_writer().await.map(drop)));
    drop(run_log);
    assert!(story.db.run_log_writer().await.is_ok());
}

#[tokio::test]
async fn a_story_is_open_once_per_process() {
    let story = open().await;
    let err = StoryDb::open(&story.story_path(), &story.calls_path())
        .await
        .err()
        .unwrap();
    assert!(matches!(err, StoryDbError::AlreadyOpen(_)), "{err}");
    // The same file for both halves is refused before anything opens.
    let other = tempfile::tempdir().unwrap();
    let same = other.path().join("one.db");
    let err = StoryDb::open(&same, &same).await.err().unwrap();
    assert!(matches!(err, StoryDbError::BadPath(_)), "{err}");
}

#[tokio::test]
async fn a_closed_story_opens_again_with_its_data() {
    let (story, author) = with_canon().await;
    let other = story.db.clone();
    drop(author);
    story.db.clone().close().await.unwrap();
    assert!(matches!(other.canon().await, Err(StoryDbError::Closed)));
    let reopened = StoryDb::open(&story.story_path(), &story.calls_path())
        .await
        .unwrap();
    assert_eq!(reopened.canon().await.unwrap(), Some(fixture_canon()));
    reopened.close().await.unwrap();
}

// ---------- the canon ----------

#[tokio::test]
async fn the_canon_reads_back_as_imported_and_versions_only_what_changed() {
    let story = open().await;
    assert_eq!(story.db.canon().await.unwrap(), None);
    let mut author = story.db.author_writer().await.unwrap();
    let canon = fixture_canon();
    // world, style, arc, five locations, three characters.
    assert_eq!(author.import_canon(&canon).await.unwrap(), 11);
    assert_eq!(story.db.canon().await.unwrap(), Some(canon.clone()));
    assert_eq!(author.import_canon(&canon).await.unwrap(), 0);

    let mut changed = canon.clone();
    changed.characters[1].voice = "Clipped, impatient.".into();
    changed.locations.swap(3, 4);
    changed.locations.retain(|l| l.key.as_str() != "chapel");
    changed.locations[1]
        .routes
        .retain(|r| r.to.as_str() != "chapel");
    // silas's sheet, the inn (route gone), the warehouse (moved), the chapel's removal.
    assert_eq!(author.import_canon(&changed).await.unwrap(), 4);
    assert_eq!(story.db.canon().await.unwrap(), Some(changed));

    let conn = rusqlite::Connection::open(story.story_path()).unwrap();
    let versions: Vec<(String, i64, bool)> = conn
        .prepare(
            "SELECT id, version, body_json IS NULL FROM canon_doc
             WHERE key IN ('chapel', 'silas') ORDER BY id",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        versions,
        [
            ("character/silas/v1".to_owned(), 1, false),
            ("character/silas/v2".to_owned(), 2, false),
            ("location/chapel/v1".to_owned(), 1, false),
            ("location/chapel/v2".to_owned(), 2, true),
        ]
    );
}

#[tokio::test]
async fn a_canon_with_problems_is_refused_whole() {
    let story = open().await;
    let mut author = story.db.author_writer().await.unwrap();
    let mut canon = fixture_canon();
    canon.characters[0].start.location = "moon".into();
    canon.locations[0].name.clear();
    let err = author.import_canon(&canon).await.unwrap_err();
    let StoryDbError::Canon(problems) = &err else {
        panic!("{err}");
    };
    assert_eq!(problems.len(), 2);
    assert_eq!(
        err.to_string(),
        "the canon was refused: location \"harbour\" has an empty name; \
         character \"mara\" starts at \"moon\", which is not a location"
    );
    assert_eq!(story.db.canon().await.unwrap(), None);
}

#[tokio::test]
async fn the_canon_is_frozen_once_it_is_approved() {
    let (story, mut author) = with_canon().await;
    let mut edited = fixture_canon();
    edited.world.rules.push("Nobody swims in winter.".into());
    assert_eq!(author.import_canon(&edited).await.unwrap(), 1);
    let run = story
        .db
        .run_log_writer()
        .await
        .unwrap()
        .start_run(&json!({}))
        .await
        .unwrap();
    let question = author
        .add_question(&run, "canon", &QuestionBody::Canon)
        .await
        .unwrap();
    author
        .resolve(
            &question.id,
            &Resolution {
                decision: crate::entities::story::Decision::Approve,
                notes: String::new(),
                evidence: json!(null),
            },
        )
        .await
        .unwrap();
    assert!(story.db.canon_approved().await.unwrap());
    let err = author.import_canon(&fixture_canon()).await.unwrap_err();
    assert!(matches!(err, StoryDbError::CanonFrozen), "{err}");
}

// ---------- the run log ----------

fn call(run: &RunId, attempt: u32) -> NewCall {
    NewCall {
        run: run.clone(),
        address: "ch1/b0/t1".into(),
        attempt,
        role: "author".into(),
        model: "qwen/qwen3.8-27b".into(),
        params: json!({"temperature": 0.8}),
        mechanism: Mechanism::Schema,
        request: json!({"messages": [{"role": "user", "content": "Write the story."}]}),
        prompt_sha256: "ab".repeat(32),
        response: Some(json!({"text": "## One\n\nText."})),
        status: CallStatus::Ok,
        usage: CallUsage {
            input_tokens: Some(2500),
            output_tokens: Some(158),
            cached_tokens: None,
            cost_usd: Some(0.0004),
        },
        latency_ms: Some(1600),
    }
}

#[tokio::test]
async fn the_run_log_records_calls_by_address() {
    let story = open().await;
    let mut log = story.db.run_log_writer().await.unwrap();
    let run = log.start_run(&json!({"branches": 3})).await.unwrap();
    assert_eq!(run, RunId::nth(1));
    assert_eq!(log.start_run(&json!({})).await.unwrap(), RunId::nth(2));

    let id = log.record_call(call(&run, 1)).await.unwrap();
    assert_eq!(id, actor_call_id(&run, "ch1/b0/t1", 1));
    let stored = story.db.call(&run, "ch1/b0/t1", 1).await.unwrap().unwrap();
    assert_eq!(stored.id, id);
    assert_eq!(stored.call, call(&run, 1));
    assert!(!stored.created_at.is_empty());
    assert_eq!(story.db.call(&run, "ch1/b0/t1", 2).await.unwrap(), None);
    let err = log.record_call(call(&run, 1)).await.unwrap_err();
    assert!(matches!(err, StoryDbError::CallExists { .. }), "{err}");
    let err = log.record_call(call(&RunId::nth(9), 1)).await.unwrap_err();
    assert!(matches!(err, StoryDbError::UnknownRun(_)), "{err}");

    // A failed call keeps no reply and no usage.
    let failed = NewCall {
        response: None,
        status: CallStatus::Cut,
        mechanism: Mechanism::Fallback,
        usage: CallUsage::default(),
        latency_ms: None,
        ..call(&run, 2)
    };
    log.record_call(failed.clone()).await.unwrap();
    let stored = story.db.call(&run, "ch1/b0/t1", 2).await.unwrap().unwrap();
    assert_eq!(stored.call, failed);

    let err = story
        .db
        .execute_raw_calls("UPDATE llm_call SET status = 'error'")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("is append-only"), "{err}");
}

/// The id the call log derives, computed here independently.
fn actor_call_id(run: &RunId, address: &str, attempt: u32) -> String {
    let namespace = uuid::Uuid::from_u128(0x6d69_6e64_666f_726b_6661_6275_6c61_0001);
    uuid::Uuid::new_v5(
        &namespace,
        format!("{run}\n{address}\n{attempt}").as_bytes(),
    )
    .to_string()
}

#[test]
fn stored_names_read_back() {
    for mechanism in [Mechanism::Schema, Mechanism::Fallback, Mechanism::Text] {
        assert_eq!(Mechanism::parse(mechanism.as_str()), Some(mechanism));
    }
    for status in [
        CallStatus::Ok,
        CallStatus::Cut,
        CallStatus::Refused,
        CallStatus::Error,
    ] {
        assert_eq!(CallStatus::parse(status.as_str()), Some(status));
    }
    assert_eq!(Mechanism::parse("tool"), None);
    assert_eq!(CallStatus::parse("late"), None);
}

// ---------- the files ----------

#[tokio::test]
async fn both_files_are_stamped_with_their_schemas() {
    let story = open().await;
    story.db.clone().close().await.unwrap();
    let version = |path: PathBuf| -> u32 {
        rusqlite::Connection::open(path)
            .unwrap()
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap()
    };
    assert_eq!(
        version(story.story_path()),
        crate::shared::storage::schema::STORY_SCHEMA
    );
    assert_eq!(
        version(story.calls_path()),
        crate::shared::storage::schema::STORY_CALLS_SCHEMA
    );
}

#[tokio::test]
async fn a_newer_file_is_refused_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let (story, calls) = (dir.path().join("s.db"), dir.path().join("c.db"));
    {
        let conn = rusqlite::Connection::open(&story).unwrap();
        conn.execute_batch("CREATE TABLE future (x); PRAGMA user_version = 2;")
            .unwrap();
    }
    let before = std::fs::read(&story).unwrap();
    let err = StoryDb::open(&story, &calls).await.err().unwrap();
    assert_eq!(
        err.to_string(),
        format!(
            "story database {} is from a newer version of mindfork (schema 2, supported 1)",
            story.display()
        )
    );
    assert_eq!(std::fs::read(&story).unwrap(), before);
    // The refusal gave the file back: a second attempt meets the same refusal,
    // not "open already".
    let err = StoryDb::open(&story, &calls).await.err().unwrap();
    assert!(
        matches!(err, StoryDbError::NewerSchema { found: 2, .. }),
        "{err}"
    );

    // The call log is judged the same way.
    let (story2, calls2) = (dir.path().join("s2.db"), dir.path().join("c2.db"));
    rusqlite::Connection::open(&calls2)
        .unwrap()
        .execute_batch("PRAGMA user_version = 5;")
        .unwrap();
    let err = StoryDb::open(&story2, &calls2).await.err().unwrap();
    assert!(
        matches!(
            err,
            StoryDbError::NewerSchema {
                file: "story call log",
                found: 5,
                ..
            }
        ),
        "{err}"
    );
}

#[tokio::test]
async fn a_file_that_is_not_a_database_is_refused_by_its_header() {
    let dir = tempfile::tempdir().unwrap();
    let (story, calls) = (dir.path().join("s.db"), dir.path().join("c.db"));
    std::fs::write(&story, b"Once upon a time, in a harbour town").unwrap();
    let err = StoryDb::open(&story, &calls).await.err().unwrap();
    assert!(matches!(err, StoryDbError::NotADatabase { .. }), "{err}");
    assert_eq!(
        std::fs::read(&story).unwrap(),
        b"Once upon a time, in a harbour town"
    );
    // An empty file is a fresh database, as SQLite reads it.
    std::fs::write(&story, b"").unwrap();
    StoryDb::open(&story, &calls)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
}

//! Tests of a story's summary (docs/fabula.md §4, PR 7). The story under test
//! is written by the store's own writers, so the reader meets the rows the
//! engine leaves on disk.

use std::path::{Path, PathBuf};

use serde_json::json;

use super::{StoryStats, story_stats_of_file, story_stats_of_image};
use crate::entities::story::{
    Decision, QuestionBody, QuestionId, Resolution, VersionOrigin, fixture_canon,
};
use crate::shared::storage::schema::STORY_SCHEMA;
use crate::shared::storage::story::{StepStatus, StoryDb};

/// A story with two chapters: the first in two versions and accepted, the
/// second drafted and waiting at its checkpoint. One run; one step failed and
/// one done.
async fn written(dir: &Path) -> PathBuf {
    let story = dir.join("story.db");
    let db = StoryDb::open(&story, &dir.join("calls.db")).await.unwrap();
    let mut author = db.author_writer().await.unwrap();
    author.import_canon(&fixture_canon()).await.unwrap();
    let mut log = db.run_log_writer().await.unwrap();
    let run = log.start_run(&json!({})).await.unwrap();
    log.finish_step(&run, "ch1/outline", StepStatus::Failed)
        .await
        .unwrap();
    log.finish_step(&run, "ch1/v1/write", StepStatus::Done)
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
    let asked = author
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
    author.resolve(&asked.id, &approve()).await.unwrap();
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
    db.close().await.unwrap();
    story
}

fn approve() -> Resolution {
    Resolution {
        decision: Decision::Approve,
        notes: String::new(),
        evidence: json!(null),
    }
}

/// `story` copied to `name` beside it, opened through the store.
async fn copy_of(story: &Path, name: &str) -> (PathBuf, StoryDb) {
    let copy = story.with_file_name(format!("{name}.db"));
    tokio::fs::copy(story, &copy).await.unwrap();
    let db = StoryDb::open(&copy, &story.with_file_name(format!("{name}.calls.db")))
        .await
        .unwrap();
    (copy, db)
}

fn units_of(stats: &StoryStats, prefix: &str) -> Vec<String> {
    stats
        .units
        .iter()
        .filter(|unit| unit.starts_with(prefix))
        .cloned()
        .collect()
}

#[tokio::test]
async fn a_story_is_counted_as_its_store_wrote_it_and_left_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let story = written(dir.path()).await;
    let before = tokio::fs::read(&story).await.unwrap();

    let stats = story_stats_of_file(&story).unwrap();

    assert_eq!(
        tokio::fs::read(&story).await.unwrap(),
        before,
        "the file changed"
    );
    let mut listing = tokio::fs::read_dir(dir.path()).await.unwrap();
    while let Some(entry) = listing.next_entry().await.unwrap() {
        let name = entry.file_name().into_string().unwrap();
        assert!(!name.ends_with("-journal"), "{name} was left beside it");
    }
    assert_eq!(stats.schema, STORY_SCHEMA);
    assert_eq!(
        (
            stats.chapters,
            stats.accepted_chapters,
            stats.chapter_versions
        ),
        (2, 1, 3)
    );
    assert_eq!((stats.questions, stats.pending_questions), (2, 1));
    assert_eq!((stats.runs, stats.scenes), (1, 0));
    // Only the step done counts, and by its key alone.
    assert_eq!(units_of(&stats, "step/").len(), 1);
    assert!(units_of(&stats, "step/")[0].starts_with("step/run1/ch1/v1/write:"));
    assert_eq!(units_of(&stats, "chapter/").len(), 3);
    assert!(
        stats.units.windows(2).all(|w| w[0] < w[1]),
        "sorted, distinct"
    );
    assert!(!units_of(&stats, "canon_doc/").is_empty());

    let image = tokio::fs::read(&story).await.unwrap();
    let from_archive = story_stats_of_image(image.as_slice(), image.len()).unwrap();
    assert_eq!(from_archive, stats);
}

/// A copy continued holds every unit of the one it was copied from; two
/// copies continued apart hold one id with two contents, and the digests tell
/// them apart. A step recorded done again is the same unit.
#[tokio::test]
async fn units_tell_a_story_continued_from_one_continued_apart() {
    let dir = tempfile::tempdir().unwrap();
    let story = written(dir.path()).await;
    let base = story_stats_of_file(&story).unwrap();
    let waiting = QuestionId::nth(2);

    let (ahead, db) = copy_of(&story, "ahead").await;
    let mut author = db.author_writer().await.unwrap();
    author.resolve(&waiting, &approve()).await.unwrap();
    let mut log = db.run_log_writer().await.unwrap();
    log.finish_step(
        &crate::entities::story::RunId::nth(1),
        "ch1/v1/write",
        StepStatus::Done,
    )
    .await
    .unwrap();
    drop((author, log));
    db.close().await.unwrap();
    let ahead = story_stats_of_file(&ahead).unwrap();
    assert!(base.units.iter().all(|u| ahead.units.contains(u)));
    assert_eq!(
        ahead.units.len(),
        base.units.len() + 2,
        "the answer, the acceptance"
    );
    assert_eq!(ahead.pending_questions, 0);

    let (apart, db) = copy_of(&story, "apart").await;
    let mut author = db.author_writer().await.unwrap();
    let change = Resolution {
        decision: Decision::Change,
        notes: "Slower.".to_owned(),
        evidence: json!(null),
    };
    author.resolve(&waiting, &change).await.unwrap();
    drop(author);
    db.close().await.unwrap();
    let apart = story_stats_of_file(&apart).unwrap();
    let (here, there) = (
        units_of(&ahead, "answer/q2:"),
        units_of(&apart, "answer/q2:"),
    );
    assert_eq!((here.len(), there.len()), (1, 1));
    assert_ne!(here, there);
}

/// A file with none of the tables (one a newer schema would have renamed
/// away, say) counts as empty, at the schema it states.
#[test]
fn a_file_without_the_tables_counts_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bare.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA user_version = 9; CREATE TABLE other (x INTEGER);")
        .unwrap();
    drop(conn);
    let stats = story_stats_of_file(&path).unwrap();
    assert_eq!(
        stats,
        StoryStats {
            schema: 9,
            ..StoryStats::default()
        }
    );
}

#[test]
fn a_file_that_is_not_a_database_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("text.db");
    std::fs::write(&path, b"not sqlite").unwrap();
    assert!(story_stats_of_file(&path).is_err());
}

/// A witness belongs to its event's unit: a copy whose event was seen by one
/// more character holds the same event id with another content.
#[tokio::test]
async fn an_events_witnesses_are_part_of_its_unit() {
    let dir = tempfile::tempdir().unwrap();
    let story = written(dir.path()).await;
    let conn = rusqlite::Connection::open(&story).unwrap();
    conn.execute_batch(
        "INSERT OR IGNORE INTO branch (id, parent_id, fork_seq, scene_id, k, seed)
             VALUES ('genesis', NULL, NULL, NULL, 0, 0);
         INSERT INTO event (id, branch_id, round, tick, location_key, kind, actor_key,
                            visibility_json, observable, private, effects_json, proposal_id,
                            cause_json)
             VALUES ('genesis/e1', 'genesis', 0, 0, 'quay', 'opening', NULL, '{}', 'Dawn.',
                     NULL, '[]', NULL, '{}');
         INSERT INTO event_witness (event_id, character_key) VALUES ('genesis/e1', 'mara');",
    )
    .unwrap();
    drop(conn);
    let (event, character) = ("genesis/e1".to_owned(), "mara".to_owned());
    let base = story_stats_of_file(&story).unwrap();

    let copy = dir.path().join("seen.db");
    tokio::fs::copy(&story, &copy).await.unwrap();
    let conn = rusqlite::Connection::open(&copy).unwrap();
    conn.execute(
        "INSERT INTO event_witness (event_id, character_key) VALUES (?1, ?2)",
        [&event, &format!("{character}-twin")],
    )
    .unwrap();
    drop(conn);
    let seen = story_stats_of_file(&copy).unwrap();

    let unit = |stats: &StoryStats| units_of(stats, &format!("event/{event}:"));
    assert_eq!((unit(&base).len(), unit(&seen).len()), (1, 1));
    assert_ne!(unit(&base), unit(&seen));
    assert_eq!(seen.units.len(), base.units.len(), "no unit of its own");
}

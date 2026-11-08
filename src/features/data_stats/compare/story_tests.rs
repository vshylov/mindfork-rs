//! Stories in `mindfork stats` and `--compare` (docs/fabula.md §4, PR 7). The
//! stories are written by the story store's own writers into the data root the
//! summary's other tests use, so the reader meets what the engine leaves on
//! disk: a story under its chat, counted as deleted while the chat is hidden
//! (§6 T4), and two copies of one story told apart by their rows.

use std::fs;

use uuid::Uuid;

use super::super::tests::{PASSWORD, backup, en, root, snapshot};
use super::super::{
    DataStats, collect_archive, collect_root, read_snapshot, render_json, render_text,
};
use super::tests::{Pair, busy, copy_tree};
use super::{Caveat, Side, StoryComparison, Verdict, compare, render_comparison_text};
use crate::entities::chat::Chat;
use crate::entities::story::{
    Decision, QuestionBody, QuestionId, Resolution, VersionOrigin, fixture_canon,
};
use crate::shared::i18n::{Lang, locale};
use crate::shared::paths::Paths;
use crate::shared::storage::JsonStore;
use crate::shared::storage::schema::STORY_SCHEMA;
use crate::shared::storage::story::StoryDb;

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(future)
}

async fn open(paths: &Paths, chat: Uuid, story: Uuid) -> StoryDb {
    let (chat, story) = (chat.to_string(), story.to_string());
    StoryDb::open(
        &paths.story_db(&chat, &story),
        &paths.story_log(&chat, &story),
    )
    .await
    .unwrap()
}

/// A story in `chat`: its first chapter accepted, its second waiting at its
/// checkpoint (question 2).
fn seed_story(paths: &Paths, chat: Uuid) -> Uuid {
    let story = Uuid::new_v4();
    block_on(async {
        let db = open(paths, chat, story).await;
        let mut author = db.author_writer().await.unwrap();
        author.import_canon(&fixture_canon()).await.unwrap();
        let mut log = db.run_log_writer().await.unwrap();
        let run = log.start_run(&serde_json::json!({})).await.unwrap();
        for chapter in [1, 2] {
            author
                .add_chapter(chapter, VersionOrigin::Draft, "Text.")
                .await
                .unwrap();
            let asked = author
                .add_question(
                    &run,
                    &format!("ch{chapter}/v1/checkpoint"),
                    &QuestionBody::Chapter {
                        chapter,
                        version: 1,
                    },
                )
                .await
                .unwrap();
            if chapter == 1 {
                author
                    .resolve(&asked.id, &resolution(Decision::Approve))
                    .await
                    .unwrap();
            }
        }
        drop((author, log));
        db.close().await.unwrap();
    });
    story
}

/// Answers the question the story waits on.
fn answer(paths: &Paths, chat: Uuid, story: Uuid, decision: Decision) {
    block_on(async {
        let db = open(paths, chat, story).await;
        let mut author = db.author_writer().await.unwrap();
        author
            .resolve(&QuestionId::nth(2), &resolution(decision))
            .await
            .unwrap();
        drop(author);
        db.close().await.unwrap();
    });
}

fn resolution(decision: Decision) -> Resolution {
    Resolution {
        notes: if decision == Decision::Change {
            "Slower.".into()
        } else {
            String::new()
        },
        decision,
        evidence: serde_json::Value::Null,
    }
}

fn hidden(paths: &Paths) -> Chat {
    let chats = JsonStore::new(paths.clone()).load_chats().unwrap();
    chats.into_iter().find(|c| c.title == "hidden").unwrap()
}

/// The fixture root with a story in its busy chat, copied as *there*.
fn story_pair() -> (Pair, Uuid, Uuid) {
    let here = root();
    let chat = busy(&here.paths).id;
    let story = seed_story(&here.paths, chat);
    let there_dir = tempfile::tempdir().unwrap();
    let there = Paths::with_root(there_dir.path().join("data"));
    copy_tree(here.paths.root(), there.root());
    let pair = Pair {
        here,
        _there_dir: there_dir,
        there,
    };
    (pair, chat, story)
}

/// `[same, only here, only there, ahead here, ahead there, diverged]`.
fn shape(c: &StoryComparison) -> [usize; 6] {
    [
        c.same,
        c.only_here.len(),
        c.only_there.len(),
        c.ahead_here.len(),
        c.ahead_there.len(),
        c.diverged.len(),
    ]
}

#[test]
fn a_story_is_counted_under_its_chat_and_deleted_with_it() {
    let root = root();
    let paths = &root.paths;
    let (busy_id, hidden_id, nobody) = (busy(paths).id, hidden(paths).id, Uuid::new_v4());
    let live = seed_story(paths, busy_id);
    seed_story(paths, hidden_id);
    let orphan = seed_story(paths, nobody);
    // Not a story: a name that is no id. Unreadable: a story's name, not a
    // database.
    let stray = paths.stories_dir().join("notes");
    fs::create_dir_all(&stray).unwrap();
    fs::write(stray.join("x.db"), b"junk").unwrap();
    let broken = format!("{busy_id}/{}.db", Uuid::new_v4());
    fs::write(paths.stories_dir().join(&broken), b"junk").unwrap();
    let before = snapshot(root.dir.path());

    let stats = collect_root(paths, en()).unwrap();

    assert_eq!(snapshot(root.dir.path()), before, "the root changed");
    let s = &stats.stories;
    assert_eq!((s.total, s.deleted), (3, 1));
    assert_eq!(s.without_chat, vec![format!("{nobody}/{orphan}")]);
    assert_eq!(
        (s.chapters, s.accepted_chapters, s.pending_questions),
        (6, 3, 3)
    );
    assert_eq!(s.unreadable, vec![broken]);
    assert_eq!((s.newest_schema, s.call_logs), (STORY_SCHEMA, 3));
    let list = stats.story_list.as_ref().unwrap();
    assert_eq!(list.len(), 3);
    let row = list.iter().find(|r| r.id == live).unwrap();
    assert_eq!(
        (row.chat_id, row.chat_title.as_deref()),
        (busy_id, Some("busy"))
    );
    assert!(!row.deleted && !row.units.is_empty());
    let gone = list.iter().find(|r| r.chat_id == hidden_id).unwrap();
    assert!(gone.deleted);
    assert!(stats.sizes.stories_bytes > 0 && stats.sizes.story_logs_bytes > 0);

    // A backup holds the stories and none of the call logs, encrypted or not.
    for password in [None, Some(PASSWORD)] {
        let packed = collect_archive(&backup(&root, password), password, en()).unwrap();
        assert_eq!(packed.story_list, stats.story_list);
        assert_eq!(packed.sizes.stories_bytes, stats.sizes.stories_bytes);
        assert_eq!(
            (packed.stories.call_logs, packed.sizes.story_logs_bytes),
            (0, 0)
        );
        assert_eq!(
            packed.stories.unreadable, stats.stories.unreadable,
            "the junk is packed as it lies"
        );
        assert_eq!(packed.fingerprint, stats.fingerprint);
    }
}

#[test]
fn the_text_names_stories_only_where_there_are_some() {
    let root = root();
    let without = render_text(&collect_root(&root.paths, en()).unwrap(), en());
    assert!(!without.contains("Stories"), "{without}");
    assert!(!without.contains("stories"), "{without}");

    seed_story(&root.paths, busy(&root.paths).id);
    let nobody = Uuid::new_v4();
    let orphan = seed_story(&root.paths, nobody);
    let stats = collect_root(&root.paths, en()).unwrap();
    let text = render_text(&stats, en());
    assert!(
        text.contains("2 (deleted: 0; chapters: 2 accepted of 4; unanswered questions: 2)"),
        "{text}"
    );
    assert!(text.contains("; stories "), "{text}");
    assert!(text.contains("; story call logs "), "{text}");
    assert!(text.contains(&format!("{nobody}/{orphan}")), "{text}");
    for lang in Lang::ALL {
        let text = render_text(&stats, locale(*lang));
        assert!(!text.contains(['{', '}']), "{lang:?}: {text}");
    }
}

#[test]
fn a_story_on_both_sides_is_the_same_until_one_side_continues_it() {
    let (pair, chat, story) = story_pair();
    let c = pair.compared();
    assert_eq!(shape(&c.stories), [1, 0, 0, 0, 0, 0]);
    assert_eq!(c.verdict, Verdict::Identical);
    let (here, there) = pair.fingerprints();
    assert_eq!(here, there);

    answer(&pair.there, chat, story, Decision::Approve);
    let c = pair.compared();
    assert_eq!(shape(&c.stories), [0, 0, 0, 0, 1, 0]);
    // The answer and the acceptance it carries.
    assert_eq!(
        (
            c.stories.ahead_there[0].here,
            c.stories.ahead_there[0].there
        ),
        (0, 2)
    );
    assert_eq!(c.verdict, Verdict::ThereHasAll);
    let (here, there) = pair.fingerprints();
    assert_ne!(here, there);
}

/// Two machines answering one question each their own way write one id with
/// two contents: neither copy holds the other.
#[test]
fn a_story_answered_apart_on_two_machines_has_diverged() {
    let (pair, chat, story) = story_pair();
    answer(&pair.here.paths, chat, story, Decision::Approve);
    answer(&pair.there, chat, story, Decision::Change);
    let c = pair.compared();
    assert_eq!(shape(&c.stories), [0, 0, 0, 0, 0, 1]);
    assert_eq!(c.verdict, Verdict::EachHasSomething);
    let text = render_comparison_text(&c, en());
    assert!(text.contains("Stories, diverged"), "{text}");
    assert!(text.contains(&format!("{chat}/{story}")), "{text}");
}

#[test]
fn a_story_on_one_side_only_is_listed_with_its_chat() {
    let (pair, chat, _) = story_pair();
    let more = seed_story(&pair.here.paths, chat);
    let c = pair.compared();
    assert_eq!(shape(&c.stories), [1, 1, 0, 0, 0, 0]);
    assert_eq!(c.stories.only_here[0].id, more);
    assert_eq!(c.verdict, Verdict::HereHasAll);
    let text = render_comparison_text(&c, en());
    assert!(text.contains("Stories: same: 1; only here: 1"), "{text}");
    assert!(text.contains("chapters accepted: 1  busy"), "{text}");
}

/// A snapshot carries the stories, and one made before them says nothing: it
/// is compared all the same, with the stories left out and said so when the
/// side that was read holds some.
#[test]
fn a_snapshot_carries_the_stories_and_one_from_before_them_is_a_caveat() {
    let (pair, _, _) = story_pair();
    let here = collect_root(&pair.here.paths, en()).unwrap();
    let file = pair.here.dir.path().join("there.json");
    fs::write(
        &file,
        render_json(&collect_root(&pair.there, en()).unwrap()),
    )
    .unwrap();
    let there = read_snapshot(&file, en()).unwrap();
    assert_eq!(compare(&here, &there, None).verdict, Verdict::Identical);

    let mut before: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    for field in ["stories", "story_list"] {
        before.as_object_mut().unwrap().remove(field);
    }
    for field in ["stories_bytes", "story_logs_bytes"] {
        before["sizes"].as_object_mut().unwrap().remove(field);
    }
    fs::write(&file, before.to_string()).unwrap();
    let old: DataStats = read_snapshot(&file, en()).unwrap();
    assert_eq!(old.story_list, None);
    let c = compare(&here, &old, None);
    let about_stories = |c: &super::Comparison| -> Vec<Caveat> {
        // The fixture's corrupt chat file brings caveats of its own.
        c.caveats
            .iter()
            .filter(|caveat| !matches!(caveat, Caveat::UnreadableChats { .. }))
            .cloned()
            .collect()
    };
    assert_eq!(
        about_stories(&c),
        vec![Caveat::StoriesNotCompared { side: Side::There }]
    );
    assert_eq!(shape(&c.stories), [0; 6]);

    // Neither side showing a story: nothing to say.
    let bare = root();
    let mut without = collect_root(&bare.paths, en()).unwrap();
    let theirs = without.clone();
    without.story_list = None;
    assert_eq!(about_stories(&compare(&theirs, &without, None)), vec![]);
}

/// A copy with no stories keeps the fingerprint a version before stories
/// gave it: two machines on either side of the update still compare by one
/// line.
#[test]
fn a_copy_without_stories_keeps_its_fingerprint() {
    let root = root();
    let stats = collect_root(&root.paths, en()).unwrap();
    assert_eq!(stats.story_list, Some(Vec::new()));
    let chats: Vec<_> = stats.chat_list.iter().map(|row| row.identity()).collect();
    let rows: Vec<Vec<(&str, &str)>> = super::super::keyed_lists(&stats.database)
        .unwrap()
        .iter()
        .map(|list| {
            list.iter()
                .map(|row| (row.key.as_str(), row.digest.as_str()))
                .collect()
        })
        .collect();
    let material = serde_json::to_vec(&(chats, rows)).unwrap();
    let mut before = crate::entities::chat_file::sha256_hex(&material);
    before.truncate(16);
    assert_eq!(stats.fingerprint, before);
}

/// Writes the fixture root with two stories, one in a deleted chat, into
/// `MINDFORK_STORY_DATA_OUT`: the data the previous release is run on, by its
/// binary, for a backup that holds stories (docs/fabula.md §4, PR 7;
/// docs/lessons.md §8). Skipped without the variable.
#[test]
#[ignore]
fn write_a_data_root_with_stories() {
    let Ok(out) = std::env::var("MINDFORK_STORY_DATA_OUT") else {
        return;
    };
    let root = root();
    let chat = busy(&root.paths).id;
    let first = seed_story(&root.paths, chat);
    seed_story(&root.paths, hidden(&root.paths).id);
    answer(&root.paths, chat, first, Decision::Approve);
    copy_tree(root.paths.root(), std::path::Path::new(&out));
    eprintln!("wrote a data root with two stories into {out}");
}

/// A copy holding a story and nothing else is not an empty copy.
#[test]
fn a_root_with_only_a_story_is_not_empty() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::with_root(dir.path().join("data"));
    seed_story(&paths, Uuid::new_v4());
    let stats = collect_root(&paths, en()).unwrap();
    assert!(!stats.is_empty());
    assert!(render_text(&stats, en()).contains("Stories"));
}

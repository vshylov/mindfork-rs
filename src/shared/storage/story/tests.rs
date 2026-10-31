//! The story database against the harbour fixture (docs/fabula.md §4, PR 1):
//! what the schema refuses, the single writer, the branch path, the canon's
//! versions, the run log, and the files' versions.

use std::path::PathBuf;

use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::entities::story::{
    Action, Cause, Effect, EventKind, LocationKey, NewEvent, SceneSetup, Visibility, fixture_canon,
};

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

fn c(key: &str) -> CharacterKey {
    CharacterKey::from(key)
}

/// The opening of the first scene: everyone is placed at the harbour.
fn opening() -> NewEvent {
    NewEvent {
        round: 0,
        tick: 0,
        location: LocationKey::from("harbour"),
        kind: EventKind::Opening,
        actor: None,
        visibility: Visibility::Public,
        observable: "The storm closes the harbour; the last boats come in.".into(),
        private: None,
        effects: vec![
            Effect::Place {
                character: c("silas"),
                to: "harbour".into(),
            },
            Effect::Place {
                character: c("ines"),
                to: "harbour".into(),
            },
        ],
        proposal: None,
        cause: Cause::Setup,
    }
}

fn speech(actor: &str, tick: u32, line: &str) -> NewEvent {
    NewEvent {
        round: 1,
        tick,
        location: LocationKey::from("harbour"),
        kind: EventKind::Speech,
        actor: Some(c(actor)),
        visibility: Visibility::Public,
        observable: line.into(),
        private: Some("A thought of their own.".into()),
        effects: Vec::new(),
        proposal: None,
        cause: Cause::Initiative,
    }
}

fn setup() -> SceneSetup {
    SceneSetup {
        location: "harbour".into(),
        present: vec![c("mara"), c("silas"), c("ines")],
        just_happened: "The storm closed the harbour.".into(),
        pressure: "The crates must move before dawn.".into(),
    }
}

fn ids(events: &[Event]) -> Vec<&str> {
    events.iter().map(|e| e.id.as_str()).collect()
}

/// Genesis with the opening committed, and one scene of chapter 1.
async fn with_genesis() -> (Story, AuthorWriter, BranchWriter, SceneId) {
    let (story, mut author) = with_canon().await;
    let mut genesis = story.db.genesis().await.unwrap();
    genesis.commit(opening()).await.unwrap();
    let scene = author
        .add_scene((1, 1, 0), &setup(), "Mara cracks first.")
        .await
        .unwrap();
    (story, author, genesis, scene)
}

// ---------- what the schema refuses ----------

#[tokio::test]
async fn update_and_delete_on_the_event_log_are_refused() {
    let (story, _author, _genesis, _scene) = with_genesis().await;
    for sql in [
        "UPDATE event SET observable = 'Nothing happened.'",
        "DELETE FROM event",
        "UPDATE event_witness SET character_key = 'mara'",
        "DELETE FROM event_witness",
        "UPDATE canon_doc SET body_json = NULL",
        "DELETE FROM canon_doc",
        "DELETE FROM scene",
        "UPDATE branch SET seed = 7",
    ] {
        let err = story.db.execute_raw(sql).await.unwrap_err();
        assert!(err.to_string().contains("is append-only"), "{sql}: {err}");
    }
    let path = story.db.path(&BranchId::genesis()).await.unwrap();
    assert_eq!(ids(&path), ["genesis/e1"]);
    assert_eq!(path[0].body, opening());
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
    // `step` stays writable: a failed step is finished by a resume.
    let conn = rusqlite::Connection::open(story.story_path()).unwrap();
    let n: i64 = conn.query_row(&count("step"), [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
}

// ---------- the single writer ----------

#[tokio::test]
async fn a_second_branch_writer_is_refused_until_the_first_is_dropped() {
    let (story, _author, genesis, _scene) = with_genesis().await;
    let err = story
        .db
        .branch_writer(&BranchId::genesis())
        .await
        .err()
        .unwrap();
    assert!(matches!(err, StoryDbError::WriterHeld(_)), "{err}");
    assert_eq!(
        err.to_string(),
        "the writer of branch genesis is held already"
    );
    drop(genesis);
    let mut again = story.db.branch_writer(&BranchId::genesis()).await.unwrap();
    assert_eq!(again.branch(), &BranchId::genesis());
    again
        .commit(speech("mara", 1, "Gate's shut."))
        .await
        .unwrap();
    let err = story
        .db
        .branch_writer(&BranchId::from("nowhere"))
        .await
        .err()
        .unwrap();
    assert!(matches!(err, StoryDbError::UnknownBranch(_)), "{err}");
}

#[tokio::test]
async fn every_store_has_one_writer_at_a_time() {
    let (story, author, genesis, _scene) = with_genesis().await;
    let held = |r: Result<(), StoryDbError>| matches!(r, Err(StoryDbError::WriterHeld(_)));
    assert!(held(story.db.author_writer().await.map(drop)));
    drop(author);
    assert!(story.db.author_writer().await.is_ok());

    let run_log = story.db.run_log_writer().await.unwrap();
    assert!(held(story.db.run_log_writer().await.map(drop)));
    drop(run_log);
    assert!(story.db.run_log_writer().await.is_ok());

    let reflections = story
        .db
        .reflection_writer(genesis.branch(), &c("mara"))
        .await
        .unwrap();
    assert!(held(
        story
            .db
            .reflection_writer(genesis.branch(), &c("mara"))
            .await
            .map(drop)
    ));
    // Another character's reflections are another store.
    assert!(
        story
            .db
            .reflection_writer(genesis.branch(), &c("ines"))
            .await
            .is_ok()
    );
    drop(reflections);
    assert!(
        story
            .db
            .reflection_writer(genesis.branch(), &c("mara"))
            .await
            .is_ok()
    );
    let err = story
        .db
        .reflection_writer(genesis.branch(), &c("nobody"))
        .await
        .err()
        .unwrap();
    assert!(
        matches!(err, StoryDbError::Refused(Refusal::UnknownCharacter(_))),
        "{err}"
    );
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
    let (story, author, genesis, _scene) = with_genesis().await;
    let other = story.db.clone();
    drop((author, genesis));
    story.db.clone().close().await.unwrap();
    assert!(matches!(other.canon().await, Err(StoryDbError::Closed)));
    let reopened = StoryDb::open(&story.story_path(), &story.calls_path())
        .await
        .unwrap();
    assert_eq!(reopened.canon().await.unwrap(), Some(fixture_canon()));
    assert_eq!(
        ids(&reopened.path(&BranchId::genesis()).await.unwrap()),
        ["genesis/e1"]
    );
    reopened.close().await.unwrap();
}

// ---------- the branch path ----------

#[tokio::test]
async fn a_branch_never_sees_a_siblings_events() {
    let (story, _author, _genesis, scene) = with_genesis().await;
    let genesis = BranchId::genesis();
    let mut k0 = story.db.fork(&scene, &genesis, 0, 11).await.unwrap();
    let mut k1 = story.db.fork(&scene, &genesis, 1, 12).await.unwrap();
    assert_eq!(k0.branch().as_str(), "ch1/sc1/r0/k0");
    k0.commit(speech("mara", 1, "Not tonight.")).await.unwrap();
    let walk = NewEvent {
        kind: EventKind::Movement,
        effects: vec![Effect::Move {
            character: c("ines"),
            to: "lighthouse".into(),
        }],
        ..speech("ines", 1, "Ines walks back towards the lighthouse.")
    };
    k1.commit(walk).await.unwrap();
    k1.commit(speech("silas", 2, "A fine night for it."))
        .await
        .unwrap();

    let path0 = story.db.path(k0.branch()).await.unwrap();
    let path1 = story.db.path(k1.branch()).await.unwrap();
    assert_eq!(ids(&path0), ["genesis/e1", "ch1/sc1/r0/k0/e1"]);
    assert_eq!(
        ids(&path1),
        ["genesis/e1", "ch1/sc1/r0/k1/e1", "ch1/sc1/r0/k1/e2"]
    );
    // The world on each branch follows its own events alone.
    let world0 = story.db.world(k0.branch()).await.unwrap();
    let world1 = story.db.world(k1.branch()).await.unwrap();
    assert_eq!(world0.position(&c("ines")), Some(&"harbour".into()));
    assert_eq!(world1.position(&c("ines")), Some(&"lighthouse".into()));
    // Silas's line on k1 was said after Ines left: she never heard it, and on
    // k0 there is nothing of k1's to hear.
    let heard = story.db.witnessed(k1.branch(), &c("ines")).await.unwrap();
    assert_eq!(ids(&heard), ["genesis/e1", "ch1/sc1/r0/k1/e1"]);
    let heard = story.db.witnessed(k0.branch(), &c("ines")).await.unwrap();
    assert_eq!(ids(&heard), ["genesis/e1", "ch1/sc1/r0/k0/e1"]);

    let info = story.db.branch(k1.branch()).await.unwrap().unwrap();
    assert_eq!(
        info,
        BranchInfo {
            id: k1.branch().clone(),
            parent: Some(genesis.clone()),
            fork_seq: Some(1),
            scene: Some(scene.clone()),
            k: 1,
            seed: 12,
        }
    );
    assert_eq!(
        story.db.branch(&BranchId::from("none")).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn a_fork_inherits_its_parent_up_to_the_fork_and_no_further() {
    let (story, mut author, mut genesis, scene) = with_genesis().await;
    let mut k0 = story.db.fork(&scene, genesis.branch(), 0, 1).await.unwrap();
    // The parent goes on after the fork; the child does not inherit that.
    genesis.commit(speech("silas", 1, "Late.")).await.unwrap();
    k0.commit(speech("mara", 1, "Gate's shut.")).await.unwrap();
    assert_eq!(
        ids(&story.db.path(k0.branch()).await.unwrap()),
        ["genesis/e1", "ch1/sc1/r0/k0/e1"]
    );
    // A grandchild forked from k0's tip sees genesis up to k0's fork, all of
    // k0, then its own.
    let next = author.add_scene((1, 2, 0), &setup(), "").await.unwrap();
    let mut child = story.db.fork(&next, k0.branch(), 0, 2).await.unwrap();
    k0.commit(speech("ines", 2, "Who's there?")).await.unwrap();
    child.commit(speech("ines", 2, "Hello?")).await.unwrap();
    assert_eq!(
        ids(&story.db.path(child.branch()).await.unwrap()),
        ["genesis/e1", "ch1/sc1/r0/k0/e1", "ch1/sc2/r0/k0/e1"]
    );
    // The view's order is the commit order within the path.
    let seqs: Vec<u64> = story
        .db
        .path(child.branch())
        .await
        .unwrap()
        .iter()
        .map(|e| e.seq)
        .collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");
}

#[tokio::test]
async fn forks_need_their_scene_and_parent_and_are_made_once() {
    let (story, _author, genesis, scene) = with_genesis().await;
    let refused = |r: Result<BranchWriter, StoryDbError>| r.err().unwrap();
    let err = refused(
        story
            .db
            .fork(&SceneId::of(9, 9, 0), genesis.branch(), 0, 0)
            .await,
    );
    assert!(matches!(err, StoryDbError::UnknownScene(_)), "{err}");
    let err = refused(story.db.fork(&scene, &BranchId::from("x"), 0, 0).await);
    assert!(matches!(err, StoryDbError::UnknownBranch(_)), "{err}");
    let k0 = story.db.fork(&scene, genesis.branch(), 0, 0).await.unwrap();
    drop(k0);
    let err = refused(story.db.fork(&scene, genesis.branch(), 0, 0).await);
    assert!(matches!(err, StoryDbError::BranchExists(_)), "{err}");
    let err = refused(story.db.genesis().await);
    assert!(matches!(err, StoryDbError::BranchExists(_)), "{err}");
}

// ---------- commits ----------

#[tokio::test]
async fn a_commit_is_checked_against_its_branch_and_stores_its_witnesses() {
    let (story, _author, mut genesis, _scene) = with_genesis().await;
    let committed = genesis
        .commit(speech("mara", 1, "Gate's shut."))
        .await
        .unwrap();
    assert_eq!(committed.id.as_str(), "genesis/e2");
    let names: Vec<&str> = committed.witnesses.iter().map(|w| w.as_str()).collect();
    assert_eq!(names, ["ines", "mara", "silas"]);
    let path = story.db.path(genesis.branch()).await.unwrap();
    assert_eq!(path[1].witnesses, committed.witnesses);
    assert_eq!(path[1].seq, committed.seq);
    assert_eq!(path[1].body, speech("mara", 1, "Gate's shut."));

    // Ines is at the harbour, so a speech of hers at the inn is refused, and
    // nothing is stored.
    let elsewhere = NewEvent {
        location: "inn".into(),
        ..speech("ines", 2, "Hello?")
    };
    let err = genesis.commit(elsewhere).await.unwrap_err();
    assert_eq!(err.to_string(), "refused: ines is at harbour, not at inn");
    let backwards = speech("mara", 0, "Earlier.");
    let err = genesis.commit(backwards).await.unwrap_err();
    assert!(
        matches!(
            err,
            StoryDbError::Refused(Refusal::TimeRunsBackwards { .. })
        ),
        "{err}"
    );
    assert_eq!(story.db.path(genesis.branch()).await.unwrap().len(), 2);
}

#[tokio::test]
async fn a_commit_names_its_cause_and_proposal_on_its_own_branch() {
    let (story, _author, genesis, scene) = with_genesis().await;
    let mut k0 = story.db.fork(&scene, genesis.branch(), 0, 0).await.unwrap();
    let mut k1 = story.db.fork(&scene, genesis.branch(), 1, 0).await.unwrap();
    let proposal = Proposal {
        private_thought: "If I say nothing, she will ask again.".into(),
        action: Action::Speak {
            line: "Gate's shut.".into(),
        },
        targets: vec![c("ines")],
        visibility: Visibility::Public,
    };
    let id = k0
        .record_proposal(1, &c("mara"), &proposal, ProposalVerdict::Committed, None)
        .await
        .unwrap();
    assert_eq!(id.as_str(), "ch1/sc1/r0/k0/p1");
    let lapsed = k0
        .record_proposal(
            1,
            &c("silas"),
            &Proposal {
                action: Action::Wait,
                ..proposal.clone()
            },
            ProposalVerdict::Lapsed,
            Some("not the initiative".into()),
        )
        .await
        .unwrap();
    assert_eq!(lapsed.as_str(), "ch1/sc1/r0/k0/p2");
    let line = NewEvent {
        proposal: Some(id.clone()),
        ..speech("mara", 1, "Gate's shut.")
    };
    let first = k0.commit(line.clone()).await.unwrap();

    // A sibling cannot commit k0's proposal, nor answer k0's event.
    let err = k1.commit(line).await.unwrap_err();
    assert!(
        matches!(err, StoryDbError::ProposalElsewhere { ref branch, .. } if branch == k0.branch()),
        "{err}"
    );
    let reply = NewEvent {
        cause: Cause::Reaction {
            to: first.id.clone(),
        },
        ..speech("ines", 2, "Why?")
    };
    let err = k1.commit(reply.clone()).await.unwrap_err();
    assert!(
        matches!(err, StoryDbError::Refused(Refusal::UnknownEvent(_))),
        "{err}"
    );
    k0.commit(reply).await.unwrap();
    let ghost = NewEvent {
        proposal: Some(ProposalId::from("ch1/sc1/r0/k1/p9")),
        ..speech("ines", 3, "...")
    };
    let err = k1.commit(ghost).await.unwrap_err();
    assert!(matches!(err, StoryDbError::UnknownProposal(_)), "{err}");

    let stored = story.db.proposals(k0.branch()).await.unwrap();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].proposal, proposal);
    assert_eq!(stored[0].verdict, ProposalVerdict::Committed);
    assert_eq!(stored[1].character, c("silas"));
    assert_eq!(stored[1].verdict, ProposalVerdict::Lapsed);
    assert_eq!(stored[1].reason.as_deref(), Some("not the initiative"));
    assert!(story.db.proposals(k1.branch()).await.unwrap().is_empty());
    let err = k1
        .record_proposal(1, &c("nobody"), &proposal, ProposalVerdict::Refused, None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, StoryDbError::Refused(Refusal::UnknownCharacter(_))),
        "{err}"
    );
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
async fn the_canon_is_frozen_once_events_exist_and_needed_before_them() {
    let story = open().await;
    let err = story.db.genesis().await.err().unwrap();
    assert!(matches!(err, StoryDbError::NoCanon), "{err}");
    let mut author = story.db.author_writer().await.unwrap();
    let err = author.add_scene((1, 1, 0), &setup(), "").await.unwrap_err();
    assert!(matches!(err, StoryDbError::NoCanon), "{err}");
    author.import_canon(&fixture_canon()).await.unwrap();
    let mut genesis = story.db.genesis().await.unwrap();
    // Genesis exists but has no event yet: the canon may still change.
    let mut edited = fixture_canon();
    edited.world.rules.push("Nobody swims in winter.".into());
    assert_eq!(author.import_canon(&edited).await.unwrap(), 1);
    genesis.commit(opening()).await.unwrap();
    let err = author.import_canon(&fixture_canon()).await.unwrap_err();
    assert!(matches!(err, StoryDbError::CanonFrozen), "{err}");
}

// ---------- scenes, picks and the main line ----------

#[tokio::test]
async fn a_scene_names_only_what_the_canon_has() {
    let (_story, mut author) = with_canon().await;
    let id = author
        .add_scene((2, 3, 1), &setup(), "intent")
        .await
        .unwrap();
    assert_eq!(id, SceneId::of(2, 3, 1));
    let err = author.add_scene((2, 3, 1), &setup(), "").await.unwrap_err();
    assert!(matches!(err, StoryDbError::SceneExists(_)), "{err}");
    let nowhere = SceneSetup {
        location: "moon".into(),
        ..setup()
    };
    let err = author.add_scene((1, 1, 0), &nowhere, "").await.unwrap_err();
    assert!(
        matches!(err, StoryDbError::Refused(Refusal::UnknownLocation(_))),
        "{err}"
    );
    let stranger = SceneSetup {
        present: vec![c("mara"), c("nobody")],
        ..setup()
    };
    let err = author
        .add_scene((1, 1, 0), &stranger, "")
        .await
        .unwrap_err();
    assert!(
        matches!(err, StoryDbError::Refused(Refusal::UnknownCharacter(_))),
        "{err}"
    );
}

#[tokio::test]
async fn the_main_line_follows_the_picks() {
    let story = open().await;
    assert_eq!(story.db.main_line_tip().await.unwrap(), None);
    let (story, mut author, genesis, scene) = {
        drop(story);
        with_genesis().await
    };
    assert_eq!(
        story.db.main_line_tip().await.unwrap(),
        Some(BranchId::genesis())
    );
    let k0 = story.db.fork(&scene, genesis.branch(), 0, 0).await.unwrap();
    let k1 = story.db.fork(&scene, genesis.branch(), 1, 0).await.unwrap();
    // A rerun is asked for: the main line does not move.
    author.record_pick(&scene, None, "flat").await.unwrap();
    assert_eq!(
        story.db.main_line_tip().await.unwrap(),
        Some(BranchId::genesis())
    );
    let err = author
        .record_pick(&scene, Some(k1.branch()), "")
        .await
        .unwrap_err();
    assert!(matches!(err, StoryDbError::PickExists(_)), "{err}");

    let rerun = author.add_scene((1, 1, 1), &setup(), "").await.unwrap();
    let r0 = story.db.fork(&rerun, genesis.branch(), 0, 0).await.unwrap();
    let err = author
        .record_pick(&rerun, Some(k0.branch()), "")
        .await
        .unwrap_err();
    assert!(matches!(err, StoryDbError::PickElsewhere { .. }), "{err}");
    let err = author
        .record_pick(&rerun, Some(&BranchId::from("x")), "")
        .await
        .unwrap_err();
    assert!(matches!(err, StoryDbError::UnknownBranch(_)), "{err}");
    author
        .record_pick(&rerun, Some(r0.branch()), "tension")
        .await
        .unwrap();
    assert_eq!(
        story.db.main_line_tip().await.unwrap(),
        Some(r0.branch().clone())
    );
    let err = author
        .record_pick(&SceneId::of(5, 5, 5), None, "")
        .await
        .unwrap_err();
    assert!(matches!(err, StoryDbError::UnknownScene(_)), "{err}");
}

// ---------- reflections ----------

#[tokio::test]
async fn reflections_follow_the_path_like_events() {
    let (story, mut author, genesis, scene) = with_genesis().await;
    let mut k0 = story.db.fork(&scene, genesis.branch(), 0, 0).await.unwrap();
    let k1 = story.db.fork(&scene, genesis.branch(), 1, 0).await.unwrap();
    let mara = c("mara");
    let mut on_k0 = story
        .db
        .reflection_writer(k0.branch(), &mara)
        .await
        .unwrap();
    let first = on_k0
        .write("She will tell the keeper.", &["Silence Ines".into()])
        .await
        .unwrap();
    assert_eq!(first, "ch1/sc1/r0/k0/mara/f1");
    k0.commit(speech("mara", 1, "Gate's shut.")).await.unwrap();
    on_k0.write("Too late now.", &[]).await.unwrap();

    let next = author.add_scene((1, 2, 0), &setup(), "").await.unwrap();
    let child = story.db.fork(&next, k0.branch(), 0, 0).await.unwrap();
    let seen = story.db.reflections(child.branch(), &mara).await.unwrap();
    let bodies: Vec<&str> = seen.iter().map(|r| r.body.as_str()).collect();
    assert_eq!(bodies, ["She will tell the keeper.", "Too late now."]);
    assert_eq!(seen[0].after_seq, 1);
    assert_eq!(seen[1].after_seq, 2);
    assert_eq!(seen[0].goal_changes, ["Silence Ines"]);
    assert_eq!(&seen[0].branch, k0.branch());
    assert!(
        story
            .db
            .reflections(k1.branch(), &mara)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        story
            .db
            .reflections(k0.branch(), &c("ines"))
            .await
            .unwrap()
            .is_empty()
    );
}

// ---------- the run log ----------

fn call(run: &RunId, attempt: u32) -> NewCall {
    NewCall {
        run: run.clone(),
        address: "ch1/sc1/r0/k0/round1/mara".into(),
        attempt,
        role: "character".into(),
        model: "qwen/qwen3.8-27b".into(),
        params: json!({"temperature": 0.8}),
        mechanism: Mechanism::Schema,
        request: json!({"messages": [{"role": "user", "content": "Propose."}]}),
        prompt_sha256: "ab".repeat(32),
        response: Some(json!({"action": {"type": "wait"}})),
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
async fn the_run_log_records_steps_and_calls_by_address() {
    let story = open().await;
    let mut log = story.db.run_log_writer().await.unwrap();
    let run = log.start_run(&json!({"branches": 3})).await.unwrap();
    assert_eq!(run, RunId::nth(1));
    assert_eq!(log.start_run(&json!({})).await.unwrap(), RunId::nth(2));

    log.finish_step(&run, "ch1/sc1/r0/k0", StepStatus::Failed)
        .await
        .unwrap();
    log.finish_step(&run, "ch1/sc1/r0/k1", StepStatus::Done)
        .await
        .unwrap();
    // A resume finishes the failed step: it keeps its place.
    log.finish_step(&run, "ch1/sc1/r0/k0", StepStatus::Done)
        .await
        .unwrap();
    assert_eq!(
        story.db.steps(&run).await.unwrap(),
        [
            ("ch1/sc1/r0/k0".to_owned(), StepStatus::Done),
            ("ch1/sc1/r0/k1".to_owned(), StepStatus::Done),
        ]
    );
    let err = log
        .finish_step(&RunId::nth(9), "x", StepStatus::Done)
        .await
        .unwrap_err();
    assert!(matches!(err, StoryDbError::UnknownRun(_)), "{err}");

    let id = log.record_call(call(&run, 1)).await.unwrap();
    assert_eq!(id, actor_call_id(&run, "ch1/sc1/r0/k0/round1/mara", 1));
    let stored = story
        .db
        .call(&run, "ch1/sc1/r0/k0/round1/mara", 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.id, id);
    assert_eq!(stored.call, call(&run, 1));
    assert!(!stored.created_at.is_empty());
    assert_eq!(
        story
            .db
            .call(&run, "ch1/sc1/r0/k0/round1/mara", 2)
            .await
            .unwrap(),
        None
    );
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
    let stored = story
        .db
        .call(&run, "ch1/sc1/r0/k0/round1/mara", 2)
        .await
        .unwrap()
        .unwrap();
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
    for status in [StepStatus::Done, StepStatus::Failed] {
        assert_eq!(StepStatus::parse(status.as_str()), Some(status));
    }
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
    assert_eq!(StepStatus::parse("maybe"), None);
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

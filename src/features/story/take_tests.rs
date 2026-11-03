//! A named scene rerun at P2 on a scripted port (docs/fabula.md §4, PR 10;
//! §6, T8, T12): the chapter's next take carries the scenes before the one
//! named, plays it and the ones after it again under addresses of their own,
//! and writes the chapter anew; a second take steps back further; a take's
//! budget is its own; a take killed at any call resumes to its checkpoint;
//! and a story with a take replays from its addresses alone.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use serde_json::json;
use tokio::sync::Notify;

use super::tests::{Open, Reply, Script, config, paused_at, short, story};
use super::{Budget, ChapterConfig, Outcome, Stop};
use crate::entities::story::{BranchId, ChapterId, Decision, QuestionBody, SceneId, VersionOrigin};
use crate::features::story::port::{PortError, Prompt};
use crate::shared::storage::story::{CallStatus, CallUsage, Mechanism, NewCall, StoryDbError};

/// What the human asked with the rerun, as the reading records it.
const ASKED: &str = "The human asked for these changes:\n- Mara keeps quiet.";

fn checkpoint(version: u32) -> QuestionBody {
    QuestionBody::Chapter {
        chapter: 1,
        version,
    }
}

/// A chapter at its checkpoint: two scenes kept, the draft and its revision.
async fn at_checkpoint(cfg: &ChapterConfig) -> Open {
    let mut open = Open::approved().await;
    let port = Script::new(Arc::new(story));
    let outcome = open.advance(&port, cfg).await;
    assert_eq!(paused_at(&outcome), &checkpoint(2));
    open
}

async fn kept_ids(open: &Open) -> Vec<String> {
    open.db
        .kept_scenes(1)
        .await
        .unwrap()
        .iter()
        .map(|k| k.scene.id.to_string())
        .collect()
}

/// Records a call of `tokens` under chapter 1 in a run of its own.
async fn spend(open: &Open, tokens: u64) {
    let mut log = open.log.lock().await;
    let run = log.start_run(&json!({})).await.unwrap();
    log.record_call(NewCall {
        run,
        address: "ch1/earlier".to_owned(),
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
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_named_scene_is_played_again_with_the_ones_after_it() {
    let mut open = at_checkpoint(&config()).await;
    open.rerun(2, ASKED).await;
    let take = open
        .db
        .take_of(1)
        .await
        .unwrap()
        .expect("the take is stored");
    assert_eq!((take.number, take.from_scene), (2, 2));
    assert_eq!(take.carried, [SceneId::of(1, 1, 0)]);
    assert_eq!(take.replaced, [SceneId::of(1, 2, 0)]);
    assert_eq!(take.floors, BTreeMap::from([(2, 1)]));
    assert_eq!(take.first_version, 3);
    assert_eq!(take.notes, ASKED);
    let answered = open.db.questions().await.unwrap();
    let rerun = answered[1].1.as_ref().unwrap();
    assert_eq!((rerun.decision, rerun.rerun), (Decision::Change, Some(2)));
    // Until it plays, the main line stands on the scene before the one named.
    assert_eq!(kept_ids(&open).await, ["ch1/sc1/r0"]);
    assert_eq!(
        open.db.main_line_tip().await.unwrap().unwrap().as_str(),
        "ch1/sc1/r0/k0"
    );

    let port = Script::new(Arc::new(story));
    let outcome = open.advance(&port, &config()).await;
    assert_eq!(paused_at(&outcome), &checkpoint(3));
    let calls = port.addresses();
    assert!(
        !calls
            .iter()
            .any(|a| a.starts_with("ch1/sc1/") || a == "ch1/outline"),
        "scene 1 and the outline stand: {calls:?}"
    );
    // The named scene is planned again, under the take's own address, with
    // what was asked and its old setup beside it, and may not end the chapter.
    let plan = port.prompt_at("ch1/t2/sc2/plan").user;
    assert!(
        plan.contains("asked for this scene to be played again"),
        "{plan}"
    );
    assert!(plan.contains("Mara keeps quiet."));
    assert!(plan.contains("The scene as it was set before: at Harbour"));
    assert!(plan.contains("Your intent for it then: Scene 2 tightens the screw."));
    assert!(plan.contains("After reading version 2 of this chapter, they asked for scene 2"));
    assert!(
        plan.contains("The scenes this chapter has kept so far:\n\nScene 1, at Harbour"),
        "the scene carried over: {plan}"
    );
    assert!(
        !port
            .schema_at("ch1/t2/sc2/plan")
            .contains(r#""enum":["end"]"#)
    );
    // The scene after it is planned again too, and may end the chapter.
    assert!(
        port.schema_at("ch1/t2/sc3/plan")
            .contains(r#""enum":["end"]"#)
    );
    // Its attempt is past the old one, forked from scene 1's kept branch.
    let scenes: Vec<String> = open
        .db
        .scenes_of(1)
        .await
        .unwrap()
        .iter()
        .map(|s| s.id.to_string())
        .collect();
    assert_eq!(scenes, ["ch1/sc1/r0", "ch1/sc2/r0", "ch1/sc2/r1"]);
    let branch = open
        .db
        .branch(&BranchId::new("ch1/sc2/r1/k0"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(branch.parent.unwrap().as_str(), "ch1/sc1/r0/k0");
    let pick = port.prompt_at("ch1/sc2/r1/pick").user;
    assert!(
        pick.contains("What the story's human author asked of this chapter:")
            && pick.contains("Mara keeps quiet."),
        "{pick}"
    );
    // The take writes the chapter anew: its own draft, reviewed and summed up.
    let versions = open.db.chapter_versions(1).await.unwrap();
    assert_eq!(
        versions.iter().map(|v| v.origin).collect::<Vec<_>>(),
        [
            VersionOrigin::Draft,
            VersionOrigin::Revision,
            VersionOrigin::Draft
        ]
    );
    let write = port.prompt_at("ch1/v3/write").user;
    assert!(write.contains("Mara keeps quiet."));
    assert!(
        write.contains("Scene 1, at Harbour") && write.contains("Scene 2, at Harbour"),
        "the draft is written from the carried scene and the new one: {write}"
    );
    assert!(!open.db.notes(&versions[2].id).await.unwrap().is_empty());
    assert!(open.db.summary(&versions[2].id).await.unwrap().is_some());
    assert_eq!(kept_ids(&open).await, ["ch1/sc1/r0", "ch1/sc2/r1"]);
    assert_eq!(
        open.db.main_line_tip().await.unwrap().unwrap().as_str(),
        "ch1/sc2/r1/k0"
    );
    // Accepted, it is the story's chapter.
    open.answer(Decision::Approve, "").await;
    assert_eq!(open.advance(&port, &config()).await, Outcome::Finished);
    assert!(open.db.accepted(&ChapterId::of(1, 3)).await.unwrap());
}

#[tokio::test]
async fn a_second_take_steps_back_further_and_counts_its_own_attempts() {
    let mut open = at_checkpoint(&config()).await;
    open.rerun(2, ASKED).await;
    let port = Script::new(Arc::new(story));
    open.advance(&port, &config()).await;
    open.rerun(1, "").await;
    let take = open.db.take_of(1).await.unwrap().unwrap();
    assert_eq!((take.number, take.from_scene), (3, 1));
    assert!(take.carried.is_empty());
    assert_eq!(take.replaced, [SceneId::of(1, 1, 0), SceneId::of(1, 2, 1)]);
    assert_eq!(take.floors, BTreeMap::from([(1, 1), (2, 2)]));
    assert_eq!(take.first_version, 4);
    assert_eq!(kept_ids(&open).await, Vec::<String>::new());
    assert_eq!(
        open.db.main_line_tip().await.unwrap(),
        Some(BranchId::genesis()),
        "the chapter steps back to its start"
    );

    let port = Script::new(Arc::new(story));
    let outcome = open.advance(&port, &config()).await;
    assert_eq!(paused_at(&outcome), &checkpoint(4));
    let plan = port.prompt_at("ch1/t3/sc1/plan").user;
    assert!(
        plan.contains("They asked for no change in particular."),
        "{plan}"
    );
    // Every request on the chapter so far reaches the take.
    assert!(plan.contains("After reading version 2 of this chapter, they asked for scene 2"));
    assert!(plan.contains("After reading version 3 of this chapter, they asked for scene 1"));
    assert_eq!(kept_ids(&open).await, ["ch1/sc1/r1", "ch1/sc2/r2"]);
    let first = open
        .db
        .branch(&BranchId::new("ch1/sc1/r1/k0"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.parent, Some(BranchId::genesis()));
    let second = open
        .db
        .branch(&BranchId::new("ch1/sc2/r2/k0"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.parent.unwrap().as_str(), "ch1/sc1/r1/k0");
    port.prompt_at("ch1/v4/write");
}

#[tokio::test]
async fn a_rerun_names_a_scene_the_latest_version_keeps_or_stores_nothing() {
    let mut open = at_checkpoint(&config()).await;
    for (decision, scene) in [
        (Decision::Change, 3),
        (Decision::Change, 0),
        (Decision::Approve, 1),
    ] {
        let err = open
            .resolve_with(decision, "", Some(scene))
            .await
            .unwrap_err();
        assert!(matches!(err, StoryDbError::BadRerun { .. }), "{err}");
    }
    let pending = open.db.pending_question().await.unwrap().unwrap();
    assert_eq!(pending.body, checkpoint(2), "still waiting");
    assert_eq!(open.db.take_of(1).await.unwrap(), None);
    open.rerun(1, "").await;
    // The take's draft comes first: no revision before it.
    let err = open
        .author
        .add_chapter(1, VersionOrigin::Revision, "x")
        .await
        .unwrap_err();
    assert!(matches!(err, StoryDbError::OutOfOrder(_)), "{err}");
    // The canon's question plays no scene.
    let mut fresh = Open::open_only().await;
    let port = Script::new(Arc::new(story));
    fresh.advance(&port, &config()).await;
    let err = fresh
        .resolve_with(Decision::Change, "", Some(1))
        .await
        .unwrap_err();
    assert!(matches!(err, StoryDbError::BadRerun { .. }), "{err}");
}

/// The chapter's budget is spent when the human asks for a scene again: a
/// revision would stop at its first call, a take starts with a budget of its
/// own (§6, T12), and counts the calls made after it began.
#[tokio::test]
async fn a_take_has_a_budget_of_its_own() {
    let tight = ChapterConfig {
        budget: Budget {
            tokens: Some(3_000),
            usd: None,
        },
        ..config()
    };
    let mut revised = at_checkpoint(&config()).await;
    spend(&revised, 5_000).await;
    revised.answer(Decision::Change, "Shorter.").await;
    let port = Script::new(Arc::new(story));
    let outcome = revised.advance(&port, &tight).await;
    assert_eq!(
        outcome,
        Outcome::Stopped {
            step: "ch1/v3/revise".to_owned(),
            why: Stop::Budget("5000 tokens of 3000".to_owned()),
        }
    );

    let mut taken = at_checkpoint(&config()).await;
    spend(&taken, 5_000).await;
    taken.rerun(2, ASKED).await;
    assert_eq!(taken.db.chapter_spend(1).await.unwrap().tokens(), 0);
    let outcome = taken.advance(&port, &tight).await;
    assert_eq!(paused_at(&outcome), &checkpoint(3));
    assert_eq!(taken.db.spend("ch1/").await.unwrap().tokens(), 5_000);
    spend(&taken, 4_000).await;
    assert_eq!(taken.db.chapter_spend(1).await.unwrap().tokens(), 4_000);
}

#[tokio::test]
async fn a_take_killed_at_any_call_resumes_to_its_checkpoint() {
    let total = {
        let mut open = at_checkpoint(&short()).await;
        open.rerun(2, ASKED).await;
        let port = Script::new(Arc::new(story));
        open.advance(&port, &short()).await;
        port.addresses().len()
    };
    assert!(total > 10, "{total} calls");
    futures_util::stream::iter(1..=total)
        .map(killed_and_resumed)
        .buffer_unordered(16)
        .collect::<Vec<()>>()
        .await;
}

async fn killed_and_resumed(cut: usize) {
    let cfg = short();
    let mut open = at_checkpoint(&cfg).await;
    open.rerun(2, ASKED).await;
    let blown = Arc::new(Notify::new());
    let fused = Script::fused(Arc::new(story), cut, blown.clone());
    tokio::select! {
        outcome = open.advance(&fused, &cfg) => panic!("cut {cut} ran through: {outcome:?}"),
        () = blown.notified() => {}
    }
    let port = Script::new(Arc::new(story));
    let outcome = open.advance(&port, &cfg).await;
    assert_eq!(paused_at(&outcome), &checkpoint(3), "cut at call {cut}");
    assert_eq!(open.db.questions().await.unwrap().len(), 3, "cut {cut}");
    assert_eq!(
        kept_ids(&open).await,
        ["ch1/sc1/r0", "ch1/sc2/r1"],
        "cut {cut}: the take keeps its first attempt"
    );
    let versions = open.db.chapter_versions(1).await.unwrap();
    assert_eq!(versions.len(), 3, "cut {cut}");
    assert_eq!(
        open.db.notes(&versions[2].id).await.unwrap().len(),
        4,
        "cut {cut}: reviewed once"
    );
    assert!(open.db.summary(&versions[2].id).await.unwrap().is_some());
}

/// Every call of a story that went through a take is at an address of its
/// own, so the story replays from what was recorded at each address, with
/// the same prompts, into the same rows.
#[tokio::test]
async fn a_story_with_a_take_replays_from_its_addresses_alone() {
    let cfg = config();
    let live = Script::new(Arc::new(story));
    let recorded = through_a_take(&live, &cfg).await;
    let replies = live.replies();
    let mut seen = BTreeSet::new();
    for (address, _) in &replies {
        assert!(seen.insert(address.clone()), "{address} was asked twice");
    }
    assert!(seen.iter().any(|a| a.starts_with("ch1/t2/")));

    let served: Arc<Mutex<BTreeMap<String, Reply>>> =
        Arc::new(Mutex::new(replies.into_iter().collect()));
    let left = served.clone();
    let replay = Script::new(Arc::new(
        move |address: &str, _: &Prompt| -> Result<Reply, PortError> {
            Ok(served
                .lock()
                .unwrap()
                .remove(address)
                .unwrap_or_else(|| panic!("{address} was never recorded")))
        },
    ));
    let replayed = through_a_take(&replay, &cfg).await;
    assert!(left.lock().unwrap().is_empty(), "every reply is served");
    let prompts = |s: &Script| -> BTreeMap<String, Prompt> { s.prompts().into_iter().collect() };
    assert_eq!(prompts(&replay), prompts(&live), "no prompt drifted");
    assert_eq!(replayed, recorded);
}

/// A story taken through P1, its chapter, a rerun of scene 2 and the take,
/// as rows: the scenes, their picks, the versions and the take.
async fn through_a_take(port: &Script, cfg: &ChapterConfig) -> Vec<String> {
    let mut open = Open::approved().await;
    open.advance(port, cfg).await;
    open.rerun(2, ASKED).await;
    let outcome = open.advance(port, cfg).await;
    assert_eq!(paused_at(&outcome), &checkpoint(3));
    let mut rows = Vec::new();
    for scene in open.db.scenes_of(1).await.unwrap() {
        let pick = open.db.pick_of(&scene.id).await.unwrap();
        rows.push(format!("{} {:?} {pick:?}", scene.id, scene.setup));
    }
    for version in open.db.chapter_versions(1).await.unwrap() {
        let summary = open.db.summary(&version.id).await.unwrap();
        rows.push(format!(
            "{} {:?} {} {summary:?}",
            version.id, version.origin, version.text
        ));
    }
    rows.push(format!("{:?}", open.db.take_of(1).await.unwrap()));
    rows
}

//! M3's systems in the chapter's loop, on a scripted port (docs/fabula.md §4,
//! PR 13; §6, T22): A1 sets each scene's opening and plays nothing, and its
//! Author invents what happens; A3 reviews nothing. Each runs to its end
//! through the evaluation runner's answers to the pauses.

use std::sync::Arc;

use super::tests::{Open, Script, config, story};
use crate::entities::story::{BranchId, EventKind};
use crate::features::story::eval::{Ending, chapter_story};

async fn to_the_end(open: &mut Open, port: &Script, config: &super::ChapterConfig) -> Ending {
    chapter_story(&open.db, &mut open.author, &open.log, port, config)
        .await
        .unwrap()
}

#[tokio::test]
async fn a1_plans_the_scenes_and_plays_none_of_them() {
    let mut open = Open::open_only().await;
    let port = Script::new(Arc::new(story));
    let mut a1 = config();
    a1.simulate = false;
    let ending = to_the_end(&mut open, &port, &a1).await;
    assert_eq!(ending, Ending::Finished("accepted".to_owned()));

    let addresses = port.addresses();
    for played in [
        "/propose",
        "/react",
        "/initiative",
        "/reactions",
        "/reflect",
        "/pick",
    ] {
        assert!(
            !addresses.iter().any(|a| a.ends_with(played)),
            "{played} in {addresses:?}"
        );
    }
    // Each scene kept one branch holding its opening alone.
    let scenes = open.db.scenes_of(1).await.unwrap();
    assert_eq!(scenes.len(), 2);
    for scene in &scenes {
        let branch = BranchId::of_scene(&scene.id, 0);
        let pick = open.db.pick_of(&scene.id).await.unwrap().unwrap();
        assert_eq!(pick.branch, Some(branch.clone()));
        assert!(
            open.db
                .branch(&BranchId::of_scene(&scene.id, 1))
                .await
                .unwrap()
                .is_none()
        );
        let own: Vec<_> = open
            .db
            .path(&branch)
            .await
            .unwrap()
            .into_iter()
            .filter(|e| e.branch == branch)
            .collect();
        assert!(!own.is_empty());
        assert!(
            own.iter().all(|e| e.body.kind == EventKind::Opening),
            "{own:?}"
        );
    }
    // The Author is told no one plays the scenes, and invents them.
    let write = port.prompt_at("ch1/v1/write");
    assert!(write.system.contains("invent what happens in it"));
    assert!(write.system.contains("No one plays the scenes"));
    assert!(!write.system.contains("Every scene is played several times"));
    assert!(write.user.contains("Not played: only its opening is set"));
    assert!(
        write
            .user
            .contains("where a scene was not played invent what happens")
    );
    let plan = port.prompt_at("ch1/sc2/plan");
    assert!(plan.user.contains("Scene 1, at"));
    assert!(plan.user.contains("Not played"));
    let outline = port.prompt_at("ch1/outline");
    assert!(outline.user.contains("each after the one before it."));
    // The checker holds the chapter to the openings it has.
    let checker = port.prompt_at("ch1/v1/continuity");
    assert!(checker.user.contains("the record holds its opening alone"));
    assert!(checker.user.contains("Where a scene was not played"));
}

#[tokio::test]
async fn f_keeps_its_played_scenes_and_its_words() {
    let mut open = Open::open_only().await;
    let port = Script::new(Arc::new(story));
    let ending = to_the_end(&mut open, &port, &config()).await;
    assert_eq!(ending, Ending::Finished("accepted".to_owned()));
    let addresses = port.addresses();
    assert!(addresses.iter().any(|a| a.ends_with("/pick")));
    let write = port.prompt_at("ch1/v1/write");
    assert!(write.system.contains("Every scene is played several times"));
    assert!(!write.user.contains("Not played"));
    assert!(
        !port
            .prompt_at("ch1/v1/continuity")
            .user
            .contains("not played")
    );
}

#[tokio::test]
async fn a3_reviews_nothing_and_still_keeps_its_register() {
    let mut open = Open::open_only().await;
    let port = Script::new(Arc::new(story));
    let mut a3 = config();
    a3.review = false;
    let ending = to_the_end(&mut open, &port, &a3).await;
    assert_eq!(ending, Ending::Finished("accepted".to_owned()));
    let addresses = port.addresses();
    for reviewed in [
        "/reader",
        "/continuity",
        "/staging",
        "/register-check",
        "/triage",
        "/revise",
    ] {
        assert!(
            !addresses.iter().any(|a| a.ends_with(reviewed)),
            "{reviewed} in {addresses:?}"
        );
    }
    assert!(addresses.iter().any(|a| a.ends_with("/register")));
    assert!(addresses.iter().any(|a| a.ends_with("/summary")));
    let versions = open.db.chapter_versions(1).await.unwrap();
    assert_eq!(versions.len(), 1, "the draft alone");
}

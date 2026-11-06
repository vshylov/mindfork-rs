//! A chapter that reaches its goal (docs/fabula.md §4): past the scene cap,
//! the Author may plan one closing scene (G5), or end the chapter there;
//! nothing is planned after it.

use std::sync::Arc;

use super::ChapterConfig;
use super::tests::{Open, Script, config, paused_at, story};
use crate::entities::story::QuestionBody;

fn checkpoint(version: u32) -> QuestionBody {
    QuestionBody::Chapter {
        chapter: 1,
        version,
    }
}

fn capped(max_scenes: u32) -> ChapterConfig {
    ChapterConfig {
        max_scenes,
        ..config()
    }
}

/// The scripted Author plans scenes 1 and 2 and ends at scene 3: with a cap
/// of one, the plan past it is the closing scene's, and the Author plans it.
#[tokio::test]
async fn a_chapter_played_to_its_cap_may_have_one_closing_scene() {
    let mut open = Open::approved().await;
    let port = Script::new(Arc::new(story));
    let outcome = open.advance(&port, &capped(1)).await;
    assert_eq!(paused_at(&outcome), &checkpoint(2));
    let scenes = open.db.scenes_of(1).await.unwrap();
    assert_eq!(
        scenes.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        ["ch1/sc1/r0", "ch1/sc2/r0"]
    );
    let plans: Vec<String> = port
        .addresses()
        .into_iter()
        .filter(|a| a.ends_with("/plan"))
        .collect();
    assert_eq!(
        plans,
        ["ch1/sc1/plan", "ch1/sc2/plan"],
        "nothing past the closing scene"
    );
    let closing = port.prompt_at("ch1/sc2/plan");
    assert!(closing.user.contains("The chapter's one scene is played."));
    assert!(
        closing
            .user
            .contains("plan one closing scene, the chapter's very last")
    );
    let pick = port.prompt_at("ch1/sc2/r0/pick");
    assert!(pick.user.contains("\n\nThe closing scene, "));
    assert!(pick.user.contains("This is the chapter's last scene"));
    // The draft reads both scenes.
    let write = port.prompt_at("ch1/v1/write");
    assert!(write.user.contains("Scene 1") && write.user.contains("Scene 2"));
}

/// With a cap of two, the closing plan is asked at scene 3, where the
/// scripted Author ends the chapter: no third scene.
#[tokio::test]
async fn the_author_may_end_the_chapter_at_the_closing_plan() {
    let mut open = Open::approved().await;
    let port = Script::new(Arc::new(story));
    let outcome = open.advance(&port, &capped(2)).await;
    assert_eq!(paused_at(&outcome), &checkpoint(2));
    assert_eq!(open.db.scenes_of(1).await.unwrap().len(), 2);
    let closing = port.prompt_at("ch1/sc3/plan");
    assert!(closing.user.contains("All 2 scenes are played."));
    assert!(
        open.db
            .all_steps()
            .await
            .unwrap()
            .contains_key("ch1/sc3/plan")
    );
    // A second run asks nothing: the question waits.
    let again = Script::new(Arc::new(story));
    assert_eq!(open.advance(&again, &capped(2)).await, outcome);
    assert!(again.addresses().is_empty());
}

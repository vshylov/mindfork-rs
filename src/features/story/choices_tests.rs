//! A chapter's choices on a scripted port (docs/fabula.md §4, *A chapter's
//! choices stand*): what a step chose where it could have asked the human
//! reaches the chapter's later Author prompts and none of its reviewers',
//! a resumed run reads them from the store, and a question answered again
//! stands once, with its latest answer.

use std::sync::Arc;

use serde_json::json;
use tokio::sync::Notify;

use super::tests::{Open, Reply, Script, config, story};
use crate::entities::story::Assumption;
use crate::features::story::port::{PortError, Prompt};

/// The outline's choice, as the later prompts list it.
const OUTLINED: &str = "- Is Ines new in town? \u{2014} Yes, a month.";

/// The story, with scene 1's plan answering the outline's question again,
/// in its own words, and a question of its own.
fn answered_again(address: &str, prompt: &Prompt) -> Result<Reply, PortError> {
    match (address, story(address, prompt)?) {
        ("ch1/sc1/plan", Reply::Json(mut value)) => {
            value["assumptions"] = json!([
                {"doubt": "is Ines new in town", "choice": "No, she was born here."},
                {"doubt": "Is the gate guarded?", "choice": "By one man."},
            ]);
            Ok(Reply::Json(value))
        }
        (_, reply) => Ok(reply),
    }
}

#[tokio::test]
async fn what_the_outline_chose_reaches_the_plans_the_picks_and_the_prose() {
    let mut open = Open::approved().await;
    let port = Script::new(Arc::new(story));
    open.advance(&port, &config()).await;
    let outline = port.prompt_at("ch1/outline");
    assert!(!outline.user.contains("These choices stand"), "none yet");
    for address in [
        "ch1/sc1/plan",
        "ch1/sc1/r0/pick",
        "ch1/sc2/plan",
        "ch1/v1/write",
        "ch1/v2/revise",
    ] {
        let prompt = port.prompt_at(address);
        assert!(
            prompt.user.contains(&format!(
                "{OUTLINED}\nThese choices stand: plan, pick and write by them"
            )),
            "{address}: {}",
            prompt.user
        );
    }
    for address in ["ch1/v1/reader", "ch1/v1/continuity", "ch1/v1/staging"] {
        let prompt = port.prompt_at(address);
        assert!(
            !prompt.user.contains("Is Ines new in town?"),
            "{address}: a reviewer reads the text, not the Author's choices"
        );
    }
}

#[tokio::test]
async fn a_question_answered_again_stands_once_with_its_latest_answer() {
    let mut open = Open::approved().await;
    let port = Script::new(Arc::new(answered_again));
    open.advance(&port, &config()).await;
    let pick = port.prompt_at("ch1/sc1/r0/pick");
    assert!(
        pick.user.contains(
            "- Is Ines new in town? \u{2014} No, she was born here.\n- Is the gate guarded? \
             \u{2014} By one man.\nThese choices stand"
        ),
        "{}",
        pick.user
    );
    assert!(!pick.user.contains("Yes, a month."), "the answer replaced");
    // The store keeps every answer as it was made.
    let made = open.db.assumptions("ch1/").await.unwrap();
    let steps: Vec<&str> = made.iter().map(|a| a.step.as_str()).collect();
    assert_eq!(steps, ["ch1/outline", "ch1/sc1/plan", "ch1/sc1/plan"]);
}

#[tokio::test]
async fn a_run_killed_after_the_outline_resumes_with_its_choices() {
    let mut open = Open::approved().await;
    let blown = Arc::new(Notify::new());
    // The outline is the run's first call; it hangs at the second, the plan.
    let fused = Script::fused(Arc::new(story), 2, blown.clone());
    let cfg = config();
    tokio::select! {
        outcome = open.advance(&fused, &cfg) => panic!("ran through: {outcome:?}"),
        () = blown.notified() => {}
    }
    let port = Script::new(Arc::new(story));
    open.advance(&port, &config()).await;
    assert!(
        !port.addresses().iter().any(|a| a == "ch1/outline"),
        "the outline is not asked again"
    );
    assert!(port.prompt_at("ch1/sc1/plan").user.contains(OUTLINED));
}

#[test]
fn a_question_is_the_same_by_its_words_in_any_script() {
    let a = |doubt: &str, choice: &str| Assumption {
        doubt: doubt.into(),
        choice: choice.into(),
    };
    let made = [
        a("Кто подписал приказ?", "Начальник района."),
        a("Знает ли Галина Тимура?", "Да."),
        a("кто  подписал приказ", "Начальник Тимура."),
    ];
    assert_eq!(
        Assumption::standing(&made),
        [
            a("Кто подписал приказ?", "Начальник Тимура."),
            a("Знает ли Галина Тимура?", "Да."),
        ]
    );
}

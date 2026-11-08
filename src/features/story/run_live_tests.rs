//! PR 8's live check of a story run as the app starts it (docs/history/fabula.md §4):
//! the fixture story with its canon approved, run on a real route under the
//! app's session budget, three times over the same story — to a small chapter
//! budget, to a stop by its token while a call is out, and to its time limit.
//! Each run resumes where the one before stopped, and each landing is printed
//! as the assistant would read it.
//!
//! What it shows that the mock engine cannot: real streams under a permit of
//! the session budget, given back after every call; the budget counted from
//! what the provider reported; a stop and a time limit cutting a call that is
//! really out, and the engine recording the step either way.
//!
//! `#[ignore]`, skipped without a route — `super::live_route`'s variables —
//! and `MINDFORK_FABULA_RUN_BUDGET`, the
//! first run's budget in tokens (40000).
//!
//! `cargo test story_run_live -- --ignored --nocapture --test-threads=1`

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::board::StoryBoard;
use super::live_route::{fallback_asked, route};
use super::run::tests::{approved, write_story};
use super::run::{Ending, Landed, StoryRun, run};
use super::single::SingleConfig;
use super::single_story::{Budget, MOST_REVISIONS, Stop};
use crate::features::tools::story::landing_text;
use crate::shared::i18n::{Lang, locale};
use crate::shared::session_budget::SessionBudget;

fn show(what: &str, landed: &Landed) {
    eprintln!(
        "--- {what}: {} calls, {} tokens, ${:?}\n{}\n",
        landed.spent.calls,
        landed.spent.tokens(),
        landed.spent.cost_usd,
        landing_text(locale(Lang::En), landed, None)
    );
}

#[tokio::test]
#[ignore]
async fn story_run_live_stops_at_its_budget_a_stop_and_its_time_limit() {
    let Some((backend, model)) = route() else {
        eprintln!("story_run_live: no route; skipped");
        return;
    };
    let budget: u64 = std::env::var("MINDFORK_FABULA_RUN_BUDGET")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(40_000);
    let dir = tempfile::tempdir().unwrap();
    let mut spec = StoryRun {
        chat: Uuid::new_v4(),
        story: "s1".to_owned(),
        run: Uuid::new_v4(),
        db: dir.path().join("stories").join("c").join("s1.db"),
        log: dir.path().join("story-logs").join("c").join("s1.db"),
        model: model.clone(),
        config: SingleConfig {
            budget: Budget {
                tokens: Some(budget),
                usd: None,
            },
            most_revisions: MOST_REVISIONS,
            ..SingleConfig::default()
        },
        window: None,
        timeout: Duration::from_secs(30 * 60),
        fallback_only: fallback_asked(),
    };
    write_story(&spec.db, &spec.log, async |db, author| {
        approved(db, author).await
    })
    .await;
    let board = Arc::new(StoryBoard::new());
    let sessions = Arc::new(SessionBudget::new(1, None));
    eprintln!("story_run_live on {model}, budget {budget} tokens");

    // 1. The budget: the run stops before the call that would pass it.
    let landed = run(
        &spec,
        backend.clone(),
        Some(sessions.clone()),
        &board,
        CancellationToken::new(),
    )
    .await;
    show("budget", &landed);
    assert!(
        matches!(
            &landed.ending,
            Ending::Stopped {
                why: Stop::Budget(_),
                ..
            }
        ),
        "{:?}",
        landed.ending
    );
    assert!(landed.spent.tokens() >= budget, "{:?}", landed.spent);
    assert_eq!(sessions.available_sessions(), 1, "every permit given back");

    // 2. A stop by the run's token while a call is out: room in the budget,
    //    and the token fired a few seconds into the run's first call.
    spec.config.budget.tokens = Some(budget * 50);
    spec.run = Uuid::new_v4();
    let cancel = CancellationToken::new();
    let task = {
        let (spec, backend, sessions, board, cancel) = (
            spec.clone(),
            backend.clone(),
            sessions.clone(),
            board.clone(),
            cancel.clone(),
        );
        tokio::spawn(async move { run(&spec, backend, Some(sessions), &board, cancel).await })
    };
    let step = in_a_call(&board, spec.run).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    cancel.cancel();
    let landed = task.await.unwrap();
    show("stop", &landed);
    let Ending::Stopped {
        step: stopped,
        why: Stop::Cancelled,
    } = &landed.ending
    else {
        panic!("{:?}", landed.ending);
    };
    eprintln!("the board showed {step}; the engine stopped at {stopped}");
    assert_eq!(sessions.available_sessions(), 1);

    // 3. The time limit: the same story, a limit of twenty seconds.
    spec.run = Uuid::new_v4();
    spec.timeout = Duration::from_secs(20);
    let landed = run(
        &spec,
        backend,
        Some(sessions.clone()),
        &board,
        CancellationToken::new(),
    )
    .await;
    show("time limit", &landed);
    assert!(
        matches!(&landed.ending, Ending::TimedOut { .. }),
        "{:?}",
        landed.ending
    );
    assert_eq!(sessions.available_sessions(), 1);
    assert!(board.of_chat(spec.chat).is_empty());
}

/// The address of the call `run` is in, once it is in one.
async fn in_a_call(board: &StoryBoard, run: Uuid) -> String {
    loop {
        if let Some(step) = board.of_run(run).and_then(|v| v.step) {
            return step;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

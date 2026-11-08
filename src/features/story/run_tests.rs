//! A story run as the app starts it (docs/fabula.md §4, PR 8), on a backend
//! that never answers until it is cancelled: the pauses that need no call,
//! the landing's contents read from the stores, the stop, the time limit, the
//! budget, a story that cannot open, and one run per story.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    Ending, LandedChapter, StoryRun, budget_of, needs_fallback, run, single_config, writer_sampling,
};
use crate::entities::sampling::ReasoningEffort;
use crate::entities::story::{Decision, QuestionBody, Resolution, VersionOrigin, fixture_canon};
use crate::features::story::board::StoryBoard;
use crate::features::story::single::SingleConfig;
use crate::features::story::single_story::{MOST_REVISIONS, Stop};
use crate::shared::api::contract::{
    ChatChunk, ChatRequest, ChatStream, EngineBackend, FinishReason,
};
use crate::shared::config::{ServerMode, SpecType, ToolSettings};
use crate::shared::storage::schema::STORY_SCHEMA;
use crate::shared::storage::story::{
    AuthorWriter, CallStatus, CallUsage, Mechanism, NewCall, StoryDb,
};

/// A backend whose every reply hangs until its request is cancelled, then
/// ends as cancelled; it counts the requests.
#[derive(Default)]
pub(crate) struct Hanging {
    pub(crate) calls: AtomicUsize,
}

#[async_trait::async_trait]
impl EngineBackend for Hanging {
    async fn chat_stream(
        &self,
        _request: ChatRequest,
        cancel: CancellationToken,
    ) -> anyhow::Result<ChatStream> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Box::pin(async_stream::stream! {
            cancel.cancelled().await;
            yield ChatChunk::Finished(FinishReason::Cancelled);
        }))
    }
}

/// A story in its own directory and the run of it.
struct Fixture {
    _dir: tempfile::TempDir,
    spec: StoryRun,
}

fn spec_in(dir: &std::path::Path) -> StoryRun {
    StoryRun {
        chat: Uuid::new_v4(),
        story: "s1".to_owned(),
        run: Uuid::new_v4(),
        db: dir.join("stories").join("c").join("s1.db"),
        log: dir.join("story-logs").join("c").join("s1.db"),
        model: "mock-model".to_owned(),
        config: SingleConfig::default(),
        window: None,
        timeout: Duration::from_secs(60),
        fallback_only: false,
    }
}

/// The story at `db`, its call log at `log`, with the fixture's canon and
/// what `write` adds through its own writers, closed again.
pub(crate) async fn write_story<F>(db: &std::path::Path, log: &std::path::Path, write: F)
where
    F: AsyncFnOnce(&StoryDb, &mut AuthorWriter),
{
    for path in [db, log] {
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
    }
    let store = StoryDb::open(db, log).await.unwrap();
    let mut author = store.author_writer().await.unwrap();
    author.import_canon(&fixture_canon()).await.unwrap();
    write(&store, &mut author).await;
    drop(author);
    store.close().await.unwrap();
}

/// A story written by `write`, in a directory of its own.
async fn story<F>(write: F) -> Fixture
where
    F: AsyncFnOnce(&StoryDb, &mut AuthorWriter),
{
    let dir = tempfile::tempdir().unwrap();
    let spec = spec_in(dir.path());
    write_story(&spec.db, &spec.log, write).await;
    Fixture { _dir: dir, spec }
}

/// A failed call an earlier run recorded under `address`, with what it cost.
fn earlier_call(
    run: crate::entities::story::RunId,
    address: &str,
    (input, output): (u64, u64),
    cost_usd: Option<f64>,
) -> NewCall {
    NewCall {
        run,
        address: address.to_owned(),
        attempt: 1,
        role: "author".to_owned(),
        model: "m".to_owned(),
        params: json!({}),
        mechanism: Mechanism::Schema,
        request: json!({}),
        prompt_sha256: "h".to_owned(),
        response: None,
        status: CallStatus::Error,
        usage: CallUsage {
            input_tokens: Some(input),
            output_tokens: Some(output),
            cached_tokens: None,
            cost_usd,
        },
        latency_ms: None,
    }
}

fn approve() -> Resolution {
    Resolution {
        decision: Decision::Approve,
        notes: String::new(),
        evidence: json!(null),
    }
}

/// The canon approved, as P1's answer leaves it.
pub(crate) async fn approved(db: &StoryDb, author: &mut AuthorWriter) {
    let run = db
        .run_log_writer()
        .await
        .unwrap()
        .start_run(&json!({}))
        .await;
    let question = author
        .add_question(&run.unwrap(), "canon", &QuestionBody::Canon)
        .await
        .unwrap();
    author.resolve(&question.id, &approve()).await.unwrap();
}

/// A story at chapter 1's checkpoint: a draft and its revision, and the
/// question asked about the revision.
pub(crate) async fn at_checkpoint(db: &StoryDb, author: &mut AuthorWriter) {
    approved(db, author).await;
    let run = db
        .run_log_writer()
        .await
        .unwrap()
        .start_run(&json!({}))
        .await
        .unwrap();
    author
        .add_chapter(1, VersionOrigin::Draft, "## One\n\nA first draft.")
        .await
        .unwrap();
    author
        .add_chapter(
            1,
            VersionOrigin::Revision,
            "## One\n\nThe ferry never came.",
        )
        .await
        .unwrap();
    author
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
}

async fn run_on(spec: &StoryRun, backend: Arc<Hanging>, board: &StoryBoard) -> super::Landed {
    run(spec, backend, None, board, CancellationToken::new()).await
}

/// An unapproved canon asks P1 at once: no call is made, nothing is spent,
/// and the board has the story back.
#[tokio::test]
async fn an_unapproved_canon_pauses_at_once_and_spends_nothing() {
    let fixture = story(async |_, _| {}).await;
    let (backend, board) = (Arc::new(Hanging::default()), StoryBoard::new());

    let landed = run_on(&fixture.spec, backend.clone(), &board).await;

    let Ending::Paused(question) = &landed.ending else {
        panic!("{:?}", landed.ending);
    };
    assert_eq!(question.body, QuestionBody::Canon);
    assert_eq!(landed.chapter, None);
    assert_eq!(landed.spent.calls, 0);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert!(board.of_chat(fixture.spec.chat).is_empty());
}

/// At P2 the landing carries the version asked about, not the one before
/// it, read from the stores, so a run that only found the question pending
/// lands the same.
#[tokio::test]
async fn a_chapter_at_its_checkpoint_lands_with_the_version_asked_about() {
    let fixture = story(async |db, author| at_checkpoint(db, author).await).await;
    let board = StoryBoard::new();

    let landed = run_on(&fixture.spec, Arc::default(), &board).await;

    assert!(matches!(
        &landed.ending,
        Ending::Paused(q) if q.body == QuestionBody::Chapter { chapter: 1, version: 2 }
    ));
    assert_eq!(
        landed.chapter,
        Some(LandedChapter {
            number: 1,
            version: 2,
            text: "## One\n\nThe ferry never came.".to_owned(),
        })
    );
}

/// Stopped while its first call hangs, the run stops at that step, which the
/// board showed while it ran; nothing was accepted, and the next run starts
/// from the same step.
#[tokio::test]
async fn a_stopped_run_names_its_step_and_the_board_showed_it() {
    let fixture = story(async |db, author| approved(db, author).await).await;
    let (backend, board) = (Arc::new(Hanging::default()), Arc::new(StoryBoard::new()));
    let cancel = CancellationToken::new();
    let task = {
        let (spec, backend, board, cancel) = (
            fixture.spec.clone(),
            backend.clone(),
            board.clone(),
            cancel.clone(),
        );
        tokio::spawn(async move { run(&spec, backend, None, &board, cancel).await })
    };
    let step = loop {
        if let Some(step) = board.of_run(fixture.spec.run).and_then(|v| v.step) {
            break step;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    assert!(step.starts_with("ch1/"), "{step}");
    cancel.cancel();

    let landed = task.await.unwrap();

    let Ending::Stopped {
        step: stopped,
        why: Stop::Cancelled,
    } = &landed.ending
    else {
        panic!("{:?}", landed.ending);
    };
    assert!(step.starts_with(stopped.as_str()), "{stopped} vs {step}");
    assert!(board.of_run(fixture.spec.run).is_none());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
}

/// The time limit cancels the calls, and the run reads as timed out at the
/// step it was at, not as stopped by a person.
#[tokio::test]
async fn the_time_limit_ends_the_run_at_its_step() {
    let mut fixture = story(async |db, author| approved(db, author).await).await;
    fixture.spec.timeout = Duration::from_millis(50);

    let landed = run_on(&fixture.spec, Arc::default(), &StoryBoard::new()).await;

    let Ending::TimedOut { step } = &landed.ending else {
        panic!("{:?}", landed.ending);
    };
    assert!(step.starts_with("ch1/"), "{step}");
}

/// A chapter whose budget is spent stops before its next call, and what was
/// spent is in the landing's words.
#[tokio::test]
async fn a_spent_budget_stops_the_run_before_its_next_call() {
    let mut fixture = story(async |db, author| {
        approved(db, author).await;
        let mut log = db.run_log_writer().await.unwrap();
        let run = log.start_run(&json!({})).await.unwrap();
        log.record_call(earlier_call(run, "ch1/b0/t1", (900, 100), None))
            .await
            .unwrap();
    })
    .await;
    fixture.spec.config.budget.tokens = Some(1000);
    let backend = Arc::new(Hanging::default());

    let landed = run_on(&fixture.spec, backend.clone(), &StoryBoard::new()).await;

    let Ending::Stopped {
        why: Stop::Budget(spent),
        ..
    } = &landed.ending
    else {
        panic!("{:?}", landed.ending);
    };
    assert!(spent.contains("1000 tokens of 1000"), "{spent}");
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(landed.spent.calls, 0, "this run spent nothing");
}

/// A story file of a newer schema is not run, and says why; a story already
/// being written by another run is not opened at all.
#[tokio::test]
async fn a_newer_story_or_one_another_run_holds_does_not_run() {
    let fixture = story(async |_, _| {}).await;
    let board = StoryBoard::new();
    let other = Uuid::new_v4();
    board
        .claim(fixture.spec.chat, &fixture.spec.story, other)
        .unwrap();

    let held = run_on(&fixture.spec, Arc::default(), &board).await;

    assert!(
        matches!(&held.ending, Ending::Failed(why) if why.contains(&other.to_string())),
        "{:?}",
        held.ending
    );
    assert_eq!(
        board.of_chat(fixture.spec.chat).len(),
        1,
        "the holder keeps it"
    );
    board.release(other);

    rusqlite::Connection::open(&fixture.spec.db)
        .unwrap()
        .execute_batch(&format!("PRAGMA user_version = {};", STORY_SCHEMA + 1))
        .unwrap();
    let newer = run_on(&fixture.spec, Arc::default(), &board).await;
    assert!(
        matches!(&newer.ending, Ending::Failed(why) if why.contains("newer")),
        "{:?}",
        newer.ending
    );
    assert!(board.of_chat(fixture.spec.chat).is_empty());
}

/// The writer thinks at a medium effort with room for a chapter, as the
/// Author did (G1, PR 6) and M3's B0 with it.
#[test]
fn the_writer_samples_as_measured() {
    let writer = writer_sampling();
    assert_eq!(writer.thinking, Some(true));
    assert_eq!(writer.reasoning_effort, Some(ReasoningEffort::Medium));
    assert_eq!(writer.max_tokens, Some(32_000));
    assert_eq!(writer.temperature, Some(0.8));
}

/// The settings' budget reaches the chapter's session as its ceiling, `0`
/// tokens and no dollars reading as no limit, and the session ends on its
/// revisions, with no target (T25); a managed server with a draft model takes
/// the fallback, and nothing else does.
#[test]
fn the_settings_shape_the_chapter_and_the_mechanism() {
    let mut tools = ToolSettings::default();
    let config = single_config(&tools);
    assert_eq!(config.budget.tokens, Some(6_000_000));
    assert_eq!(config.budget.usd, None);
    assert_eq!(config.most_revisions, MOST_REVISIONS);
    assert_eq!(config.target, None);
    assert_eq!(config.words, SingleConfig::default().words);
    tools.story_budget_tokens = 0;
    tools.story_budget_usd = Some(1.5);
    let budget = budget_of(&tools);
    assert_eq!(budget.tokens, None);
    assert_eq!(budget.usd, Some(1.5));
    tools.story_budget_usd = Some(0.0);
    assert_eq!(budget_of(&tools).usd, None);

    assert!(needs_fallback(ServerMode::Managed, SpecType::DraftMtp));
    assert!(needs_fallback(ServerMode::Managed, SpecType::DraftSimple));
    assert!(!needs_fallback(ServerMode::Managed, SpecType::NgramSimple));
    assert!(!needs_fallback(ServerMode::Managed, SpecType::None));
    assert!(!needs_fallback(ServerMode::External, SpecType::DraftMtp));
}

/// What a run spent is what its own calls added, cost included, and not what
/// the story spent before it; a turn that fails stops the run at its address,
/// where the next run asks it again; and a stop asks the person nothing.
#[tokio::test]
async fn a_run_lands_what_its_calls_spent_and_the_step_that_failed() {
    use crate::shared::api::contract::{Served, TokenUsage};
    use crate::shared::api::mock::MockBackend;
    let fixture = story(async |db, author| {
        approved(db, author).await;
        // A run before this one, whose draft failed after spending tokens.
        let mut log = db.run_log_writer().await.unwrap();
        let earlier = log.start_run(&json!({})).await.unwrap();
        log.record_call(earlier_call(earlier, "ch1/b0/t1", (5000, 0), Some(1.0)))
            .await
            .unwrap();
    })
    .await;
    let draft = vec![
        ChatChunk::Text("## The harbour\n\nThe gate was shut.".to_owned()),
        ChatChunk::Usage(TokenUsage {
            prompt_tokens: 120,
            completion_tokens: 30,
            ..TokenUsage::default()
        }),
        ChatChunk::Served(Served {
            provider: None,
            cost_nanos: Some(20_000_000),
        }),
        ChatChunk::Finished(FinishReason::Stop),
    ];
    let failed = vec![
        ChatChunk::Error {
            message: "the endpoint is gone".to_owned(),
            transient: false,
        },
        ChatChunk::Finished(FinishReason::Error),
    ];
    let backend = MockBackend::sequence(vec![draft, failed]);

    let landed = run(
        &fixture.spec,
        Arc::new(backend),
        None,
        &StoryBoard::new(),
        CancellationToken::new(),
    )
    .await;

    let Ending::Stopped {
        step,
        why: Stop::Failed(_),
    } = &landed.ending
    else {
        panic!("{:?}", landed.ending);
    };
    assert_eq!(step, "ch1/b0/t2", "the critique after the draft");
    assert_eq!(
        landed.spent.input_tokens, 120,
        "the draft, not the run before"
    );
    assert_eq!(landed.spent.output_tokens, 30);
    let cost = landed.spent.cost_usd.expect("the provider reported it");
    assert!((cost - 0.02).abs() < 1e-9, "{cost}");
    assert_eq!(landed.chapter, None, "no chapter is asked about");
}

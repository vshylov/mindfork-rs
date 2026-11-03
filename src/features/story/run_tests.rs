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
    Ending, LandedChapter, StoryRun, chapter_config, character_sampling, needs_fallback,
    role_sampling, run,
};
use crate::entities::sampling::ReasoningEffort;
use crate::entities::story::{
    Assumption, BranchId, Decision, QuestionBody, Resolution, SceneSetup, VersionOrigin,
    fixture_canon,
};
use crate::entities::story::{
    ChapterArc, ChapterId, Decided, NewNote, NoteDecision, NoteId, NoteKind, NoteSeverity,
    Register, Reviewer, Thread,
};
use crate::features::story::board::StoryBoard;
use crate::features::story::chapter::{ChapterConfig, Stop};
use crate::features::story::keeper::LeftOpen;
use crate::features::story::port::Role;
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
        config: ChapterConfig::default(),
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
        rerun: None,
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

/// A story at chapter 1's checkpoint: a scene asked for again and then kept
/// with the Author's note, one assumption, a draft and its revision with their
/// summaries, and the question asked about the revision.
pub(crate) async fn at_checkpoint(db: &StoryDb, author: &mut AuthorWriter) {
    approved(db, author).await;
    let run = db
        .run_log_writer()
        .await
        .unwrap()
        .start_run(&json!({}))
        .await
        .unwrap();
    let setup = SceneSetup {
        location: "harbour".into(),
        present: vec!["mara".into(), "silas".into()],
        just_happened: "The ferry was cancelled.".into(),
        pressure: "The gate closes at sunset.".into(),
    };
    // The scene's first attempt was asked for again; its second was kept.
    let again = author
        .add_scene((1, 1, 0), &setup, "Mara learns of the debt.")
        .await
        .unwrap();
    author
        .record_pick(&again, None, "Play it again in the rain.")
        .await
        .unwrap();
    let kept = author
        .add_scene((1, 1, 1), &setup, "Mara learns of the debt.")
        .await
        .unwrap();
    let genesis = db.genesis().await.unwrap();
    let branch = db.fork(&kept, &BranchId::genesis(), 1, 7).await.unwrap();
    author
        .record_pick(&kept, Some(branch.branch()), "The quarrel shows the debt.")
        .await
        .unwrap();
    drop((genesis, branch));
    author
        .add_assumption(
            &run,
            "ch1/sc1/plan",
            "author",
            &Assumption {
                doubt: "Whether Silas lies.".into(),
                choice: "He lies.".into(),
            },
        )
        .await
        .unwrap();
    // A draft, and the revision the review asked for: the checkpoint asks
    // about the revision.
    let draft = author
        .add_chapter(1, VersionOrigin::Draft, "## One\n\nA first draft.")
        .await
        .unwrap();
    author
        .add_summary(&draft.id, "An earlier summary.")
        .await
        .unwrap();
    let revised = author
        .add_chapter(
            1,
            VersionOrigin::Revision,
            "## One\n\nThe ferry never came.",
        )
        .await
        .unwrap();
    author
        .add_summary(&revised.id, "Mara learns of the debt.")
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

/// At P2 the landing carries the version asked about, not the one before it,
/// with its summary, the note of each scene kept — a rerun's request is no
/// pick — and the chapter's assumptions, all read from the stores, so a run
/// that only found the question pending lands the same.
#[tokio::test]
async fn a_chapter_at_its_checkpoint_lands_with_its_summary_picks_and_assumptions() {
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
            summary: Some("Mara learns of the debt.".to_owned()),
        })
    );
    assert_eq!(
        landed.picks,
        [(1, "The quarrel shows the debt.".to_owned())],
        "by the scene's number"
    );
    assert_eq!(
        landed.assumptions,
        [Assumption {
            doubt: "Whether Silas lies.".into(),
            choice: "He lies.".into(),
        }]
    );
}

/// Chapter 1's checkpoint with two threads its register leaves open: the
/// Author kept the first open on purpose and accepted the note on the second.
async fn with_open_threads(db: &StoryDb, author: &mut AuthorWriter) {
    at_checkpoint(db, author).await;
    let thread = |n: usize, question: &str| Thread {
        id: format!("ch1.thread{n}"),
        question: question.to_owned(),
        quote: "a quote".to_owned(),
    };
    author
        .add_register(
            &ChapterId::of(1, 2),
            &Register {
                opened: vec![
                    thread(1, "Whose steps were on the path?"),
                    thread(2, "Who wrote the log's last line?"),
                ],
                paid: Vec::new(),
                facts: Vec::new(),
            },
        )
        .await
        .unwrap();
    let note = |n: usize| NewNote {
        kind: NoteKind::OpenThread,
        severity: NoteSeverity::High,
        anchor: Some(format!("ch1.thread{n}")),
        body: "Unpaid.".to_owned(),
        evidence: Vec::new(),
    };
    let draft = ChapterId::of(1, 1);
    db.review_writer(&draft, Reviewer::Continuity)
        .await
        .unwrap()
        .write(&[note(1), note(2)])
        .await
        .unwrap();
    let decided = |n: usize, decision, reason: &str| Decided {
        note: NoteId::new(format!("ch1/v1/continuity/n{n}")),
        decision,
        reason: reason.to_owned(),
    };
    author
        .record_decisions(
            &draft,
            &[
                decided(1, NoteDecision::Rejected, "Left for the reader."),
                decided(2, NoteDecision::Accepted, "Pay it off."),
            ],
        )
        .await
        .unwrap();
}

/// At the arc's last chapter the landing lists the threads the story leaves
/// open, with the Author's reason where it kept one open; a chapter before
/// the last lists none (docs/fabula.md §6, T16).
#[tokio::test]
async fn the_last_chapter_lands_with_the_threads_it_leaves_open() {
    let fixture = story(async |db, author| with_open_threads(db, author).await).await;
    let landed = run_on(&fixture.spec, Arc::default(), &StoryBoard::new()).await;
    assert_eq!(
        landed.left_open,
        [
            LeftOpen {
                question: "Whose steps were on the path?".into(),
                reason: Some("Left for the reader.".into()),
            },
            LeftOpen {
                question: "Who wrote the log's last line?".into(),
                reason: None,
            },
        ],
        "a note the Author accepted gives no reason to leave it"
    );

    let fixture = story(async |db, author| {
        with_open_threads(db, author).await;
        let mut canon = fixture_canon();
        canon.arc.chapters.push(ChapterArc {
            goal: "Ines decides whom to trust.".into(),
        });
        author.import_canon(&canon).await.unwrap();
    })
    .await;
    let landed = run_on(&fixture.spec, Arc::default(), &StoryBoard::new()).await;
    assert!(landed.chapter.is_some());
    assert!(landed.left_open.is_empty(), "not the arc's last chapter");
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
        log.record_call(earlier_call(run, "ch1/outline", (900, 100), None))
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

/// Characters run muted, every other role thinks — the Author at a medium
/// effort with room for a chapter (G1, PR 6).
#[test]
fn the_roles_sample_as_measured() {
    let muted = character_sampling();
    assert_eq!(muted.thinking, Some(false));
    assert_eq!(muted.reasoning_effort, Some(ReasoningEffort::None));
    assert_eq!(muted.reasoning_budget, Some(0));
    let roles = role_sampling();
    for role in [
        Role::WorldMaster,
        Role::Author,
        Role::Reader,
        Role::Continuity,
        Role::CanonKeeper,
    ] {
        let (_, sampling) = roles.iter().find(|(r, _)| *r == role).unwrap();
        assert_eq!(sampling.thinking, Some(true), "{role:?}");
    }
    let (_, author) = roles.iter().find(|(r, _)| *r == Role::Author).unwrap();
    assert_eq!(author.reasoning_effort, Some(ReasoningEffort::Medium));
    assert_eq!(author.max_tokens, Some(32_000));
    for role in [Role::Reader, Role::Continuity, Role::CanonKeeper] {
        let (_, sampling) = roles.iter().find(|(r, _)| *r == role).unwrap();
        assert_eq!(
            sampling.max_tokens,
            Some(32_000),
            "{role:?}: PR 10 found 16,384 cut on Qwen"
        );
    }
    assert!(!roles.iter().any(|(r, _)| *r == Role::Character));
}

/// The settings' budget reaches the chapter, `0` tokens and no dollars
/// reading as no limit; a managed server with a draft model takes the
/// fallback, and nothing else does.
#[test]
fn the_settings_shape_the_chapter_and_the_mechanism() {
    let mut tools = ToolSettings::default();
    let config = chapter_config(&tools, "Russian");
    assert_eq!(config.budget.tokens, Some(6_000_000));
    assert_eq!(config.budget.usd, None);
    assert_eq!(config.human_language, "Russian");
    assert_eq!(config.max_scenes, ChapterConfig::default().max_scenes);
    tools.story_budget_tokens = 0;
    tools.story_budget_usd = Some(1.5);
    let config = chapter_config(&tools, "English");
    assert_eq!(config.budget.tokens, None);
    assert_eq!(config.budget.usd, Some(1.5));
    tools.story_budget_usd = Some(0.0);
    assert_eq!(chapter_config(&tools, "English").budget.usd, None);

    assert!(needs_fallback(ServerMode::Managed, SpecType::DraftMtp));
    assert!(needs_fallback(ServerMode::Managed, SpecType::DraftSimple));
    assert!(!needs_fallback(ServerMode::Managed, SpecType::NgramSimple));
    assert!(!needs_fallback(ServerMode::Managed, SpecType::None));
    assert!(!needs_fallback(ServerMode::External, SpecType::DraftMtp));
}

/// What a run spent is what its own calls added, cost included, and not what
/// the story spent before it; a call that fails after the outline stops the
/// run at its step, where the next run tries it again; and the outline's
/// assumption waits for the checkpoint rather than riding on the stop.
#[tokio::test]
async fn a_run_lands_what_its_calls_spent_and_the_step_that_failed() {
    use crate::shared::api::contract::{Served, TokenUsage};
    use crate::shared::api::mock::MockBackend;
    let fixture = story(async |db, author| {
        approved(db, author).await;
        // A run before this one, which spent tokens of its own.
        let mut log = db.run_log_writer().await.unwrap();
        let earlier = log.start_run(&json!({})).await.unwrap();
        log.record_call(earlier_call(earlier, "ch1/outline", (5000, 0), Some(1.0)))
            .await
            .unwrap();
    })
    .await;
    let outline = json!({
        "goals": ["The secret surfaces."],
        "assumptions": [{"doubt": "Whether the ferry runs.", "choice": "It does not."}],
    });
    let backend = MockBackend::sequence(vec![vec![
        ChatChunk::Text(outline.to_string()),
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
    ]]);

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
    assert!(step.starts_with("ch1/") && step != "ch1/outline", "{step}");
    // The mock answers every later call with the same reply, which the next
    // step's schema refuses: the outline, then the call and its one repair.
    let calls = landed.spent.calls;
    assert_eq!(calls, 3, "the outline, the next call and its repair");
    assert_eq!(landed.spent.input_tokens, 120 * calls);
    assert_eq!(landed.spent.output_tokens, 30 * calls);
    let cost = landed.spent.cost_usd.expect("the provider reported it");
    assert!((cost - 0.02 * calls as f64).abs() < 1e-9, "{cost}");
    // The outline's assumption waits for the checkpoint: a stop asks nothing.
    assert!(landed.assumptions.is_empty(), "{:?}", landed.assumptions);
    assert!(landed.picks.is_empty());
    assert_eq!(landed.chapter, None, "no chapter is asked about");
}

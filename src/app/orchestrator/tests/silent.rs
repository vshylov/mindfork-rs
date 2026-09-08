//! Orchestrator tests — the silent lane (docs/research/silent-tasks-budget.md
//! §4, §7): the app's own background requests — the title, the three silent
//! loops, the compaction roll, impersonation on the shared engine — under the
//! app-wide session budget; and the lane yielding to an interactive stream
//! (docs/research/silent-preemption.md §4, §7). Part of the [`super`] module
//! (fixtures in mod.rs; the keyed engine in parallel.rs; the background
//! fixtures in background.rs).

use super::super::background::{Acted, Acting, BgOutcome};
use super::background::{cfg, finished, next, running_run, runs_out, start};
use super::parallel::{KeyedRecorder, long_text, sized};
use super::subagent::{hang, text};
use super::*;
use crate::entities::note::Note;
use crate::features::tools::notes::SELF_NOTE_TAG;
use crate::features::tools::self_model::{GET_SELF_MODEL_ID, UPDATE_SELF_MODEL_ID};
use crate::shared::config::AutoTitleMode;
use crate::shared::session_budget::SILENT_YIELDS_MAX;
use tokio_util::sync::CancellationToken;

/// Stable lines of the silent requests' system prompts (the `en` bundle) —
/// what the keyed recorder routes on.
const TITLE_KEY: &str = "inventing a short title";
const COMPACT_KEY: &str = "compressing the earlier part";
const REFLECT_KEY: &str = "quiet background self-reflection";

/// A forty-word message: long enough that a two-exchange chat outgrows a
/// small compaction tail, so the roll has a boundary to cut at.
fn long(text: &str) -> String {
    std::iter::repeat_n(text, 40).collect::<Vec<_>>().join(" ")
}

/// An orchestrator on which **every** silent request fires at the next
/// landing: every cadence at one, the profile with the self-model and note
/// tools, two user notes and two `@self` observations in the store (the
/// consolidations' "something to do" signals), a two-exchange chat above a
/// small compaction tail, the automatic title at the reply, the window told
/// explicitly. Returns `(dir, orch, chat_id)`.
fn orch_ready_for_the_fan_out() -> (tempfile::TempDir, Orchestrator, Uuid) {
    let (dir, mut orch) = bare_orch();
    orch.config.self_model.auto_reflect_every = 1;
    orch.config.self_model.auto_consolidate_every = 1;
    orch.config.notes.auto_consolidate_every = 1;
    orch.config.interface.auto_title = AutoTitleMode::AfterAssistantReply;
    orch.config.compaction.enabled = true;
    orch.config.compaction.threshold_pct = 75;
    orch.config.compaction.context_tokens = Some(1000);
    orch.config.compaction.tail_tokens = 32;
    let mut profile = Profile::new("P", "sys");
    profile.enabled_tools = vec![
        GET_SELF_MODEL_ID.into(),
        "update_self_model".into(),
        "note_merge".into(),
    ];
    let pid = profile.id;
    let mut chat = Chat::from_profile(&profile, "t");
    chat.push_message(Message::user(long("first question")));
    chat.push_message(Message::assistant(long("first answer")));
    chat.push_message(Message::user(long("second question")));
    chat.push_message(Message::assistant(long("second answer")));
    let chat_id = chat.id;
    for text in ["user note one", "user note two"] {
        orch.storage
            .db()
            .note_insert(&Note::new(pid, text, Vec::new()))
            .unwrap();
    }
    for text in ["I value brevity", "the user likes it short"] {
        orch.storage
            .db()
            .note_insert(&Note::new(pid, text, vec![SELF_NOTE_TAG.to_string()]))
            .unwrap();
    }
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);
    (dir, orch, chat_id)
}

/// The five silent requests a landing can start, in `handle_done`'s order
/// (fork F6: the roll ahead of the loops).
fn fan_out(orch: &mut Orchestrator, chat_id: Uuid) {
    orch.maybe_auto_title(chat_id, AutoTitleMode::AfterAssistantReply);
    orch.maybe_auto_compact(
        chat_id,
        Some(super::super::generation::TurnUsage {
            prompt_tokens: 900,
            completion_tokens: 10,
        }),
    );
    orch.maybe_auto_reflect(chat_id);
    orch.maybe_auto_consolidate(chat_id);
    orch.maybe_auto_self_consolidate(chat_id);
}

/// Polls until `done` holds or `ms` have passed.
async fn settle(ms: u64, done: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    while !done() && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// The orchestrator up on `backend` with its default profile switched to
/// **English** — the recorder keys on the `en` prompts, and the default
/// profile speaks Russian — and the first chat activated. The edit is
/// applied before any later command, since commands are handled in order.
async fn spawn_english(
    backend: Arc<KeyedRecorder>,
    cfg: AppConfig,
) -> (
    tempfile::TempDir,
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
    Uuid,
) {
    let (dir, cmd_tx, mut rx, handle) =
        spawn_orch_cfg(Some(backend as Arc<dyn EngineBackend>), cfg);
    let profiles = next(&mut rx, |e| matches!(e, AppEvent::ProfileList(_))).await;
    let AppEvent::ProfileList(profiles) = profiles else {
        unreachable!()
    };
    let active = next(&mut rx, |e| matches!(e, AppEvent::ChatActivated { .. })).await;
    let AppEvent::ChatActivated { id: chat_id, .. } = active else {
        unreachable!()
    };
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: profiles[0].id,
            edit: Box::new(crate::features::profiles::ProfileEdit {
                language: Some(crate::shared::i18n::Lang::En),
                // The self-model tools are off by default, and reflection
                // fires only on a profile that has them.
                enabled_tools: Some(vec![
                    GET_SELF_MODEL_ID.into(),
                    "update_self_model".into(),
                    "start_subagent".into(),
                    "call_subagent".into(),
                ]),
                ..Default::default()
            }),
        })
        .unwrap();
    (dir, cmd_tx, rx, handle, chat_id)
}

/// One landing with every cadence due opens the five silent requests
/// **one at a time** (research §4.1, fork F5): the recorder answers each
/// after a delay, so streams that overlap would be counted together — and
/// on the code before the lane they were, five at once (research §3.1).
#[tokio::test]
async fn a_landing_opens_the_silent_requests_one_at_a_time() {
    let (_d, mut orch, chat_id) = orch_ready_for_the_fan_out();
    let backend = KeyedRecorder::new(vec![("", Vec::new())], 60);
    orch.engines.backend = Some(backend.clone() as Arc<dyn EngineBackend>);

    fan_out(&mut orch, chat_id);
    settle(3000, || backend.requests().len() >= 5).await;

    let kinds = [
        BackgroundKind::Reflection,
        BackgroundKind::Consolidation,
        BackgroundKind::SelfConsolidation,
        BackgroundKind::Compaction,
    ];
    assert!(
        kinds.iter().all(|k| orch.bg_running(*k)),
        "every silent task took its slot"
    );
    let requests = backend.requests().len();
    eprintln!(
        "silent requests opened: {requests}, most at once: {}",
        backend.max_in_flight()
    );
    assert!(requests >= 5, "the title and the four tasks: {requests}");
    assert_eq!(
        backend.max_in_flight(),
        1,
        "the silent lane is one stream wide"
    );
    // The tasks screen's snapshot tells the one streaming from the ones
    // waiting behind it (research §4.6), while any of them is still out.
    let list = orch.task_list();
    let running: Vec<_> = list.app.iter().filter(|t| t.running).collect();
    assert_eq!(running.len(), 4);
}

/// The probe's arm 1 as a unit test (silent-tasks-budget §3.1, then
/// silent-preemption §4): a compaction roll asked for while a background run
/// streams, on a pool the two do not fit together — the roll **waits** for
/// the run to end (it arrives with no stream open); the wake turn the
/// landing starts then does not fit beside the roll and **displaces** it:
/// the roll's stream ends, the turn streams at once, and the roll is made
/// again — the same request, arriving after the turn — and lands a summary,
/// never the fragment its first stream left. Two sessions, so the permits
/// allow two at once and only the pool serialises: nothing ever overlaps,
/// and all three complete.
#[tokio::test]
async fn the_roll_waits_for_the_run_and_yields_to_the_wake_turn() {
    let backend = KeyedRecorder::new(
        vec![
            (
                "",
                vec![
                    text("ok"),
                    start("c1"),
                    // The turn's exact usage puts the chat at the trigger:
                    // 3000 of a 4000 window at 75 %.
                    sized(text("started it"), 3000, 10),
                    text("noted"),
                ],
            ),
            ("be harsh", vec![hang("thinking")]),
            // The first stream is long enough to be displaced mid-way; the
            // retry's is the summary.
            (COMPACT_KEY, vec![long_text(30), text("a summary")]),
        ],
        30,
    );
    let mut cfg = cfg(2);
    cfg.engine.managed.context_size = 4000;
    cfg.compaction.enabled = true;
    cfg.compaction.threshold_pct = 75;
    cfg.compaction.tail_tokens = 32;
    let (dir, cmd_tx, mut rx, handle, chat_id) = spawn_english(backend.clone(), cfg).await;
    // A first exchange, so the roll has a user boundary to cut at.
    cmd_tx
        .send(AppCommand::SendMessage("warm-up".into()))
        .unwrap();
    next(&mut rx, finished).await;
    cmd_tx
        .send(AppCommand::SendMessage("delegate in the background".into()))
        .unwrap();
    let run_id = running_run(&mut rx).await;
    next(&mut rx, finished).await;
    // The landing started the roll; it is waiting for room beside the run —
    // a waiting request has not reached the recorder.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        backend.open_at_arrival(COMPACT_KEY).is_empty(),
        "the roll must not stream beside the run: {:?}",
        backend.open_at_arrival(COMPACT_KEY)
    );
    cmd_tx
        .send(AppCommand::StopSubagentRun { id: run_id })
        .unwrap();
    // The run lands, the roll streams, the wake turn displaces it and the
    // roll is made again: collect both ends in whichever order they come.
    let (mut compacted, mut woke) = (false, false);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !(compacted && woke) && std::time::Instant::now() < deadline {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(AppEvent::Compacted { .. })) => compacted = true,
            Ok(Some(AppEvent::Finished { .. })) => woke = true,
            Ok(Some(_)) => {}
            _ => break,
        }
    }
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    assert!(compacted, "the roll completed");
    assert!(woke, "the wake turn completed");
    assert_eq!(
        backend.open_at_arrival(COMPACT_KEY),
        vec![0, 0],
        "the roll arrived once the run's stream was gone, and again once the wake turn's was"
    );
    assert_eq!(
        backend.max_in_flight(),
        1,
        "two sessions, one pool: the run, the roll and the wake turn took turns"
    );
    let requests = backend.requests();
    let is_roll = |r: &crate::shared::api::ChatRequest| {
        r.system.as_deref().is_some_and(|s| s.contains(COMPACT_KEY))
    };
    let rolls: Vec<usize> = (0..requests.len())
        .filter(|&i| is_roll(&requests[i]))
        .collect();
    let wake = requests
        .iter()
        .rposition(|r| {
            r.system
                .as_deref()
                .is_none_or(|s| !s.contains(COMPACT_KEY) && !s.contains("be harsh"))
        })
        .unwrap();
    assert_eq!(rolls.len(), 2, "the roll was made twice");
    assert!(
        rolls[0] < wake && wake < rolls[1],
        "the wake turn streamed between the roll's two attempts: rolls {rolls:?}, wake {wake}"
    );
    let same = |a: &crate::shared::api::ChatRequest, b: &crate::shared::api::ChatRequest| {
        a.messages.len() == b.messages.len()
            && a.messages.last().map(|m| &m.content) == b.messages.last().map(|m| &m.content)
    };
    assert!(
        same(&requests[rolls[0]], &requests[rolls[1]]),
        "the retry is the same request"
    );
    let chat = super::subagent::load(dir.path(), chat_id);
    let compaction = chat.compaction.as_ref().expect("the summary landed");
    assert_eq!(
        compaction.summary, "a summary",
        "the retry's summary, not the displaced stream's fragment"
    );
    assert_eq!(chat.messages.last().unwrap().text, "noted", "the wake turn");
}

/// A reflection round displaced by the user's next turn (silent-preemption
/// §4.4): the turn does not fit beside the round's stream on the pool, the
/// stream ends, the turn streams at once, and the round is made again with
/// the same request — the task lands as a success, its spawn-time watermark
/// honest.
#[tokio::test]
async fn a_reflection_round_displaced_by_a_turn_is_made_again() {
    let backend = KeyedRecorder::new(
        vec![
            ("", vec![text("ok"), text("again")]),
            (REFLECT_KEY, vec![long_text(30), text("reflected")]),
        ],
        30,
    );
    let mut cfg = cfg(1);
    cfg.engine.managed.context_size = 4000;
    cfg.self_model.auto_reflect_every = 1;
    let (_dir, cmd_tx, mut rx, handle, _chat) = spawn_english(backend.clone(), cfg).await;
    cmd_tx.send(AppCommand::SendMessage("one".into())).unwrap();
    next(&mut rx, finished).await;
    // The reflection's first round is streaming (30 chunks at 30 ms).
    settle(2000, || !backend.open_at_arrival(REFLECT_KEY).is_empty()).await;
    cmd_tx.send(AppCommand::SendMessage("two".into())).unwrap();
    next(&mut rx, finished).await;
    settle(3000, || backend.open_at_arrival(REFLECT_KEY).len() >= 2).await;
    next(&mut rx, |e| {
        matches!(
            e,
            AppEvent::BackgroundTask {
                kind: BackgroundKind::Reflection,
                active: false
            }
        )
    })
    .await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    assert_eq!(
        backend.open_at_arrival(REFLECT_KEY),
        vec![0, 0],
        "displaced, then made again once the turn's stream was gone"
    );
    assert_eq!(backend.max_in_flight(), 1);
    let order: Vec<bool> = backend
        .requests()
        .iter()
        .map(|r| r.system.as_deref().is_some_and(|s| s.contains(REFLECT_KEY)))
        .collect();
    assert_eq!(
        order,
        vec![false, true, false, true],
        "the turn streamed between the round's two attempts"
    );
}

/// The automatic title displaced by the user's next turn is made again and
/// lands (silent-preemption §4.4): a title is owed once per point, so the
/// same task re-asks rather than giving up.
#[tokio::test]
async fn the_title_displaced_by_a_turn_is_made_again() {
    let backend = KeyedRecorder::new(
        vec![
            ("", vec![text("ok"), text("again")]),
            (TITLE_KEY, vec![long_text(30), text("A title")]),
        ],
        30,
    );
    let mut cfg = cfg(1);
    cfg.engine.managed.context_size = 4000;
    cfg.interface.auto_title = AutoTitleMode::AfterAssistantReply;
    let (dir, cmd_tx, mut rx, handle, chat_id) = spawn_english(backend.clone(), cfg).await;
    cmd_tx.send(AppCommand::SendMessage("one".into())).unwrap();
    next(&mut rx, finished).await;
    settle(2000, || !backend.open_at_arrival(TITLE_KEY).is_empty()).await;
    cmd_tx.send(AppCommand::SendMessage("two".into())).unwrap();
    next(&mut rx, finished).await;
    settle(3000, || backend.open_at_arrival(TITLE_KEY).len() >= 2).await;
    // The title lands on the list once the retry's stream ends.
    next(
        &mut rx,
        |e| matches!(e, AppEvent::ChatList(list) if list.iter().any(|c| c.title == "A title")),
    )
    .await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    assert_eq!(backend.open_at_arrival(TITLE_KEY), vec![0, 0]);
    assert_eq!(backend.max_in_flight(), 1);
    let chat = super::subagent::load(dir.path(), chat_id);
    assert_eq!(chat.title, "A title");
}

/// Under a pool a silent loop's second round is priced from its first round's
/// exact size (research §4.2, §5): after a round the server sized at 3000, the
/// next reservation cannot fit beside a run holding 2100 of a 6000 pool, so
/// the loop waits for the run — while its first round, small, streamed beside
/// it.
#[tokio::test]
async fn a_silent_loops_later_round_is_floored_by_its_exact_size() {
    let backend = KeyedRecorder::new(
        vec![
            ("", vec![start("c1"), text("started it"), text("noted")]),
            ("be harsh", vec![hang("thinking")]),
            (
                REFLECT_KEY,
                vec![
                    sized(
                        super::subagent::call("r1", GET_SELF_MODEL_ID, "{}"),
                        3000,
                        10,
                    ),
                    text("reflected"),
                ],
            ),
        ],
        30,
    );
    let mut cfg = cfg(2);
    cfg.engine.managed.context_size = 6000;
    cfg.self_model.auto_reflect_every = 1;
    let (_dir, cmd_tx, mut rx, handle, _chat) = spawn_english(backend.clone(), cfg).await;
    cmd_tx
        .send(AppCommand::SendMessage("delegate in the background".into()))
        .unwrap();
    let run_id = running_run(&mut rx).await;
    next(&mut rx, finished).await;
    // The reflection's first round streams beside the run (small beside
    // 2100 on 6000); its second, floored at 3010 + the cap, does not fit
    // and waits.
    settle(2000, || !backend.open_at_arrival(REFLECT_KEY).is_empty()).await;
    assert_eq!(backend.open_at_arrival(REFLECT_KEY), vec![1]);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(
        backend.open_at_arrival(REFLECT_KEY).len(),
        1,
        "the second round waits while the run is out"
    );
    cmd_tx
        .send(AppCommand::StopSubagentRun { id: run_id })
        .unwrap();
    next(&mut rx, runs_out(0)).await;
    settle(2000, || backend.open_at_arrival(REFLECT_KEY).len() >= 2).await;
    assert_eq!(
        backend.open_at_arrival(REFLECT_KEY),
        vec![1, 0],
        "the second round arrived once the run's stream was gone"
    );
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

/// After a landing the roll is asked for **before** the loops (fork F6): the
/// title first (the one the user can see), then the roll — the one silent
/// task that protects the next turn — then reflection.
#[tokio::test]
async fn the_fan_out_asks_for_the_title_then_the_roll_then_the_loops() {
    let backend = KeyedRecorder::new(
        vec![
            ("", vec![text("ok"), sized(text("more"), 3000, 10)]),
            (TITLE_KEY, vec![text("A title")]),
            (COMPACT_KEY, vec![text("a summary")]),
            (REFLECT_KEY, vec![text("reflected")]),
        ],
        20,
    );
    let mut cfg = cfg(1);
    cfg.engine.managed.context_size = 4000;
    cfg.compaction.enabled = true;
    cfg.compaction.threshold_pct = 75;
    cfg.compaction.tail_tokens = 32;
    cfg.self_model.auto_reflect_every = 2;
    cfg.interface.auto_title = AutoTitleMode::AfterAssistantReply;
    let (_dir, cmd_tx, mut rx, handle, _chat) = spawn_english(backend.clone(), cfg).await;
    // Long enough that the second exchange alone outgrows the roll's tail.
    cmd_tx.send(AppCommand::SendMessage(long("one"))).unwrap();
    next(&mut rx, finished).await;
    // The title landed before the next turn: this test is about the order
    // the landing asks in, not about the turn displacing the title's stream
    // (silent-preemption §4.4, `the_title_displaced_by_a_turn_is_made_again`).
    next(
        &mut rx,
        |e| matches!(e, AppEvent::ChatList(list) if list.iter().any(|c| c.title == "A title")),
    )
    .await;
    cmd_tx.send(AppCommand::SendMessage(long("two"))).unwrap();
    next(&mut rx, finished).await;
    settle(3000, || {
        !backend.open_at_arrival(COMPACT_KEY).is_empty()
            && !backend.open_at_arrival(REFLECT_KEY).is_empty()
    })
    .await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let order: Vec<&str> = backend
        .requests()
        .iter()
        .filter_map(|r| {
            let system = r.system.as_deref().unwrap_or_default();
            [TITLE_KEY, COMPACT_KEY, REFLECT_KEY]
                .into_iter()
                .find(|k| system.contains(k))
        })
        .collect();
    assert_eq!(
        order,
        vec![TITLE_KEY, COMPACT_KEY, REFLECT_KEY],
        "the title at the first reply; at the second landing the roll ahead of the reflection"
    );
    assert_eq!(backend.max_in_flight(), 1);
}

/// Impersonation on the shared engine is one of the app's own requests: it
/// takes the silent lane, and the budget names it while it streams — and it
/// **holds** it: an interactive stream that does not fit beside the preview
/// waits it out, since the user's own request is never displaced
/// (silent-preemption §4.3, R4).
#[tokio::test]
async fn impersonation_on_the_shared_engine_takes_the_silent_lane_and_holds_it() {
    let (_d, mut orch, _pid) = orch_with_active_profile();
    // A pool, so a stream can fail to fit beside the preview.
    orch.config.compaction.context_tokens = Some(1000);
    let chat_id = orch.active_id.unwrap();
    orch.chat_mut(chat_id)
        .unwrap()
        .push_message(Message::user("hello"));
    let backend = KeyedRecorder::new(vec![("", vec![long_text(20)])], 40);
    orch.engines.backend = Some(backend.clone() as Arc<dyn EngineBackend>);

    orch.handle_impersonate(String::new());
    let budget = orch.session_budget();
    settle(1000, || budget.silent_streaming() == Some("impersonation")).await;
    assert_eq!(budget.silent_streaming(), Some("impersonation"));
    let calm = CancellationToken::new();
    let mut turn = std::pin::pin!(budget.acquire(900, &calm));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), &mut turn)
            .await
            .is_err(),
        "no room beside the preview: waits"
    );
    assert_eq!(
        budget.silent_streaming(),
        Some("impersonation"),
        "still streaming — not displaced"
    );
    assert!(turn.await.is_some(), "admitted once the preview ended");
    assert_eq!(budget.silent_streaming(), None, "released with the stream");
    assert_eq!(backend.requests().len(), 1, "streamed once, to its end");
}

/// A silent loop spawned by hand on `orch`'s budget: one round at most, the
/// request keyed on `system`, `clock` over its streaming — the pieces the
/// yields cap and the clock are tested on without a landing to trigger.
fn spawn_loop(
    orch: &mut Orchestrator,
    backend: Arc<KeyedRecorder>,
    chat_id: Uuid,
    system: &str,
    clock: std::time::Duration,
) -> (
    CancellationToken,
    UnboundedReceiver<(BackgroundKind, BgOutcome)>,
    Arc<Acted>,
) {
    spawn_loop_allowing(orch, backend, chat_id, system, clock, Vec::new())
}

/// [`spawn_loop`] with a tool set the loop may call — a reader or a writer
/// of the profile's memory, for the tests of what consumes a window
/// (docs/research/acted-by-effect.md §6).
fn spawn_loop_allowing(
    orch: &mut Orchestrator,
    backend: Arc<KeyedRecorder>,
    chat_id: Uuid,
    system: &str,
    clock: std::time::Duration,
    allowed: Vec<crate::entities::profile::ToolId>,
) -> (
    CancellationToken,
    UnboundedReceiver<(BackgroundKind, BgOutcome)>,
    Arc<Acted>,
) {
    let profile_id = orch
        .chats
        .iter()
        .find(|c| c.id == chat_id)
        .unwrap()
        .profile_id;
    let cancel = CancellationToken::new();
    let acted = Arc::new(Acted::default());
    let sessions = orch.session_budget();
    let ctx = orch.background_tool_ctx(
        backend.clone() as Arc<dyn EngineBackend>,
        sessions,
        profile_id,
        chat_id,
        system.to_string(),
        None,
        crate::shared::i18n::Lang::En,
        cancel.clone(),
    );
    let (done_tx, done_rx) = tokio::sync::mpsc::unbounded_channel();
    super::super::tool_loop::spawn_silent_loop(super::super::tool_loop::SilentLoop {
        backend: backend as Arc<dyn EngineBackend>,
        registry: orch.registry.clone(),
        ctx,
        request: crate::shared::api::ChatRequest {
            continue_final: false,
            system: Some(system.to_string()),
            messages: vec![crate::shared::api::ApiMessage::user("reflect")],
            sampling: crate::entities::sampling::SamplingConfig {
                max_tokens: Some(500),
                ..Default::default()
            },
            tools: Vec::new(),
        },
        allowed,
        cancel: cancel.clone(),
        acted: acted.clone(),
        max_rounds: 1,
        timeout: clock,
        label: "test loop",
        profile_id,
        kind: BackgroundKind::Reflection,
        done_tx,
        summary_semantics: None,
    });
    (cancel, done_rx, acted)
}

/// A silent task yields at most `SILENT_YIELDS_MAX` times (silent-preemption
/// §4.4, fork F5): each of the first three interactive streams displaces its
/// round and is admitted at once; the fourth attempt holds, the waiter waits
/// that round out, and the task lands as a success.
#[tokio::test]
async fn a_silent_task_holds_after_its_third_displacement() {
    let (_d, mut orch, chat_id) = orch_ready_for_reflection();
    orch.config.compaction.context_tokens = Some(1000);
    let backend = KeyedRecorder::new(
        vec![(
            "quiet loop",
            (0..4).map(|_| long_text(30)).collect::<Vec<_>>(),
        )],
        30,
    );
    let (_stop, mut done_rx, _acted) = spawn_loop(
        &mut orch,
        backend.clone(),
        chat_id,
        "quiet loop",
        std::time::Duration::from_secs(30),
    );
    let budget = orch.session_budget();
    let calm = CancellationToken::new();
    for yields in 1..=SILENT_YIELDS_MAX {
        settle(2000, || {
            backend.open_at_arrival("quiet loop").len() as u32 == yields
        })
        .await;
        let turn = budget
            .acquire(900, &calm)
            .await
            .expect("the round's stream displaced, the waiter in");
        drop(turn);
    }
    settle(2000, || backend.open_at_arrival("quiet loop").len() == 4).await;
    let mut turn = std::pin::pin!(budget.acquire(900, &calm));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(300), &mut turn)
            .await
            .is_err(),
        "the fourth attempt holds: the waiter waits"
    );
    assert!(turn.await.is_some(), "admitted once the held stream ended");
    let (kind, outcome) = tokio::time::timeout(std::time::Duration::from_secs(5), done_rx.recv())
        .await
        .expect("the task landed")
        .unwrap();
    assert_eq!(kind, BackgroundKind::Reflection);
    assert_eq!(outcome, BgOutcome::Done);
    assert_eq!(
        backend.requests().len(),
        4,
        "three displaced rounds and the held one"
    );
}

/// The loop's clock runs over its streaming and tools, not over its wait for
/// room (silent-preemption §4.5): a task held back longer than its limit
/// still runs when the room comes.
#[tokio::test]
async fn a_silent_loops_wait_for_room_is_not_on_its_clock() {
    let (_d, mut orch, chat_id) = orch_ready_for_reflection();
    orch.config.compaction.context_tokens = Some(1000);
    let backend = KeyedRecorder::new(vec![("quiet loop", vec![text("done")])], 20);
    let budget = orch.session_budget();
    let calm = CancellationToken::new();
    let turn = budget.acquire(900, &calm).await.unwrap();
    let (_stop, mut done_rx, _acted) = spawn_loop(
        &mut orch,
        backend.clone(),
        chat_id,
        "quiet loop",
        std::time::Duration::from_millis(300),
    );
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    assert!(backend.requests().is_empty(), "waiting for room");
    assert!(done_rx.try_recv().is_err(), "not timed out while waiting");
    drop(turn);
    let (_, outcome) = tokio::time::timeout(std::time::Duration::from_secs(5), done_rx.recv())
        .await
        .expect("the task landed")
        .unwrap();
    assert_eq!(outcome, BgOutcome::Done);
    assert_eq!(backend.requests().len(), 1);
}

/// …and a stream longer than the limit ends the task as it always did.
#[tokio::test]
async fn a_silent_loops_stream_is_on_its_clock() {
    let (_d, mut orch, chat_id) = orch_ready_for_reflection();
    let backend = KeyedRecorder::new(vec![("quiet loop", vec![hang("thinking")])], 20);
    let (_stop, mut done_rx, _acted) = spawn_loop(
        &mut orch,
        backend.clone(),
        chat_id,
        "quiet loop",
        std::time::Duration::from_millis(200),
    );
    let (_, outcome) = tokio::time::timeout(std::time::Duration::from_secs(5), done_rx.recv())
        .await
        .expect("the task landed")
        .unwrap();
    let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::En);
    assert_eq!(
        outcome,
        BgOutcome::Failed(loc.t("loop.time_limit_exceeded").to_string())
    );
}

/// A silent request whose wait for room is cancelled leaves no reservation
/// behind and opens no stream: the app was quitting, nothing ran, nothing
/// failed.
#[tokio::test]
async fn a_cancelled_wait_opens_no_stream_and_leaves_no_reservation() {
    let (_d, mut orch, chat_id) = orch_ready_for_reflection();
    orch.config.compaction.context_tokens = Some(1000);
    let backend = KeyedRecorder::new(vec![("", Vec::new())], 20);
    orch.engines.backend = Some(backend.clone() as Arc<dyn EngineBackend>);
    let budget = orch.session_budget();
    // An interactive stream holds most of the pool: the reflection's round
    // (its cap alone is 2048) cannot fit beside it.
    let calm = CancellationToken::new();
    let _turn = budget.acquire(900, &calm).await.unwrap();

    orch.maybe_auto_reflect(chat_id);
    assert!(orch.bg_running(BackgroundKind::Reflection));
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert!(backend.requests().is_empty(), "waiting, not streaming");
    assert_eq!(budget.in_flight(), 900);
    assert_eq!(budget.silent_streaming(), None);

    orch.quit_bg();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert!(
        backend.requests().is_empty(),
        "the cancelled wait never streamed"
    );
    assert_eq!(budget.in_flight(), 900, "no reservation left behind");
    assert_eq!(budget.silent_streaming(), None);
}

/// The notice a stopped manual roll answers with, in whichever interface
/// language the orchestrator is speaking.
fn is_compact_cancelled(e: &AppEvent) -> bool {
    matches!(e, AppEvent::Notice(m) if [crate::shared::i18n::Lang::En, crate::shared::i18n::Lang::Ru]
        .iter()
        .any(|l| m == crate::shared::i18n::locale(*l).t("ui.compact.cancelled")))
}

/// A silent loop stopped mid-stream lands as **cancelled**
/// (docs/research/stop-silent-task.md §3.3): its own token fired, the
/// stream ended on the next chunk, and the outcome is neither a success
/// nor a failure.
#[tokio::test]
async fn a_loop_stopped_mid_stream_lands_cancelled() {
    let (_d, mut orch, chat_id) = orch_ready_for_reflection();
    orch.config.compaction.context_tokens = Some(1000);
    let backend = KeyedRecorder::new(vec![("quiet loop", vec![long_text(30)])], 30);
    let (stop, mut done_rx, _acted) = spawn_loop(
        &mut orch,
        backend.clone(),
        chat_id,
        "quiet loop",
        std::time::Duration::from_secs(30),
    );
    settle(2000, || !backend.open_at_arrival("quiet loop").is_empty()).await;
    stop.cancel();
    let (kind, outcome) = tokio::time::timeout(std::time::Duration::from_secs(5), done_rx.recv())
        .await
        .expect("the task landed")
        .unwrap();
    assert_eq!(kind, BackgroundKind::Reflection);
    assert_eq!(outcome, BgOutcome::Cancelled { consumed: false });
    assert_eq!(backend.requests().len(), 1, "no retry after a stop");
}

/// A silent loop stopped while it waits for room lands as cancelled and
/// opens no stream (R5).
#[tokio::test]
async fn a_loop_stopped_while_waiting_lands_cancelled() {
    let (_d, mut orch, chat_id) = orch_ready_for_reflection();
    orch.config.compaction.context_tokens = Some(1000);
    let backend = KeyedRecorder::new(vec![("quiet loop", vec![text("done")])], 20);
    let budget = orch.session_budget();
    let calm = CancellationToken::new();
    let turn = budget.acquire(900, &calm).await.unwrap();
    let (stop, mut done_rx, _acted) = spawn_loop(
        &mut orch,
        backend.clone(),
        chat_id,
        "quiet loop",
        std::time::Duration::from_secs(30),
    );
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(backend.requests().is_empty(), "waiting for room");
    stop.cancel();
    let (_, outcome) = tokio::time::timeout(std::time::Duration::from_secs(5), done_rx.recv())
        .await
        .expect("the task landed")
        .unwrap();
    assert_eq!(outcome, BgOutcome::Cancelled { consumed: false });
    assert!(
        backend.requests().is_empty(),
        "the stopped wait never streamed"
    );
    drop(turn);
}

/// A silent loop stopped while it waits to re-make a displaced round lands
/// as cancelled, and the waiter that displaced it is unaffected (§4).
#[tokio::test]
async fn a_loop_stopped_during_its_retry_lands_cancelled() {
    let (_d, mut orch, chat_id) = orch_ready_for_reflection();
    orch.config.compaction.context_tokens = Some(1000);
    let backend = KeyedRecorder::new(vec![("quiet loop", vec![long_text(30), long_text(30)])], 30);
    let (stop, mut done_rx, _acted) = spawn_loop(
        &mut orch,
        backend.clone(),
        chat_id,
        "quiet loop",
        std::time::Duration::from_secs(30),
    );
    settle(2000, || !backend.open_at_arrival("quiet loop").is_empty()).await;
    let budget = orch.session_budget();
    let calm = CancellationToken::new();
    // The turn displaces the round and is admitted; the loop's retry now
    // waits for room beside it (500 + 900 > 1000).
    let turn = budget.acquire(900, &calm).await.expect("the round yielded");
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(backend.requests().len(), 1, "the retry is waiting");
    stop.cancel();
    let (_, outcome) = tokio::time::timeout(std::time::Duration::from_secs(5), done_rx.recv())
        .await
        .expect("the task landed")
        .unwrap();
    assert_eq!(outcome, BgOutcome::Cancelled { consumed: false });
    assert_eq!(backend.requests().len(), 1, "the retry never streamed");
    drop(turn);
}

/// A manual `/compact` stopped from the tasks screen answers with one
/// notice and folds nothing; the next `/compact` runs (R4).
#[tokio::test]
async fn a_manual_roll_stopped_answers_with_a_notice_and_the_next_one_runs() {
    let backend = KeyedRecorder::new(
        vec![
            ("", vec![text("ok"), text("again")]),
            (COMPACT_KEY, vec![long_text(30), text("a summary")]),
        ],
        30,
    );
    let mut cfg = cfg(1);
    cfg.engine.managed.context_size = 4000;
    cfg.compaction.enabled = true;
    cfg.compaction.threshold_pct = 0;
    cfg.compaction.tail_tokens = 32;
    let (dir, cmd_tx, mut rx, handle, chat_id) = spawn_english(backend.clone(), cfg).await;
    cmd_tx.send(AppCommand::SendMessage(long("one"))).unwrap();
    next(&mut rx, finished).await;
    cmd_tx.send(AppCommand::SendMessage(long("two"))).unwrap();
    next(&mut rx, finished).await;
    cmd_tx.send(AppCommand::Compact).unwrap();
    settle(2000, || !backend.open_at_arrival(COMPACT_KEY).is_empty()).await;
    cmd_tx
        .send(AppCommand::StopBackgroundTask {
            kind: BackgroundKind::Compaction,
        })
        .unwrap();
    let (mut stopped, mut compacted_early) = (false, false);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !stopped && std::time::Instant::now() < deadline {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(e)) => {
                compacted_early |= matches!(e, AppEvent::Compacted { .. });
                stopped = is_compact_cancelled(&e);
            }
            _ => break,
        }
    }
    assert!(stopped, "the notice arrived");
    assert!(!compacted_early, "nothing was folded");
    cmd_tx.send(AppCommand::Compact).unwrap();
    next(&mut rx, |e| matches!(e, AppEvent::Compacted { .. })).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    assert_eq!(backend.open_at_arrival(COMPACT_KEY), vec![0, 0]);
    let chat = super::subagent::load(dir.path(), chat_id);
    assert_eq!(
        chat.compaction.as_ref().map(|c| c.summary.as_str()),
        Some("a summary"),
        "the second roll's summary, never the stopped stream's fragment"
    );
}

/// An automatic roll stopped from the tasks screen says nothing and is
/// planned again at the next landing, the conversation still being over the
/// threshold (R4, fork F5).
#[tokio::test]
async fn an_automatic_roll_stopped_is_quiet_and_planned_again_at_the_next_landing() {
    let backend = KeyedRecorder::new(
        vec![
            (
                "",
                vec![
                    sized(text("ok"), 3000, 10),
                    sized(text("again"), 3000, 10),
                    sized(text("more"), 3000, 10),
                ],
            ),
            (COMPACT_KEY, vec![long_text(30), text("a summary")]),
        ],
        30,
    );
    let mut cfg = cfg(1);
    cfg.engine.managed.context_size = 4000;
    cfg.compaction.enabled = true;
    cfg.compaction.threshold_pct = 75;
    cfg.compaction.tail_tokens = 32;
    let (dir, cmd_tx, mut rx, handle, chat_id) = spawn_english(backend.clone(), cfg).await;
    // The first landing is over the threshold but has one exchange and
    // nothing to fold; the second starts the roll.
    cmd_tx.send(AppCommand::SendMessage(long("one"))).unwrap();
    next(&mut rx, finished).await;
    cmd_tx.send(AppCommand::SendMessage(long("two"))).unwrap();
    next(&mut rx, finished).await;
    settle(2000, || !backend.open_at_arrival(COMPACT_KEY).is_empty()).await;
    cmd_tx
        .send(AppCommand::StopBackgroundTask {
            kind: BackgroundKind::Compaction,
        })
        .unwrap();
    next(&mut rx, |e| {
        matches!(
            e,
            AppEvent::BackgroundTask {
                kind: BackgroundKind::Compaction,
                active: false
            }
        )
    })
    .await;
    // Quiet: no notice, no error, nothing folded.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    while let Ok(e) = rx.try_recv() {
        assert!(
            !matches!(
                e,
                AppEvent::Notice(_) | AppEvent::Error(_) | AppEvent::Compacted { .. }
            ),
            "an automatic roll stops quietly: {e:?}"
        );
    }
    cmd_tx.send(AppCommand::SendMessage(long("three"))).unwrap();
    next(&mut rx, finished).await;
    next(&mut rx, |e| matches!(e, AppEvent::Compacted { .. })).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    assert_eq!(
        backend.open_at_arrival(COMPACT_KEY),
        vec![0, 0],
        "the roll was planned again at the next landing"
    );
    let chat = super::subagent::load(dir.path(), chat_id);
    assert_eq!(
        chat.compaction.as_ref().map(|c| c.summary.as_str()),
        Some("a summary")
    );
}

/// One round that calls a tool — the shape that makes a round's tools run
/// before the next stream (docs/research/stop-refunds-window.md §3.2).
fn one_call() -> super::subagent::Script {
    super::subagent::Script {
        chunks: vec![
            ChatChunk::ToolCall(crate::shared::api::contract::ToolCallDelta {
                thought_signature: None,
                index: 0,
                id: Some("c1".into()),
                name: Some("get_self_model".into()),
                arguments: "{}".into(),
            }),
            ChatChunk::Finished(FinishReason::ToolCalls),
        ],
        hang: false,
    }
}

// ---------- a quit gives the window back (docs/research/quit-refunds-window.md) ----------

/// A quit mid-reflection, through the orchestrator's own loop (§3.2): the
/// reflection was still in its first stream, so the chat on disk after the
/// exit flush carries the watermark and stamp from before the spawn — the
/// next launch reflects on the same replies.
#[tokio::test]
async fn a_quit_mid_reflection_gives_the_window_back() {
    let backend = KeyedRecorder::new(
        vec![
            ("", vec![text("ok")]),
            (REFLECT_KEY, vec![hang("thinking")]),
        ],
        30,
    );
    let mut cfg = cfg(1);
    cfg.engine.managed.context_size = 4000;
    cfg.self_model.auto_reflect_every = 1;
    let (dir, cmd_tx, mut rx, handle, chat_id) = spawn_english(backend.clone(), cfg).await;
    cmd_tx.send(AppCommand::SendMessage("one".into())).unwrap();
    next(&mut rx, finished).await;
    settle(3000, || !backend.open_at_arrival(REFLECT_KEY).is_empty()).await;
    assert_eq!(
        backend.open_at_arrival(REFLECT_KEY).len(),
        1,
        "the reflection is streaming"
    );
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = super::subagent::load(dir.path(), chat_id);
    assert_eq!(chat.reflected_upto, None, "the window is unread again");
    assert_eq!(chat.reflected_at, None);
}

// ---------- "acted on" by effect (docs/research/acted-by-effect.md) ----------

/// One round that calls the self-model writer with a change — the shape
/// that consumes a window (docs/research/acted-by-effect.md §3.1).
fn write_call() -> super::subagent::Script {
    super::subagent::Script {
        chunks: vec![
            ChatChunk::ToolCall(crate::shared::api::contract::ToolCallDelta {
                thought_signature: None,
                index: 0,
                id: Some("w1".into()),
                name: Some(UPDATE_SELF_MODEL_ID.into()),
                arguments: r#"{"summary": "I value brevity"}"#.into(),
            }),
            ChatChunk::Finished(FinishReason::ToolCalls),
        ],
        hang: false,
    }
}

async fn landed(done: &mut UnboundedReceiver<(BackgroundKind, BgOutcome)>) -> BgOutcome {
    tokio::time::timeout(std::time::Duration::from_secs(5), done.recv())
        .await
        .expect("the task landed")
        .unwrap()
        .1
}

/// A round of reads consumes nothing (§3.3): a loop whose first round called
/// `get_self_model` — allowed, and run — and whose second stream is stopped
/// lands `consumed: false`, its state back at `Idle`; a loop whose round
/// wrote lands `consumed: true` and stays `Wrote`; one stopped in its first
/// stream never left `Idle`.
#[tokio::test]
async fn a_round_of_reads_consumes_nothing_and_a_write_does() {
    let (_d, mut orch, chat_id) = orch_ready_for_reflection();
    orch.config.compaction.context_tokens = Some(1000);
    let backend = KeyedRecorder::new(
        vec![
            ("reading loop", vec![one_call(), long_text(30)]),
            ("writing loop", vec![write_call(), long_text(30)]),
            ("quiet loop", vec![long_text(30)]),
        ],
        30,
    );
    let clock = std::time::Duration::from_secs(30);

    let (stop, mut done, acted) = spawn_loop_allowing(
        &mut orch,
        backend.clone(),
        chat_id,
        "reading loop",
        clock,
        vec![GET_SELF_MODEL_ID.into()],
    );
    settle(3000, || backend.open_at_arrival("reading loop").len() == 2).await;
    assert_eq!(acted.get(), Acting::Idle, "a round of reads: back to idle");
    stop.cancel();
    assert_eq!(
        landed(&mut done).await,
        BgOutcome::Cancelled { consumed: false }
    );

    let (stop, mut done, acted) = spawn_loop_allowing(
        &mut orch,
        backend.clone(),
        chat_id,
        "writing loop",
        clock,
        vec![UPDATE_SELF_MODEL_ID.into()],
    );
    settle(3000, || backend.open_at_arrival("writing loop").len() == 2).await;
    assert_eq!(acted.get(), Acting::Wrote, "the writer reported");
    stop.cancel();
    assert_eq!(
        landed(&mut done).await,
        BgOutcome::Cancelled { consumed: true }
    );

    let (stop, mut done, acted) =
        spawn_loop(&mut orch, backend.clone(), chat_id, "quiet loop", clock);
    settle(2000, || !backend.open_at_arrival("quiet loop").is_empty()).await;
    stop.cancel();
    assert_eq!(
        landed(&mut done).await,
        BgOutcome::Cancelled { consumed: false }
    );
    assert_eq!(acted.get(), Acting::Idle, "never in a round of tools");
}

/// A call outside the task's set runs nothing and reports nothing: the
/// window stays refundable.
#[tokio::test]
async fn a_disallowed_call_consumes_nothing() {
    let (_d, mut orch, chat_id) = orch_ready_for_reflection();
    orch.config.compaction.context_tokens = Some(1000);
    let backend = KeyedRecorder::new(
        vec![("refused loop", vec![write_call(), long_text(30)])],
        30,
    );
    let (stop, mut done, acted) = spawn_loop(
        &mut orch,
        backend.clone(),
        chat_id,
        "refused loop",
        std::time::Duration::from_secs(30),
    );
    settle(3000, || backend.open_at_arrival("refused loop").len() == 2).await;
    assert_eq!(acted.get(), Acting::Idle);
    stop.cancel();
    assert_eq!(
        landed(&mut done).await,
        BgOutcome::Cancelled { consumed: false }
    );
}

/// A quit after a reflection's first round that only read gives the window
/// back — through the orchestrator's own loop, the chat read from disk
/// (§3.2): the reads changed nothing.
#[tokio::test]
async fn a_quit_after_a_round_of_reads_gives_the_window_back() {
    let backend = KeyedRecorder::new(
        vec![
            ("", vec![text("ok")]),
            (REFLECT_KEY, vec![one_call(), hang("thinking")]),
        ],
        30,
    );
    let mut cfg = cfg(1);
    cfg.engine.managed.context_size = 4000;
    cfg.self_model.auto_reflect_every = 1;
    let (dir, cmd_tx, mut rx, handle, chat_id) = spawn_english(backend.clone(), cfg).await;
    cmd_tx.send(AppCommand::SendMessage("one".into())).unwrap();
    next(&mut rx, finished).await;
    settle(3000, || backend.open_at_arrival(REFLECT_KEY).len() == 2).await;
    assert_eq!(
        backend.open_at_arrival(REFLECT_KEY).len(),
        2,
        "a round of reads ran"
    );
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = super::subagent::load(dir.path(), chat_id);
    assert_eq!(chat.reflected_upto, None, "reads consumed nothing");
    assert_eq!(chat.reflected_at, None);
}

/// …and a reflection whose first round wrote keeps its advance at a quit
/// (R2): the write is in the store, and the window must not be read twice.
#[tokio::test]
async fn a_quit_after_a_write_keeps_the_advance() {
    let backend = KeyedRecorder::new(
        vec![
            ("", vec![text("ok")]),
            (REFLECT_KEY, vec![write_call(), hang("thinking")]),
        ],
        30,
    );
    let mut cfg = cfg(1);
    cfg.engine.managed.context_size = 4000;
    cfg.self_model.auto_reflect_every = 1;
    let (dir, cmd_tx, mut rx, handle, chat_id) = spawn_english(backend.clone(), cfg).await;
    cmd_tx.send(AppCommand::SendMessage("one".into())).unwrap();
    next(&mut rx, finished).await;
    settle(3000, || backend.open_at_arrival(REFLECT_KEY).len() == 2).await;
    assert_eq!(
        backend.open_at_arrival(REFLECT_KEY).len(),
        2,
        "the write ran"
    );
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = super::subagent::load(dir.path(), chat_id);
    assert!(
        chat.reflected_upto.is_some(),
        "kept: the window was written into"
    );
    assert!(chat.reflected_at.is_some());
}

//! Orchestrator tests — the silent lane (docs/research/silent-tasks-budget.md
//! §4, §7): the app's own background requests — the title, the three silent
//! loops, the compaction roll, impersonation on the shared engine — under the
//! app-wide session budget. Part of the [`super`] module (fixtures in mod.rs;
//! the keyed engine in parallel.rs; the background fixtures in background.rs).

use super::background::{cfg, finished, next, running_run, runs_out, start};
use super::parallel::{KeyedRecorder, long_text, sized};
use super::subagent::{hang, text};
use super::*;
use crate::entities::note::Note;
use crate::features::tools::notes::SELF_NOTE_TAG;
use crate::features::tools::self_model::GET_SELF_MODEL_ID;
use crate::shared::config::AutoTitleMode;
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

/// The probe's arm 1 as a unit test (research §3.1): a compaction roll
/// asked for while a background run streams, on a pool the two do not fit
/// together — the roll **waits** for the run to end (it arrives with no
/// stream open); the wake turn the landing starts then does not fit beside
/// the roll and waits for it in turn. Two sessions, so the permits allow two
/// at once and only the pool serialises: nothing ever overlaps, and all
/// three complete.
#[tokio::test]
async fn the_roll_waits_for_the_run_and_the_wake_turn_waits_for_the_roll() {
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
            (COMPACT_KEY, vec![long_text(30)]),
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
    // The run lands, the roll streams, the wake turn follows it: collect
    // both ends in whichever order they come.
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

    assert!(compacted, "the roll completed after the run ended");
    assert!(woke, "the wake turn completed after the roll");
    assert_eq!(
        backend.open_at_arrival(COMPACT_KEY),
        vec![0],
        "the roll arrived once the run's stream was gone"
    );
    assert_eq!(
        backend.max_in_flight(),
        1,
        "two sessions, one pool: the run, the roll and the wake turn took turns"
    );
    let chat = super::subagent::load(dir.path(), chat_id);
    assert!(chat.compaction.is_some(), "the summary landed");
    assert_eq!(chat.messages.last().unwrap().text, "noted", "the wake turn");
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
/// takes the silent lane, and the budget names it while it streams.
#[tokio::test]
async fn impersonation_on_the_shared_engine_takes_the_silent_lane() {
    let (_d, mut orch, _pid) = orch_with_active_profile();
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
    settle(3000, || budget.silent_streaming().is_none()).await;
    assert_eq!(budget.silent_streaming(), None, "released with the stream");
    assert_eq!(backend.requests().len(), 1);
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

    orch.cancel_all_bg();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert!(
        backend.requests().is_empty(),
        "the cancelled wait never streamed"
    );
    assert_eq!(budget.in_flight(), 900, "no reservation left behind");
    assert_eq!(budget.silent_streaming(), None);
}

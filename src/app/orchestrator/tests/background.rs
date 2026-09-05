//! Orchestrator tests — background sub-agent runs (spec §9.3.2,
//! docs/research/background-subagents.md §4, §7): a `start_subagent` call
//! answers at once and its run outlives the turn; the record lands with the
//! turn as a placeholder and is filled in by id when the run ends; the result
//! is a task notification the chat stores and the next turn carries, and a
//! turn the app starts itself when the chat is open and idle; `Esc` leaves
//! the run alone, `/subagents stop`, the exchange's deletion and `Quit` end
//! it; the cap refuses; the runs share the turns' session budget. Part of the
//! [`super`] module (fixtures in mod.rs; the keyed engine in parallel.rs; the
//! scripts in subagent.rs).

use super::parallel::KeyedRecorder;
use super::subagent::{Script, call, hang, load, text};
use super::*;
use crate::entities::message::MessageRole;
use crate::entities::subagent::RunOutcome;
use crate::shared::api::contract::ApiRole;
use crate::shared::api::contract::ToolCallDelta;

const CRITIC: &str = r#"{"name":"Critic","system_message":"be harsh","message":"rate X"}"#;

/// One round with one `start_subagent` call.
fn start(id: &str) -> Script {
    call(id, "start_subagent", CRITIC)
}

/// One round with two `start_subagent` calls — the cap's case.
fn two_starts(a: &str, b: &str) -> Script {
    let delta = |index: usize, id: &str| {
        ChatChunk::ToolCall(ToolCallDelta {
            thought_signature: None,
            index,
            id: Some(id.into()),
            name: Some("start_subagent".into()),
            arguments: CRITIC.into(),
        })
    };
    Script {
        chunks: vec![
            delta(0, a),
            delta(1, b),
            ChatChunk::Finished(FinishReason::ToolCalls),
        ],
        hang: false,
    }
}

fn cfg(sessions: u32) -> AppConfig {
    let mut cfg = no_auto_cfg();
    cfg.tools.subagent_background = true;
    cfg.engine.managed.sessions = sessions;
    // A roomy pool: the permit count is the only bound these tests measure
    // (see parallel.rs's `cfg`).
    cfg.engine.managed.context_size = 65_536;
    cfg
}

/// The orchestrator up on `backend`, the first chat activated and the
/// parent's message sent.
async fn begin(
    backend: Arc<KeyedRecorder>,
    cfg: AppConfig,
) -> (
    tempfile::TempDir,
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
    Uuid,
) {
    let (dir, cmd_tx, mut evt_rx, handle) =
        spawn_orch_cfg(Some(backend as Arc<dyn EngineBackend>), cfg);
    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let AppEvent::ChatActivated { id: chat_id, .. } = active else {
        unreachable!()
    };
    cmd_tx
        .send(AppCommand::SendMessage("delegate in the background".into()))
        .unwrap();
    (dir, cmd_tx, evt_rx, handle, chat_id)
}

async fn next(rx: &mut UnboundedReceiver<AppEvent>, pred: impl Fn(&AppEvent) -> bool) -> AppEvent {
    wait_for(rx, pred).await.expect("the event arrives")
}

fn finished(e: &AppEvent) -> bool {
    matches!(e, AppEvent::Finished { .. })
}

fn runs_out(n: u32) -> impl Fn(&AppEvent) -> bool {
    move |e| matches!(e, AppEvent::BackgroundRuns { out } if *out == n)
}

/// The id of a background run the list marks *running* — from a `ChatList`
/// snapshot, the way the screen learns it.
fn running_child(e: &AppEvent) -> Option<Uuid> {
    let AppEvent::ChatList(chats) = e else {
        return None;
    };
    chats
        .iter()
        .flat_map(|c| c.children.iter())
        .find(|c| c.running && c.background)
        .map(|c| c.id)
}

async fn running_run(rx: &mut UnboundedReceiver<AppEvent>) -> Uuid {
    let e = next(rx, |e| running_child(e).is_some()).await;
    running_child(&e).unwrap()
}

/// Nothing matching `pred` arrives for a short while — what "the run is
/// still out" or "no turn was started" looks like from the event stream.
async fn nothing_like(rx: &mut UnboundedReceiver<AppEvent>, pred: impl Fn(&AppEvent) -> bool) {
    let seen =
        tokio::time::timeout(std::time::Duration::from_millis(200), wait_for(rx, pred)).await;
    assert!(seen.is_err(), "unexpected event: {seen:?}");
}

fn roles(chat: &Chat) -> Vec<MessageRole> {
    chat.messages.iter().map(|m| m.role).collect()
}

/// A run that ends **while the parent's turn is still streaming**: its result
/// is held, appended after the turn's rows once they land, and the assistant
/// is woken on it — a turn with no new user message whose request ends in
/// the notification as user text. The record landed as a placeholder and is
/// filled in by id.
#[tokio::test]
async fn a_run_ending_during_the_turn_is_delivered_at_landing_and_wakes_the_assistant() {
    let backend = KeyedRecorder::new(
        vec![
            (
                "",
                vec![start("c1"), hang("started it"), text("the review is in")],
            ),
            ("be harsh", vec![text("harsh view")]),
        ],
        10,
    );
    let (dir, cmd_tx, mut rx, handle, chat_id) = begin(backend.clone(), cfg(2)).await;
    next(&mut rx, runs_out(1)).await;
    next(&mut rx, runs_out(0)).await;
    // The parent is still hanging on its second round: end it.
    cmd_tx.send(AppCommand::Cancel).unwrap();
    next(&mut rx, finished).await;
    // The wake turn on the notification.
    next(&mut rx, finished).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = load(dir.path(), chat_id);
    assert_eq!(
        roles(&chat),
        [
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::Tool,
            MessageRole::Assistant,
            MessageRole::System,
            MessageRole::Assistant,
        ],
        "{:?}",
        chat.messages
    );
    let run = chat.messages[1].tool_calls[0]
        .subagent
        .as_deref()
        .expect("the placeholder became the run");
    assert!(run.background);
    assert_eq!(run.outcome, Some(RunOutcome::Completed));
    assert_eq!(run.final_reply(), Some("harsh view"));
    // The started line names the transcript, so the model can cite it.
    let address = crate::features::chat_links::uri(run.id);
    assert!(
        chat.messages[2].text.contains(&address),
        "{}",
        chat.messages[2].text
    );
    // The notification: a System row for the feed, the run's id on it, the
    // reply and the address in its text.
    let note = &chat.messages[4];
    assert_eq!(note.notification, Some(run.id));
    assert!(
        note.text.contains("harsh view") && note.text.contains(&address),
        "{}",
        note.text
    );
    assert_eq!(chat.messages[5].text, "the review is in");

    // The wake request ends in the notification, as user text, alone.
    let last = backend.requests().last().unwrap().clone();
    let tail = last.messages.last().unwrap();
    assert_eq!(tail.role, ApiRole::User);
    assert!(tail.content.contains("harsh view"), "{}", tail.content);
}

/// With one session the run and the turns take turns: the child's stream
/// and the parent's second round never overlap, nor does the wake turn's —
/// one `Arc` of the budget for all of them.
#[tokio::test]
async fn with_one_session_the_run_and_the_turns_take_turns() {
    let backend = KeyedRecorder::new(
        vec![
            (
                "",
                vec![start("c1"), text("started it"), text("the review is in")],
            ),
            ("be harsh", vec![text("harsh view")]),
        ],
        20,
    );
    let (dir, cmd_tx, mut rx, handle, chat_id) = begin(backend.clone(), cfg(1)).await;
    next(&mut rx, finished).await;
    next(&mut rx, runs_out(0)).await;
    next(&mut rx, finished).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    assert_eq!(backend.max_in_flight(), 1, "one permit, everything in turn");
    let chat = load(dir.path(), chat_id);
    assert_eq!(
        roles(&chat),
        [
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::Tool,
            MessageRole::Assistant,
            MessageRole::System,
            MessageRole::Assistant,
        ]
    );
    assert_eq!(chat.messages[5].text, "the review is in");
}

/// A run that outlives its turn is a *running* row of the list; `/subagents
/// stop` ends it, it lands `cancelled` with what it had, and the notification
/// says so.
#[tokio::test]
async fn a_run_outliving_the_turn_is_stopped_and_lands_cancelled() {
    let backend = KeyedRecorder::new(
        vec![
            ("", vec![start("c1"), text("started it"), text("noted")]),
            ("be harsh", vec![hang("thinking")]),
        ],
        10,
    );
    let (dir, cmd_tx, mut rx, handle, chat_id) = begin(backend.clone(), cfg(2)).await;
    let run_id = running_run(&mut rx).await;
    next(&mut rx, finished).await;
    cmd_tx
        .send(AppCommand::StopSubagentRun { id: run_id })
        .unwrap();
    next(&mut rx, runs_out(0)).await;
    next(&mut rx, finished).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = load(dir.path(), chat_id);
    let run = chat.messages[1].tool_calls[0].subagent.as_deref().unwrap();
    assert_eq!(run.id, run_id);
    assert_eq!(run.outcome, Some(RunOutcome::Cancelled));
    assert!(run.background);
    assert_eq!(run.final_reply(), Some("thinking"), "the partial landed");
    let note = chat
        .messages
        .iter()
        .find(|m| m.is_notification())
        .expect("a notification");
    assert!(
        note.text
            .contains(&crate::features::chat_links::uri(run_id)),
        "{}",
        note.text
    );
    assert_eq!(chat.messages.last().unwrap().text, "noted", "the wake turn");
}

/// `Esc` ends the turn and not the run: it is still out afterwards, and
/// stopping it is a separate act.
#[tokio::test]
async fn esc_ends_the_turn_and_leaves_the_run_out() {
    let backend = KeyedRecorder::new(
        vec![
            ("", vec![start("c1"), hang("started it"), text("noted")]),
            ("be harsh", vec![hang("thinking")]),
        ],
        10,
    );
    let (dir, cmd_tx, mut rx, handle, chat_id) = begin(backend.clone(), cfg(2)).await;
    let run_id = running_run(&mut rx).await;
    cmd_tx.send(AppCommand::Cancel).unwrap();
    next(&mut rx, finished).await;
    nothing_like(&mut rx, runs_out(0)).await;
    cmd_tx
        .send(AppCommand::StopSubagentRun { id: run_id })
        .unwrap();
    next(&mut rx, runs_out(0)).await;
    next(&mut rx, finished).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = load(dir.path(), chat_id);
    let run = chat.messages[1].tool_calls[0].subagent.as_deref().unwrap();
    assert_eq!(run.outcome, Some(RunOutcome::Cancelled));
    assert!(chat.messages.iter().any(|m| m.is_notification()));
}

/// The transcript of a run out in the background opens with a `LiveTurn`
/// marked `background` — the screen's cue that `Esc` leaves it and `F6`
/// stops the run (spec §9.3.2) — where the parent's own mid-turn activation
/// is a turn's, and moving between the two cancels nothing.
#[tokio::test]
async fn a_background_transcript_activates_as_a_background_live_turn() {
    let backend = KeyedRecorder::new(
        vec![
            ("", vec![start("c1"), hang("started it"), text("noted")]),
            ("be harsh", vec![hang("thinking")]),
        ],
        10,
    );
    let (dir, cmd_tx, mut rx, handle, chat_id) = begin(backend.clone(), cfg(2)).await;
    let run_id = running_run(&mut rx).await;

    let live_turn = |e: &AppEvent, id: Uuid| -> Option<Option<bool>> {
        match e {
            AppEvent::ChatActivated {
                id: got, live_turn, ..
            } if *got == id => Some(live_turn.as_ref().map(|l| l.background)),
            _ => None,
        }
    };
    cmd_tx.send(AppCommand::SwitchChat(run_id)).unwrap();
    let e = next(&mut rx, |e| live_turn(e, run_id).is_some()).await;
    assert_eq!(
        live_turn(&e, run_id),
        Some(Some(true)),
        "the transcript streams a background run"
    );
    cmd_tx.send(AppCommand::SwitchChat(chat_id)).unwrap();
    let e = next(&mut rx, |e| live_turn(e, chat_id).is_some()).await;
    assert_eq!(
        live_turn(&e, chat_id),
        Some(Some(false)),
        "the parent is still mid-turn, and the turn is a turn"
    );
    // Neither switch ended anything: the turn is still hanging, the run out.
    nothing_like(&mut rx, finished).await;
    cmd_tx.send(AppCommand::Cancel).unwrap();
    next(&mut rx, finished).await;
    cmd_tx
        .send(AppCommand::StopSubagentRun { id: run_id })
        .unwrap();
    next(&mut rx, runs_out(0)).await;
    next(&mut rx, finished).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    let chat = load(dir.path(), chat_id);
    assert!(
        !chat.unread,
        "a result landing in the open chat marks nothing"
    );
}

/// A run that **ended** while its chat's turn was still running waits for
/// that turn to land, and its transcript is no longer streaming: opening it
/// carries no live turn, so the screen offers no stop key for a run there is
/// nothing left to stop (spec §11.2).
#[tokio::test]
async fn an_ended_run_waiting_to_land_opens_without_a_live_turn() {
    let backend = KeyedRecorder::new(
        vec![
            (
                "",
                vec![start("c1"), hang("started it"), text("the review is in")],
            ),
            ("be harsh", vec![text("harsh view")]),
        ],
        10,
    );
    let (dir, cmd_tx, mut rx, handle, chat_id) = begin(backend.clone(), cfg(2)).await;
    let run_id = running_run(&mut rx).await;
    // The run ends while the parent's second round still hangs: the seat is
    // there, ended, and the record is not in `Chat` yet.
    next(&mut rx, runs_out(0)).await;

    cmd_tx.send(AppCommand::SwitchChat(run_id)).unwrap();
    let e = next(
        &mut rx,
        |e| matches!(e, AppEvent::ChatActivated { id, .. } if *id == run_id),
    )
    .await;
    let AppEvent::ChatActivated {
        live_turn,
        messages,
        ..
    } = e
    else {
        unreachable!()
    };
    assert!(live_turn.is_none(), "nothing streams in an ended run");
    assert!(
        messages.iter().any(|m| m.text.contains("harsh view")),
        "the mirror's rounds are still what the transcript shows: {messages:?}"
    );

    cmd_tx.send(AppCommand::Cancel).unwrap();
    next(&mut rx, finished).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    let chat = load(dir.path(), chat_id);
    let run = chat.messages[1].tool_calls[0].subagent.as_deref().unwrap();
    assert_eq!(run.outcome, Some(RunOutcome::Completed), "it landed anyway");
}

/// The list's card of `id`, from a `ChatList` snapshot.
fn card_of(e: &AppEvent, id: Uuid) -> Option<crate::entities::chat::ChatSummary> {
    let AppEvent::ChatList(chats) = e else {
        return None;
    };
    chats.iter().find(|c| c.id == id).cloned()
}

/// Ends the turn in the first chat and opens a second one, so the run's
/// result lands in a chat the user is not looking at.
async fn look_away(
    cmd_tx: &UnboundedSender<AppCommand>,
    rx: &mut UnboundedReceiver<AppEvent>,
    chat_id: Uuid,
) -> Uuid {
    next(rx, finished).await;
    cmd_tx
        .send(AppCommand::NewChat { profile_id: None })
        .unwrap();
    let e = next(
        rx,
        |e| matches!(e, AppEvent::ChatActivated { id, .. } if *id != chat_id),
    )
    .await;
    let AppEvent::ChatActivated { id, .. } = e else {
        unreachable!()
    };
    id
}

/// A result landing in a chat the user is not looking at marks the chat
/// **unread** — on the list and in its file — and starts no turn there;
/// the mark survives a restart, so a result nobody came back to is still
/// visible after `Quit` (spec §11.2).
#[tokio::test]
async fn a_result_landing_in_a_closed_chat_marks_it_unread_and_the_mark_persists() {
    let backend = KeyedRecorder::new(
        vec![
            (
                "",
                vec![start("c1"), text("started it"), text("never asked")],
            ),
            ("be harsh", vec![hang("thinking")]),
        ],
        10,
    );
    let (dir, cmd_tx, mut rx, handle, chat_id) = begin(backend.clone(), cfg(2)).await;
    let run_id = running_run(&mut rx).await;
    look_away(&cmd_tx, &mut rx, chat_id).await;

    cmd_tx
        .send(AppCommand::StopSubagentRun { id: run_id })
        .unwrap();
    next(&mut rx, runs_out(0)).await;
    let e = next(&mut rx, |e| card_of(e, chat_id).is_some_and(|c| c.unread)).await;
    assert!(card_of(&e, chat_id).unwrap().unread);
    // No wake into a chat nobody is looking at (research fork F2).
    nothing_like(&mut rx, finished).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = load(dir.path(), chat_id);
    assert!(chat.unread, "the mark is in the file");
    assert!(chat.messages.iter().any(|m| m.is_notification()));
    assert_ne!(
        chat.messages.last().unwrap().text,
        "never asked",
        "no turn was started in the closed chat"
    );
}

/// Opening the chat is reading it: the mark clears on the list and in the
/// file, and the notification is still there for the next message.
#[tokio::test]
async fn opening_the_chat_clears_the_unread_mark() {
    let backend = KeyedRecorder::new(
        vec![
            (
                "",
                vec![start("c1"), text("started it"), text("never asked")],
            ),
            ("be harsh", vec![hang("thinking")]),
        ],
        10,
    );
    let (dir, cmd_tx, mut rx, handle, chat_id) = begin(backend.clone(), cfg(2)).await;
    let run_id = running_run(&mut rx).await;
    look_away(&cmd_tx, &mut rx, chat_id).await;
    cmd_tx
        .send(AppCommand::StopSubagentRun { id: run_id })
        .unwrap();
    next(&mut rx, |e| card_of(e, chat_id).is_some_and(|c| c.unread)).await;

    cmd_tx.send(AppCommand::SwitchChat(chat_id)).unwrap();
    let e = next(&mut rx, |e| card_of(e, chat_id).is_some_and(|c| !c.unread)).await;
    assert!(!card_of(&e, chat_id).unwrap().unread);
    next(
        &mut rx,
        |e| matches!(e, AppEvent::ChatActivated { id, .. } if *id == chat_id),
    )
    .await;
    // Opening it starts no turn either: the note waits for the next message.
    nothing_like(&mut rx, finished).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = load(dir.path(), chat_id);
    assert!(!chat.unread);
    assert!(chat.messages.last().unwrap().is_notification());
}

/// With the wake off the notification waits for the user's next message —
/// and travels merged in front of it, as one user message on the wire.
#[tokio::test]
async fn without_the_wake_the_notification_rides_the_next_message() {
    let backend = KeyedRecorder::new(
        vec![
            ("", vec![start("c1"), text("started it"), text("got it")]),
            ("be harsh", vec![text("harsh view")]),
        ],
        10,
    );
    let mut cfg = cfg(2);
    cfg.tools.subagent_background_wake = false;
    let (dir, cmd_tx, mut rx, handle, chat_id) = begin(backend.clone(), cfg).await;
    next(&mut rx, finished).await;
    next(&mut rx, runs_out(0)).await;
    nothing_like(&mut rx, finished).await;
    cmd_tx
        .send(AppCommand::SendMessage("thanks".into()))
        .unwrap();
    next(&mut rx, finished).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = load(dir.path(), chat_id);
    assert_eq!(
        roles(&chat),
        [
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::Tool,
            MessageRole::Assistant,
            MessageRole::System,
            MessageRole::User,
            MessageRole::Assistant,
        ],
        "{:?}",
        chat.messages
    );
    let last = backend.requests().last().unwrap().clone();
    let users: Vec<&str> = last
        .messages
        .iter()
        .filter(|m| m.role == ApiRole::User)
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(
        users.len(),
        2,
        "one merged user message, not two: {users:?}"
    );
    assert!(
        users[1].contains("harsh view") && users[1].ends_with("thanks"),
        "{}",
        users[1]
    );
}

/// Past the cap a second `start_subagent` in the same reply is refused with
/// a result that names the setting; the first runs.
#[tokio::test]
async fn the_cap_refuses_the_next_start_and_names_the_setting() {
    let backend = KeyedRecorder::new(
        vec![
            ("", vec![two_starts("c1", "c2"), text("ok"), text("noted")]),
            ("be harsh", vec![hang("thinking")]),
        ],
        10,
    );
    let mut cfg = cfg(2);
    cfg.tools.subagent_background_max = 1;
    let (dir, cmd_tx, mut rx, handle, chat_id) = begin(backend.clone(), cfg).await;
    let run_id = running_run(&mut rx).await;
    next(&mut rx, finished).await;
    cmd_tx
        .send(AppCommand::StopSubagentRun { id: run_id })
        .unwrap();
    next(&mut rx, runs_out(0)).await;
    next(&mut rx, finished).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = load(dir.path(), chat_id);
    let calls = &chat.messages[1].tool_calls;
    assert_eq!(calls.len(), 2);
    assert!(calls[0].subagent.is_some(), "the first started");
    assert!(calls[1].subagent.is_none(), "the second did not");
    let refusal = calls[1].result.as_deref().unwrap();
    assert!(
        refusal.contains("subagent_background_max"),
        "the refusal names the cap: {refusal}"
    );
}

/// `Quit` lands every run out as `cancelled`, from its mirror, before the
/// exit flush — the file never reads as unfinished after a clean exit.
#[tokio::test]
async fn quit_lands_every_run_cancelled() {
    let backend = KeyedRecorder::new(
        vec![
            ("", vec![start("c1"), text("started it")]),
            ("be harsh", vec![hang("thinking")]),
        ],
        10,
    );
    let (dir, cmd_tx, mut rx, handle, chat_id) = begin(backend.clone(), cfg(2)).await;
    running_run(&mut rx).await;
    next(&mut rx, finished).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = load(dir.path(), chat_id);
    let run = chat.messages[1].tool_calls[0].subagent.as_deref().unwrap();
    assert!(run.background);
    assert_eq!(run.outcome, Some(RunOutcome::Cancelled));
    assert!(run.finished_at.is_some());
    assert!(
        !chat.messages.iter().any(|m| m.is_notification()),
        "nothing was announced on the way out"
    );
}

/// Taking back the exchange that started a run ends the run; it lands onto
/// the archived copy of its record and announces nothing.
#[tokio::test]
async fn taking_back_the_exchange_ends_its_run_in_the_archive() {
    let backend = KeyedRecorder::new(
        vec![
            ("", vec![start("c1"), text("started it")]),
            ("be harsh", vec![hang("thinking")]),
        ],
        10,
    );
    let (dir, cmd_tx, mut rx, handle, chat_id) = begin(backend.clone(), cfg(2)).await;
    running_run(&mut rx).await;
    next(&mut rx, finished).await;
    cmd_tx.send(AppCommand::DeleteLastExchange).unwrap();
    next(&mut rx, runs_out(0)).await;
    nothing_like(&mut rx, finished).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = load(dir.path(), chat_id);
    assert!(chat.messages.is_empty(), "{:?}", chat.messages);
    let run = chat.deleted[0]
        .messages
        .iter()
        .flat_map(|m| m.tool_calls.iter())
        .find_map(|r| r.subagent.as_deref())
        .expect("the run on the archived record");
    assert_eq!(run.outcome, Some(RunOutcome::Cancelled));
}

/// The tool is offered to the model only with the setting on; the catalog
/// and the profile know it either way (the `web_search` shape).
#[test]
fn start_subagent_is_gated_by_the_setting() {
    use crate::features::tools::{ToolGates, default_tool_ids, effective_tool_ids};
    let enabled = default_tool_ids();
    assert!(enabled.iter().any(|t| t == "start_subagent"));
    let gates = |background: bool| ToolGates {
        web: true,
        python: true,
        fs: true,
        mcp: true,
        background,
        history: false,
        workspace: false,
        workspace_commands: Default::default(),
        sampling_provider: None,
    };
    assert!(
        !effective_tool_ids(&enabled, &gates(false))
            .iter()
            .any(|t| t == "start_subagent")
    );
    assert!(
        effective_tool_ids(&enabled, &gates(true))
            .iter()
            .any(|t| t == "start_subagent")
    );
}

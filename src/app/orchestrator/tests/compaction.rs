//! Orchestrator tests — conversation history compression (`/compact`, spec §6.7).
//! Part of the [`super`] module (fixtures in mod.rs).
//! See docs/research/history-compression.md.
//!
//! Two shapes are used deliberately. The end-to-end tests go through the real
//! `run` loop (`spawn_orch_cfg`), because a roll is a background task whose
//! result comes back through an internal channel — the bare orchestrator has no
//! loop draining it. The ones about *applying* a finished roll drive
//! [`Orchestrator::handle_compact_result`] directly: the boundary races and the
//! "`messages` are never edited" invariant live there, and a direct call is the
//! only way to control the timing they are about.

use super::*;

use std::collections::VecDeque;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

use super::super::compaction::{CompactEnd, CompactOrigin};
use crate::features::compaction::summary_system_message;
use crate::shared::api::ChatRequest;
use crate::shared::api::contract::ChatStream;
use crate::shared::config::{CompactionSettings, DEFAULT_COMPACTION_SUMMARY_WORDS};
use crate::shared::i18n::{Lang, locale};

/// An engine that records every request it is given and replies with the
/// scripted texts in order (falling back to a fixed reply once they run out).
///
/// Both halves matter here: a roll's *result* is the summary, and what a roll
/// *sent* is the only thing that tells rolling a summary forward apart from
/// starting one over.
struct RecordingBackend {
    requests: Mutex<Vec<ChatRequest>>,
    replies: Mutex<VecDeque<String>>,
}

impl RecordingBackend {
    fn new(replies: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
            replies: Mutex::new(replies.iter().map(|s| (*s).to_string()).collect()),
        })
    }

    fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().unwrap().clone()
    }

    /// The requests that were summarization rolls, oldest first — told apart by
    /// the system prompt the compaction path builds (a chat turn carries the
    /// profile's own).
    fn rolls(&self) -> Vec<ChatRequest> {
        let sys = summary_system_message(locale(Lang::default()), DEFAULT_COMPACTION_SUMMARY_WORDS);
        self.requests()
            .into_iter()
            .filter(|r| r.system.as_deref() == Some(sys.as_str()))
            .collect()
    }
}

#[async_trait::async_trait]
impl EngineBackend for RecordingBackend {
    async fn chat_stream(
        &self,
        req: ChatRequest,
        _cancel: CancellationToken,
    ) -> anyhow::Result<ChatStream> {
        self.requests.lock().unwrap().push(req);
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| "ок".to_string());
        let s = async_stream::stream! {
            yield ChatChunk::Text(reply);
            yield ChatChunk::Finished(FinishReason::Stop);
        };
        Ok(Box::pin(s))
    }
}

/// Compaction on, with a tiny verbatim tail so a couple of exchanges already
/// qualify for a cut (the default 2048 tokens would need a long conversation).
/// Automatic titling off: this suite scripts its engines as ordered reply
/// lists, and the title request the first exchange would fire consumes an
/// entry out of turn (the trigger has its own tests in `tests/title.rs`).
fn compact_cfg(tail_tokens: usize) -> AppConfig {
    let mut cfg = no_auto_cfg();
    cfg.compaction = CompactionSettings {
        enabled: true,
        tail_tokens,
        ..Default::default()
    };
    cfg
}

/// What [`orch_with_history`] hands back: the data directory, the orchestrator,
/// its event stream, the active chat's id, and the engine it will talk to.
type HistoryFixture = (
    tempfile::TempDir,
    Orchestrator,
    UnboundedReceiver<AppEvent>,
    Uuid,
    Arc<RecordingBackend>,
);

/// A bare orchestrator with an active chat of `exchanges` user/assistant pairs
/// and a recording engine already wired in — enough history for a cut to exist.
fn orch_with_history(exchanges: usize) -> HistoryFixture {
    let (dir, mut orch, rx) = bare_orch_rx();
    let profile = Profile::new("P", "sys");
    let mut chat = Chat::from_profile(&profile, "t");
    for i in 0..exchanges {
        chat.push_message(Message::user(format!("вопрос {i}")));
        chat.push_message(Message::assistant(format!("ответ {i}")));
    }
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);
    // A ready engine, so a refusal can never be "no backend" by accident.
    let backend = RecordingBackend::new(&["сводка"]);
    orch.engines.backend = Some(backend.clone() as Arc<dyn EngineBackend>);
    (dir, orch, rx, chat_id, backend)
}

fn chat_of(orch: &Orchestrator, id: Uuid) -> &Chat {
    orch.chats.iter().find(|c| c.id == id).expect("the chat")
}

/// Everything the orchestrator has emitted so far.
fn drain(rx: &mut UnboundedReceiver<AppEvent>) -> Vec<AppEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

/// One turn through the real loop. Each turn adds two messages (user +
/// assistant) to the chat.
///
/// `Finished` alone is **not** enough to wait for: the generation task emits it
/// before posting its result, and the assistant message is appended later, by
/// `handle_done` on the loop's side. Waiting for the `ChatList` that
/// `handle_done` emits afterwards is what makes the next command see the whole
/// exchange (the `title.rs` precedent).
async fn turn(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
    text: &str,
) {
    cmd_tx.send(AppCommand::SendMessage(text.into())).unwrap();
    wait_for(evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    wait_for(evt_rx, |e| matches!(e, AppEvent::ChatList(_)))
        .await
        .unwrap();
}

/// Waits for a roll to land, failing loudly rather than hanging if it never
/// does. Returns `(chat_id, boundary, summary, folded)`.
async fn wait_compacted(rx: &mut UnboundedReceiver<AppEvent>) -> (Uuid, Uuid, String, usize) {
    let ev = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        wait_for(rx, |e| matches!(e, AppEvent::Compacted { .. })),
    )
    .await
    .expect("a Compacted event within 10s")
    .expect("a Compacted event");
    match ev {
        AppEvent::Compacted {
            chat_id,
            boundary,
            summary,
            folded,
        } => (chat_id, boundary, summary, folded),
        _ => unreachable!(),
    }
}

/// The single chat as it reached disk. Compaction is persisted with the save
/// debounce, so this runs after `Quit` has flushed.
fn saved_chat(dir: &tempfile::TempDir) -> Chat {
    Storage::open(Paths::with_root(dir.path()))
        .unwrap()
        .json()
        .load_chats()
        .unwrap()
        .into_iter()
        .next()
        .expect("one chat")
}

// ---------- refusals: the command is always answered ----------

/// Fork F10, the load-bearing one: off means **inert**, not "declines quietly".
/// A `Notice` rather than an `Error` — the user turned the feature off, nothing
/// went wrong — and no roll is even started.
#[tokio::test]
async fn the_master_switch_makes_compact_inert() {
    let (_d, mut orch, mut rx, chat_id, backend) = orch_with_history(3);
    orch.config.compaction = CompactionSettings {
        enabled: false,
        tail_tokens: 1, // a cut would exist if the switch let one be planned
        ..Default::default()
    };

    orch.handle_compact();

    let events = drain(&mut rx);
    assert!(
        events.iter().any(|e| matches!(e, AppEvent::Notice(_))),
        "the command must be answered: {events:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(e, AppEvent::Error(_))),
        "a switch that is off is not a failure: {events:?}"
    );
    assert!(chat_of(&orch, chat_id).compaction.is_none());
    assert!(!orch.bg_running(BackgroundKind::Compaction));
    assert!(
        backend.requests().is_empty(),
        "no roll may be started while the switch is off"
    );
}

/// The other refusal: the feature is on, but the whole conversation still fits
/// inside the verbatim tail. Also an answer, never silence.
#[tokio::test]
async fn a_conversation_that_is_still_short_is_answered() {
    let (_d, mut orch, mut rx, chat_id, backend) = orch_with_history(2);
    orch.config.compaction = CompactionSettings {
        enabled: true,
        // Larger than the whole conversation → `plan_cut` finds nothing to fold.
        tail_tokens: 100_000,
        ..Default::default()
    };

    orch.handle_compact();

    let events = drain(&mut rx);
    assert!(
        events.iter().any(|e| matches!(e, AppEvent::Notice(_))),
        "nothing to compact must still be reported: {events:?}"
    );
    assert!(chat_of(&orch, chat_id).compaction.is_none());
    assert!(!orch.bg_running(BackgroundKind::Compaction));
    assert!(
        backend.requests().is_empty(),
        "a refusal must not cost a generation"
    );
}

// ---------- a successful roll, end to end ----------

#[tokio::test]
async fn a_successful_roll_stores_the_summary_and_tells_the_feed() {
    const SUMMARY: &str = "Ранее: обсудили первый и второй вопрос.";
    let backend = RecordingBackend::new(&["ответ один", "ответ два", SUMMARY]);
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend.clone()), compact_cfg(1));
    let activated = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let active_id = match activated {
        AppEvent::ChatActivated { id, .. } => id,
        _ => unreachable!(),
    };

    turn(&cmd_tx, &mut evt_rx, "первый вопрос").await;
    turn(&cmd_tx, &mut evt_rx, "второй вопрос").await;

    cmd_tx.send(AppCommand::Compact).unwrap();
    let (event_chat, boundary, summary, folded) = wait_compacted(&mut evt_rx).await;
    assert_eq!(
        event_chat, active_id,
        "the event names the chat it is about"
    );
    assert_eq!(summary, SUMMARY, "the event carries what the model wrote");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = saved_chat(&dir);
    let c = chat.compaction.expect("a stored summary");
    assert_eq!(c.summary, SUMMARY);
    assert_eq!(c.rolls, 1, "the first compaction of this chat");
    assert_eq!(
        c.upto, folded,
        "the event reports the same span as is stored"
    );
    assert_eq!(c.boundary_id, boundary);
    assert_eq!(
        chat.messages[c.upto].id, c.boundary_id,
        "the stored index and the stored id must point at the same message"
    );
    assert_eq!(
        chat.messages[c.upto].role,
        MessageRole::User,
        "the cut always lands on a user message, so a request can never split \
         an assistant turn from its tool results"
    );
    // Two turns = four messages; folding some of them removed none.
    assert_eq!(chat.messages.len(), 4);
    assert!(c.upto > 0 && c.upto < chat.messages.len());
}

// ---------- stage 3: the read-back tools ----------

/// Waits until the engine has been sent at least `n` requests. The generation
/// task records its request as soon as it runs, so a few yields are enough —
/// and this is the only way to see it, since the bare orchestrator runs no loop
/// and never reaches `handle_done`.
async fn wait_for_requests(backend: &RecordingBackend, n: usize) -> Vec<ChatRequest> {
    for _ in 0..200 {
        let reqs = backend.requests();
        if reqs.len() >= n {
            return reqs;
        }
        tokio::task::yield_now().await;
    }
    panic!("the engine was never sent {n} request(s)");
}

fn offers_history_tools(req: &ChatRequest) -> bool {
    use crate::features::tools::history::{HISTORY_READ_ID, HISTORY_SEARCH_ID};
    req.tools
        .iter()
        .any(|t| t.name == HISTORY_READ_ID || t.name == HISTORY_SEARCH_ID)
}

/// S12 end to end: the two tools reach the model only once this chat actually
/// has a folded-away range — the same condition that puts the summary block in
/// the prompt, which is what lets the block name them without ever promising an
/// absent tool. Two schemas on every turn of every chat is exactly the cost this
/// feature's audience cannot afford.
// Spawns: a turn runs in its own task.
#[tokio::test]
async fn the_read_back_tools_are_offered_only_after_a_compaction() {
    let (_d, mut orch, _rx, chat_id, backend) = orch_with_history(4);
    orch.config = compact_cfg(1);
    orch.profiles[0].enabled_tools = crate::features::tools::default_tool_ids();

    orch.handle_send("первый вопрос".into());
    let reqs = wait_for_requests(&backend, 1).await;
    assert!(
        !offers_history_tools(&reqs[0]),
        "nothing folded yet — the tools must not be offered"
    );

    // Fold, through the real application path.
    let boundary_id = chat_of(&orch, chat_id).messages[2].id;
    // The bare orchestrator runs no loop, so nothing calls `handle_done` to put
    // the state back — do it by hand, or the second `handle_send` is a no-op.
    orch.gen_state = crate::app::gen_state::GenState::Idle;
    orch.handle_compact_result(CompactResult {
        origin: CompactOrigin::Manual,
        chat_id,
        boundary_id,
        rolls: 1,
        text: Ok("сводка".into()),
    });

    orch.handle_send("второй вопрос".into());
    let reqs = wait_for_requests(&backend, 2).await;
    let turn = reqs.last().unwrap();
    assert!(
        offers_history_tools(turn),
        "with a folded range the tools must be offered"
    );
    // The block that names them is in the same request, from the same condition.
    assert!(
        turn.system
            .as_deref()
            .unwrap_or_default()
            .contains("сводка"),
        "the summary block travels with the tools"
    );
}

/// The turn's snapshot has to carry the folded range itself, or the tools would
/// be offered and then answer "nothing is folded" — the dead end this project
/// keeps closing.
#[test]
fn the_turn_snapshot_carries_the_folded_range() {
    use crate::features::compaction::HistoryView;
    let (_d, mut orch, _rx, chat_id, _backend) = orch_with_history(4);
    orch.config = compact_cfg(1);
    let boundary_id = chat_of(&orch, chat_id).messages[2].id;
    orch.handle_compact_result(CompactResult {
        origin: CompactOrigin::Manual,
        chat_id,
        boundary_id,
        rolls: 1,
        text: Ok("сводка".into()),
    });

    let chat = chat_of(&orch, chat_id);
    let (_, upto) = chat.compaction_view(true).expect("a folded range");
    let view = HistoryView::render(&chat.messages[..upto], locale(Lang::default()))
        .expect("the folded range renders");
    // What the reader sees is the folded part and nothing after it.
    assert!(view.page(64, 1).is_some());
    let whole: String = (1..=view.page_count(64))
        .map(|p| view.page(64, p).unwrap())
        .collect();
    assert!(whole.contains("вопрос 0"), "{whole}");
    assert!(
        !whole.contains(&chat.messages[upto].text),
        "the verbatim tail is already in the prompt: {whole}"
    );
}

// ---------- the invariant the whole design rests on ----------

/// Compression only changes what a *request* carries. `messages` is compared by
/// id, not by count: a replacement of the same length would pass a count check.
#[tokio::test]
async fn compressing_never_edits_the_conversation() {
    let (_d, mut orch, _rx, chat_id, _backend) = orch_with_history(3);
    let before: Vec<Uuid> = chat_of(&orch, chat_id)
        .messages
        .iter()
        .map(|m| m.id)
        .collect();
    let boundary_id = chat_of(&orch, chat_id).messages[2].id;

    orch.handle_compact_result(CompactResult {
        origin: CompactOrigin::Manual,
        chat_id,
        boundary_id,
        rolls: 1,
        text: Ok("сводка".into()),
    });

    let chat = chat_of(&orch, chat_id);
    assert!(chat.compaction.is_some(), "the summary was applied");
    let after: Vec<Uuid> = chat.messages.iter().map(|m| m.id).collect();
    assert_eq!(
        before, after,
        "the feed, search, export and the reflection watermark all keep seeing \
         the whole conversation"
    );
}

// ---------- rolling forward, not restarting ----------

#[tokio::test]
async fn a_second_roll_rolls_the_summary_forward() {
    const FIRST: &str = "Ранее: обсудили погоду.";
    const SECOND: &str = "Ранее: обсудили погоду и встречу.";
    let backend =
        RecordingBackend::new(&["ответ 1", "ответ 2", FIRST, "ответ 3", "ответ 4", SECOND]);
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend.clone()), compact_cfg(1));
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    turn(&cmd_tx, &mut evt_rx, "какая погода").await;
    turn(&cmd_tx, &mut evt_rx, "какой прогноз").await;
    cmd_tx.send(AppCommand::Compact).unwrap();
    let (_, _, _, first_upto) = wait_compacted(&mut evt_rx).await;

    turn(&cmd_tx, &mut evt_rx, "во сколько встреча").await;
    turn(&cmd_tx, &mut evt_rx, "перенеси встречу").await;
    cmd_tx.send(AppCommand::Compact).unwrap();
    let (_, _, second_summary, second_upto) = wait_compacted(&mut evt_rx).await;

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    assert_eq!(second_summary, SECOND);
    assert!(
        second_upto > first_upto,
        "the boundary must move forward: {first_upto} → {second_upto}"
    );
    let chat = saved_chat(&dir);
    let c = chat.compaction.expect("a stored summary");
    assert_eq!(c.rolls, 2, "a roll, not a fresh first compaction");
    assert_eq!(
        c.summary, SECOND,
        "the newest summary replaces the previous"
    );

    // What actually distinguishes a roll from starting over: the request carries
    // the previous summary, and only the span the summary does not yet cover.
    let rolls = backend.rolls();
    assert_eq!(rolls.len(), 2, "one request per compaction");
    let second = &rolls[1].messages[0].content;
    assert!(
        second.contains(FIRST),
        "the second roll must carry the previous summary forward: {second}"
    );
    assert!(
        !second.contains("какая погода"),
        "what the first roll already folded must not be re-summarized: {second}"
    );
    assert!(
        second.contains("какой прогноз"),
        "the span since the previous boundary must be there: {second}"
    );
    assert!(
        !rolls[0].messages[0].content.contains(FIRST),
        "the first roll has no previous summary to roll forward"
    );
}

// ---------- races and failures ----------

/// The history can be edited (`Ctrl+E`/`Ctrl+R`) while a roll is in flight. The
/// boundary is re-found by id, so a summary that can no longer be placed is
/// dropped rather than pinned to whatever now sits at that index.
#[tokio::test]
async fn a_boundary_that_vanished_mid_roll_discards_the_summary() {
    let (_d, mut orch, mut rx, chat_id, _backend) = orch_with_history(3);
    orch.begin_bg(BackgroundKind::Compaction, CancellationToken::new(), None);
    let _ = drain(&mut rx);

    orch.handle_compact_result(CompactResult {
        origin: CompactOrigin::Manual,
        chat_id,
        boundary_id: Uuid::new_v4(), // never belonged to this chat
        rolls: 1,
        text: Ok("сводка".into()),
    });

    let chat = chat_of(&orch, chat_id);
    assert!(
        chat.compaction.is_none(),
        "a summary with nowhere to attach is discarded, not stored"
    );
    let events = drain(&mut rx);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AppEvent::Compacted { .. })),
        "nothing to tell the feed about: {events:?}"
    );
    assert!(!orch.bg_running(BackgroundKind::Compaction));
    assert_eq!(
        orch.bg_failures(BackgroundKind::Compaction),
        0,
        "the history moving under the roll is not a failure of the roll"
    );
}

/// Housekeeping must not reorder the chat list (the reflection precedent).
#[tokio::test]
async fn a_compaction_does_not_bump_modified_at() {
    let (_d, mut orch, _rx, chat_id, _backend) = orch_with_history(3);
    let before = chat_of(&orch, chat_id).modified_at;
    let boundary_id = chat_of(&orch, chat_id).messages[2].id;

    orch.handle_compact_result(CompactResult {
        origin: CompactOrigin::Manual,
        chat_id,
        boundary_id,
        rolls: 1,
        text: Ok("сводка".into()),
    });

    let chat = chat_of(&orch, chat_id);
    assert!(chat.compaction.is_some(), "the summary was applied");
    assert_eq!(
        chat.modified_at, before,
        "compressing is housekeeping — it must not bump the chat up the list"
    );
}

#[tokio::test]
async fn a_failed_roll_is_reported_and_clears_the_indicator() {
    let (_d, mut orch, mut rx, chat_id, _backend) = orch_with_history(3);
    orch.begin_bg(BackgroundKind::Compaction, CancellationToken::new(), None);
    let _ = drain(&mut rx);

    orch.handle_compact_result(CompactResult {
        origin: CompactOrigin::Manual,
        chat_id,
        boundary_id: chat_of(&orch, chat_id).messages[2].id,
        rolls: 1,
        text: Err(CompactEnd::Failed("сервер недоступен".into())),
    });

    let events = drain(&mut rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AppEvent::Error(m) if m.contains("сервер недоступен"))),
        "the reason reaches the user: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            AppEvent::BackgroundTask {
                kind: BackgroundKind::Compaction,
                active: false
            }
        )),
        "the status-bar indicator is cleared: {events:?}"
    );
    assert!(!orch.bg_running(BackgroundKind::Compaction));
    assert!(chat_of(&orch, chat_id).compaction.is_none());
    // The user typed the command and was told directly, so the failure streak is
    // deliberately not advanced — the streak exists for *silent* runs, and a
    // second alert for a command just typed would be noise.
    assert_eq!(orch.bg_failures(BackgroundKind::Compaction), 0);
}

// ---------- stage 2: the automatic trigger ----------
//
// These drive `maybe_auto_compact` directly on a bare orchestrator. Every gate
// it applies is synchronous, and "did a roll start?" is exactly
// `bg_running(Compaction)` — which is also what makes a *negative* assertion
// meaningful here: through the loop, "no `Compacted` event yet" is
// indistinguishable from "the roll is still running".

use super::super::generation::TurnUsage;
use crate::shared::config::{EngineSettings, ManagedSettings, ServerMode};

/// A managed engine with a deliberately tiny window, so a modest `usage` is
/// already over the threshold. Managed is also the one budget source that needs
/// no network (S1), which keeps these tests off the discovery path.
fn auto_cfg(context_size: u32, threshold_pct: u8) -> AppConfig {
    AppConfig {
        compaction: CompactionSettings {
            enabled: true,
            tail_tokens: 1,
            threshold_pct,
            ..Default::default()
        },
        engine: EngineSettings {
            mode: ServerMode::Managed,
            managed: ManagedSettings {
                context_size,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

fn usage(prompt: u32, completion: u64) -> Option<TurnUsage> {
    Some(TurnUsage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        prefill: None,
    })
}

// Spawns: reaching a roll (or asking the engine for its window) starts a task.
#[tokio::test]
async fn auto_compaction_fires_once_a_turn_crosses_the_threshold() {
    let (_d, mut orch, _rx, chat_id, _backend) = orch_with_history(3);
    orch.config = auto_cfg(1000, 75);
    // 700 + 100 = 800 of a 1000-token window: past the 750 mark.
    orch.maybe_auto_compact(chat_id, usage(700, 100));
    assert!(
        orch.bg_running(BackgroundKind::Compaction),
        "a roll must be under way"
    );
}

/// The reply counts towards the next turn's prompt: on its own the prompt is
/// still under the mark, and ignoring what was generated on top of it would
/// postpone the compaction by exactly the turn that overflows.
// Spawns: reaching a roll (or asking the engine for its window) starts a task.
#[tokio::test]
async fn the_reply_counts_towards_the_next_prompt() {
    let (_d, mut orch, _rx, chat_id, _backend) = orch_with_history(3);
    orch.config = auto_cfg(1000, 75);
    orch.maybe_auto_compact(chat_id, usage(700, 0));
    assert!(!orch.bg_running(BackgroundKind::Compaction), "700 < 750");
    orch.maybe_auto_compact(chat_id, usage(700, 60));
    assert!(orch.bg_running(BackgroundKind::Compaction), "760 >= 750");
}

/// S2: without an exact `usage` the trigger stays quiet rather than falling back
/// to the byte estimate, whose error changes sign by content type (§9a M9) and
/// is worst on exactly the tool-heavy chats that overflow first.
#[test]
fn without_exact_usage_the_trigger_stays_quiet() {
    let (_d, mut orch, _rx, chat_id, _backend) = orch_with_history(3);
    orch.config = auto_cfg(10, 75); // any conversation is over this window
    orch.maybe_auto_compact(chat_id, None);
    assert!(!orch.bg_running(BackgroundKind::Compaction));
}

/// Fork F10 again, on the new path: off means inert, and a threshold of 0 means
/// "manual only" — both leave `/compact` working.
#[test]
fn the_switch_and_a_zero_threshold_both_disable_the_auto_path() {
    for (enabled, pct) in [(false, 75), (true, 0)] {
        let (_d, mut orch, _rx, chat_id, _backend) = orch_with_history(3);
        let mut cfg = auto_cfg(1000, pct);
        cfg.compaction.enabled = enabled;
        orch.config = cfg;
        orch.maybe_auto_compact(chat_id, usage(900, 50));
        assert!(
            !orch.bg_running(BackgroundKind::Compaction),
            "enabled={enabled} pct={pct}"
        );
    }
}

/// One at a time: a roll already in flight is moving the boundary anyway.
#[test]
fn a_roll_already_running_is_not_started_twice() {
    let (_d, mut orch, _rx, chat_id, backend) = orch_with_history(3);
    orch.config = auto_cfg(1000, 75);
    orch.begin_bg(BackgroundKind::Compaction, CancellationToken::new(), None);
    let before = backend.requests().len();
    orch.maybe_auto_compact(chat_id, usage(900, 50));
    assert_eq!(backend.requests().len(), before, "no second roll was sent");
}

/// A conversation with nothing left to fold is over the threshold on every
/// single turn. It must not spin: no roll, and — the part that would be visible
/// — no message.
#[test]
fn nothing_left_to_fold_is_silent() {
    let (_d, mut orch, mut rx, chat_id, _backend) = orch_with_history(1);
    orch.config = auto_cfg(10, 75);
    let _ = drain(&mut rx);
    orch.maybe_auto_compact(chat_id, usage(900, 50));
    assert!(!orch.bg_running(BackgroundKind::Compaction));
    assert!(
        drain(&mut rx).is_empty(),
        "an unavoidable state must not nag every turn"
    );
}

// ---------- stage 2: resolving the budget ----------

#[test]
fn an_explicit_setting_outranks_every_other_source() {
    let (_d, mut orch, _rx, _chat_id, _backend) = orch_with_history(1);
    orch.config = auto_cfg(1000, 75);
    orch.config.compaction.context_tokens = Some(4096);
    assert_eq!(orch.context_budget(), Some(4096));
    // …and it applies where there is nothing to discover, which is the case it
    // exists for (a cloud model, or a server that does not report its window).
    orch.config.engine.mode = ServerMode::OpenAi;
    assert_eq!(orch.context_budget(), Some(4096));
}

#[test]
fn a_managed_server_is_measured_against_its_own_c_flag() {
    let (_d, mut orch, _rx, _chat_id, _backend) = orch_with_history(1);
    orch.config = auto_cfg(3072, 75);
    assert_eq!(orch.context_budget(), Some(3072));
}

/// With no source at all the answer is `None` — "cannot say", never a guess.
/// `/compact` needs no budget, so the user is not stuck either way.
#[test]
fn an_engine_that_cannot_say_leaves_the_budget_unknown() {
    let (_d, mut orch, _rx, chat_id, _backend) = orch_with_history(3);
    orch.config = auto_cfg(1000, 75);
    orch.config.engine.mode = ServerMode::External;
    // The recording backend keeps the trait's default answer.
    let epoch = orch.context.epoch();
    orch.handle_budget_result(epoch, None);
    assert_eq!(orch.context_budget(), None);
    orch.maybe_auto_compact(chat_id, usage(900, 50));
    assert!(!orch.bg_running(BackgroundKind::Compaction));
}

// Spawns: reaching a roll (or asking the engine for its window) starts a task.
#[tokio::test]
async fn a_discovered_window_is_used_and_can_be_re_asked() {
    let (_d, mut orch, _rx, _chat_id, _backend) = orch_with_history(1);
    orch.config = auto_cfg(1000, 75);
    orch.config.engine.mode = ServerMode::External;
    let epoch = orch.context.epoch();
    orch.handle_budget_result(epoch, Some(16384));
    assert_eq!(orch.context_budget(), Some(16384));
    // A readiness flip or an engine change forgets it: the next engine may have
    // a different window, and a server that could not answer before may now.
    orch.context.invalidate();
    assert_eq!(orch.context_budget(), None);
}

/// The epoch is what makes switching engines mid-question safe: an answer about
/// the previous engine must not become the new one's budget.
// Spawns: reaching a roll (or asking the engine for its window) starts a task.
#[tokio::test]
async fn an_answer_about_a_replaced_engine_is_dropped() {
    let (_d, mut orch, _rx, _chat_id, _backend) = orch_with_history(1);
    orch.config = auto_cfg(1000, 75);
    orch.config.engine.mode = ServerMode::External;
    let stale = orch.context.epoch();
    orch.context.invalidate();
    orch.handle_budget_result(stale, Some(131072));
    assert_eq!(
        orch.context_budget(),
        None,
        "the late answer belonged to an engine that is gone"
    );
}

// ---------- stage 2: how a failure is reported, by origin ----------

/// S6: a silent background roll spends a strike, so three consecutive failures
/// alert once — and it does *not* report each one, which is the difference from
/// the manual path.
#[test]
fn an_automatic_failure_advances_the_streak_without_reporting() {
    let (_d, mut orch, mut rx, chat_id, _backend) = orch_with_history(3);
    let _ = drain(&mut rx);
    let boundary_id = chat_of(&orch, chat_id).messages[2].id;
    orch.handle_compact_result(CompactResult {
        chat_id,
        boundary_id,
        rolls: 1,
        origin: CompactOrigin::Auto,
        text: Err(CompactEnd::Failed("сервер недоступен".into())),
    });
    assert_eq!(orch.bg_failures(BackgroundKind::Compaction), 1);
    let events = drain(&mut rx);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AppEvent::Error(m) if m.contains("сервер недоступен"))),
        "a background failure is not announced on its own: {events:?}"
    );
}

/// An empty summary is a failure too — and on the automatic path it must not be
/// announced either, or a model that keeps returning nothing would produce a
/// message a turn.
#[test]
fn an_empty_automatic_summary_is_counted_not_announced() {
    let (_d, mut orch, mut rx, chat_id, _backend) = orch_with_history(3);
    let _ = drain(&mut rx);
    let boundary_id = chat_of(&orch, chat_id).messages[2].id;
    orch.handle_compact_result(CompactResult {
        chat_id,
        boundary_id,
        rolls: 1,
        origin: CompactOrigin::Auto,
        text: Ok("   ".into()),
    });
    assert_eq!(orch.bg_failures(BackgroundKind::Compaction), 1);
    assert!(
        !drain(&mut rx)
            .iter()
            .any(|e| matches!(e, AppEvent::Error(_))),
        "no error is shown for a silent run"
    );
    assert!(chat_of(&orch, chat_id).compaction.is_none());
}

// ---------- stage 2: what the user is told when the window is already full ----------

/// An engine that always fails with the body llama-server really sends when a
/// prompt no longer fits (§9a M3 — HTTP 400 before the stream starts).
struct OverflowingBackend;

const OVERFLOW_BODY: &str = "engine returned status 400 Bad Request: \
{\"error\":{\"code\":400,\"message\":\"the request exceeds the available context size\",\
\"type\":\"exceed_context_size_error\",\"n_prompt_tokens\":32706,\"n_ctx\":16384}}";

#[async_trait::async_trait]
impl EngineBackend for OverflowingBackend {
    async fn chat_stream(
        &self,
        _req: ChatRequest,
        _cancel: CancellationToken,
    ) -> anyhow::Result<ChatStream> {
        anyhow::bail!("{OVERFLOW_BODY}")
    }
}

/// Drives one failing turn and returns what the user was told.
async fn overflow_message(compaction_enabled: bool) -> String {
    let mut cfg = compact_cfg(1);
    cfg.compaction.enabled = compaction_enabled;
    let backend: Arc<dyn EngineBackend> = Arc::new(OverflowingBackend);
    let (_dir, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend), cfg);
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    cmd_tx
        .send(AppCommand::SendMessage("вопрос".into()))
        .unwrap();
    let ev = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Error(_)))
        .await
        .unwrap();
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    match ev {
        AppEvent::Error(m) => m,
        _ => unreachable!(),
    }
}

/// S4. The message must say what to do — and *which* thing to do depends on the
/// switch, because naming `/compact` while compression is off would send the
/// user to a command that refuses. That dead end is the defect class this
/// project has closed three times.
#[tokio::test]
async fn a_full_window_is_explained_and_never_points_at_a_dead_end() {
    let on = overflow_message(true).await;
    assert!(
        on.contains("/compact"),
        "with compression on, name the command that fixes it: {on}"
    );

    let off = overflow_message(false).await;
    assert!(
        !off.contains("/compact"),
        "with compression off, /compact would refuse — do not send the user there: {off}"
    );
    assert_ne!(on, off, "the two situations need different advice");

    // Whatever the advice, the server's own words survive: the client's rule of
    // never swallowing an error body is what made this diagnosable in the first
    // place.
    for msg in [&on, &off] {
        assert!(
            msg.contains("n_ctx") && msg.contains("16384"),
            "the raw reason is still there: {msg}"
        );
    }
}

/// An ordinary failure keeps the ordinary message: the hint must not attach
/// itself to every error that happens to mention a number.
#[tokio::test]
async fn an_unrelated_failure_is_not_dressed_up_as_an_overflow() {
    struct Broken;
    #[async_trait::async_trait]
    impl EngineBackend for Broken {
        async fn chat_stream(
            &self,
            _req: ChatRequest,
            _cancel: CancellationToken,
        ) -> anyhow::Result<ChatStream> {
            anyhow::bail!("connection refused (os error 10061)")
        }
    }
    let backend: Arc<dyn EngineBackend> = Arc::new(Broken);
    let (_dir, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend), compact_cfg(1));
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    cmd_tx
        .send(AppCommand::SendMessage("вопрос".into()))
        .unwrap();
    let ev = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Error(_)))
        .await
        .unwrap();
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    match ev {
        AppEvent::Error(m) => {
            assert!(!m.contains("/compact"), "no compaction advice here: {m}");
            assert!(m.contains("connection refused"), "{m}");
        }
        _ => unreachable!(),
    }
}

/// Impersonation (`Ctrl+U`) sends the conversation too, so it must send the
/// **compacted** view of it — otherwise it keeps hitting the very ceiling this
/// track removes, from a different key (spec §11.8).
///
/// Lives here rather than next to the other impersonation tests because driving
/// a real roll needs this module's fixtures; the request-shaping half is unit
/// tested in `tests/impersonation.rs`. What only this test can catch is the
/// wiring — `handle_impersonate` actually passing the view.
#[tokio::test]
async fn impersonation_sends_the_compacted_view() {
    const SUMMARY: &str = "Ранее: обсудили первый и второй вопрос.";
    let backend = RecordingBackend::new(&["ответ один", "ответ два", SUMMARY, "моя реплика"]);
    let (_dir, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend.clone()), compact_cfg(1));
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    turn(&cmd_tx, &mut evt_rx, "первый вопрос").await;
    turn(&cmd_tx, &mut evt_rx, "второй вопрос").await;
    cmd_tx.send(AppCommand::Compact).unwrap();
    let (_, _, _, folded) = wait_compacted(&mut evt_rx).await;
    assert!(folded > 0, "something was actually folded");

    cmd_tx
        .send(AppCommand::Impersonate {
            seed: String::new(),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::ImpersonationFinished { .. })
    })
    .await
    .unwrap();
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // The impersonation request is the last one, and the only one with no tools
    // whose system prompt is not a summarization roll.
    let last = backend.requests().pop().expect("an impersonation request");
    let system = last.system.as_deref().unwrap_or_default();
    assert!(
        system.contains(SUMMARY),
        "the summary must reach the impersonation prompt: {system}"
    );
    let sent: Vec<&str> = last.messages.iter().map(|m| m.content.as_str()).collect();
    assert!(
        !sent.iter().any(|t| t.contains("первый вопрос")),
        "the folded exchange must not be sent verbatim: {sent:?}"
    );
}

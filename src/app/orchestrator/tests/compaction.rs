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
fn compact_cfg(tail_tokens: usize) -> AppConfig {
    AppConfig {
        compaction: CompactionSettings {
            enabled: true,
            tail_tokens,
            ..Default::default()
        },
        ..Default::default()
    }
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
    orch.begin_bg(BackgroundKind::Compaction, CancellationToken::new());
    let _ = drain(&mut rx);

    orch.handle_compact_result(CompactResult {
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
    orch.begin_bg(BackgroundKind::Compaction, CancellationToken::new());
    let _ = drain(&mut rx);

    orch.handle_compact_result(CompactResult {
        chat_id,
        boundary_id: chat_of(&orch, chat_id).messages[2].id,
        rolls: 1,
        text: Err("сервер недоступен".into()),
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

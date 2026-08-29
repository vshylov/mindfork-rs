//! Orchestrator tests — the sub-agent as a nested turn (spec §9.3.2,
//! docs/research/subagent-chats.md §3.2–§3.5): a `call_subagent` call runs a
//! child loop with the turn's tools, and the run lands on the call's record.
//! Part of the [`super`] module (fixtures in mod.rs).

use std::collections::VecDeque;
use std::sync::Mutex;

use super::*;
use crate::entities::subagent::RunOutcome;
use crate::shared::api::ChatRequest;
use crate::shared::api::contract::{ChatStream, ToolCallDelta};

/// One scripted engine round; `hang` keeps the stream open after the chunks
/// until the request's token is cancelled (a slow sub-agent).
pub(super) struct Script {
    pub(super) chunks: Vec<ChatChunk>,
    pub(super) hang: bool,
}

/// An engine that records every request it is given and plays the scripts in
/// order, one per `chat_stream` call — the parent's rounds and the child's
/// interleave on one engine exactly as they do in the application.
pub(super) struct ScriptRecorder {
    requests: Mutex<Vec<ChatRequest>>,
    scripts: Mutex<VecDeque<Script>>,
}

impl ScriptRecorder {
    pub(super) fn new(scripts: Vec<Script>) -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
            scripts: Mutex::new(scripts.into()),
        })
    }
    pub(super) fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl EngineBackend for ScriptRecorder {
    async fn chat_stream(
        &self,
        req: ChatRequest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<ChatStream> {
        self.requests.lock().unwrap().push(req);
        let script = self.scripts.lock().unwrap().pop_front().unwrap_or(Script {
            chunks: vec![ChatChunk::Finished(FinishReason::Stop)],
            hang: false,
        });
        let s = async_stream::stream! {
            for chunk in script.chunks {
                yield chunk;
            }
            if script.hang {
                cancel.cancelled().await;
                yield ChatChunk::Finished(FinishReason::Cancelled);
            }
        };
        Ok(Box::pin(s))
    }
}

pub(super) fn call(id: &str, name: &str, args: &str) -> Script {
    Script {
        chunks: vec![
            ChatChunk::ToolCall(ToolCallDelta {
                thought_signature: None,
                index: 0,
                id: Some(id.into()),
                name: Some(name.into()),
                arguments: args.into(),
            }),
            ChatChunk::Finished(FinishReason::ToolCalls),
        ],
        hang: false,
    }
}

pub(super) fn text(t: &str) -> Script {
    Script {
        chunks: vec![
            ChatChunk::Text(t.into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
        hang: false,
    }
}

pub(super) fn hang(prefix: &str) -> Script {
    Script {
        chunks: vec![ChatChunk::Text(prefix.into())],
        hang: true,
    }
}

const DELEGATE: &str =
    r#"{"name":"Critic","system_message":"be harsh","message":"what time is it"}"#;

/// Runs one parent turn over `scripts`; returns the root, the recorder, every
/// event the UI saw, and the chat id.
pub(super) async fn run_turn(
    scripts: Vec<Script>,
    cfg: AppConfig,
) -> (tempfile::TempDir, Arc<ScriptRecorder>, Vec<AppEvent>, Uuid) {
    let backend = ScriptRecorder::new(scripts);
    let (dir, cmd_tx, mut evt_rx, handle) =
        spawn_orch_cfg(Some(backend.clone() as Arc<dyn EngineBackend>), cfg);
    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let AppEvent::ChatActivated { id: chat_id, .. } = active else {
        unreachable!()
    };
    cmd_tx
        .send(AppCommand::SendMessage("delegate this".into()))
        .unwrap();
    let mut events = Vec::new();
    while let Some(ev) = evt_rx.recv().await {
        let done = matches!(ev, AppEvent::Finished { .. });
        events.push(ev);
        if done {
            break;
        }
    }
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    (dir, backend, events, chat_id)
}

pub(super) fn load(root: &std::path::Path, id: Uuid) -> Chat {
    Storage::open(Paths::with_root(root))
        .unwrap()
        .json()
        .load_chat(id)
        .unwrap()
        .unwrap()
}

fn tool_names(req: &ChatRequest) -> Vec<String> {
    req.tools.iter().map(|t| t.name.to_string()).collect()
}

#[tokio::test]
async fn subagent_runs_with_tools_and_lands_on_the_record() {
    // Parent: delegate → child: a tool call, then an answer → parent: the reply.
    let (dir, backend, events, chat_id) = run_turn(
        vec![
            call("c1", "call_subagent", DELEGATE),
            call("c2", "current_time", "{}"),
            text("it is noon"),
            text("done: noon"),
        ],
        no_auto_cfg(),
    )
    .await;

    // The parent's history: user → assistant(call) → tool → assistant(final).
    let chat = load(dir.path(), chat_id);
    assert_eq!(chat.messages.len(), 4, "{:?}", chat.messages);
    let record = &chat.messages[1].tool_calls[0];
    assert_eq!(record.name, "call_subagent");
    let run = record
        .subagent
        .as_deref()
        .expect("the run is on the record");
    assert_eq!(run.name.as_deref(), Some("Critic"));
    assert_eq!(run.title, "Critic");
    assert_eq!(run.system_message, "be harsh");
    assert_eq!(run.outcome, Some(RunOutcome::Completed));
    assert!(run.finished_at.is_some());
    // The run's own transcript: user → assistant(tool) → tool → assistant(answer).
    assert_eq!(run.messages.len(), 4, "{:?}", run.messages);
    assert_eq!(run.messages[0].role, MessageRole::User);
    assert_eq!(run.messages[0].text, "what time is it");
    assert_eq!(run.messages[1].tool_calls[0].name, "current_time");
    assert!(run.messages[1].tool_calls[0].subagent.is_none());
    assert_eq!(run.messages[2].role, MessageRole::Tool);
    assert_eq!(run.final_reply(), Some("it is noon"));
    assert!(run.tokens > 0, "the run's cost is recorded");
    // The parent's model got the answer and the transcript's address.
    let result = chat.messages[2].text.as_str();
    assert!(result.starts_with("it is noon"), "{result}");
    assert!(
        result.contains(&crate::features::chat_links::uri(run.id)),
        "{result}"
    );
    assert_eq!(chat.messages[3].text, "done: noon");

    // What the child was actually sent: its persona, its one message, the
    // turn's tools minus the withheld ones — and no nesting.
    let reqs = backend.requests();
    assert_eq!(reqs.len(), 4, "parent, child, child, parent");
    let child = &reqs[1];
    assert_eq!(child.system.as_deref(), Some("be harsh"));
    assert_eq!(child.messages.len(), 1);
    assert_eq!(child.messages[0].content, "what time is it");
    let names = tool_names(child);
    assert!(names.contains(&"current_time".to_string()), "{names:?}");
    assert!(!names.contains(&"call_subagent".to_string()), "{names:?}");
    assert!(!names.contains(&"history_read".to_string()), "{names:?}");
    assert!(!names.contains(&"get_self_model".to_string()), "{names:?}");
    // The parent, by contrast, still offers the tool.
    assert!(tool_names(&reqs[0]).contains(&"call_subagent".to_string()));
    // The child's second round carries its tool result back to it.
    assert_eq!(reqs[2].messages.len(), 3);

    // The feed saw one card — the parent's — and none of the child's stream.
    let cards: Vec<&AppEvent> = events
        .iter()
        .filter(|e| matches!(e, AppEvent::ToolCall { .. }))
        .collect();
    assert_eq!(cards.len(), 1);
    assert!(
        matches!(cards[0], AppEvent::ToolCall { name, call_id, .. } if name == "call_subagent" && call_id == "c1")
    );
    // …and the card opened before the run, under the same call id — the
    // child's own `current_time` opened nothing in the parent's feed.
    let started: Vec<(String, String)> = events
        .iter()
        .filter_map(|e| match e {
            AppEvent::ToolCallStarted { call_id, name, .. } => {
                Some((call_id.clone(), name.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        started,
        vec![("c1".to_string(), "call_subagent".to_string())]
    );
    let first_started = events
        .iter()
        .position(|e| matches!(e, AppEvent::ToolCallStarted { .. }))
        .unwrap();
    let first_card = events
        .iter()
        .position(|e| matches!(e, AppEvent::ToolCall { .. }))
        .unwrap();
    assert!(first_started < first_card);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AppEvent::Chunk { text, .. } if text == "it is noon")),
        "the child's text must not reach the parent's bubble"
    );
    // The counter counted the child's tokens on top of the parent's: one text
    // chunk in the child, one in the parent.
    let max_completion = events
        .iter()
        .filter_map(|e| match e {
            AppEvent::TokenUsage { completion, .. } => Some(*completion),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    assert_eq!(max_completion, 2);

    // The status-bar chip followed the run (spec §9.3.2): round 1, then the
    // tool it entered, then round 2, then cleared — and the parent's own
    // rounds reported nothing.
    let chip: Vec<Option<(String, u32, Option<String>)>> = events
        .iter()
        .filter_map(|e| match e {
            AppEvent::SubagentProgress { progress, .. } => Some(
                progress
                    .as_ref()
                    .map(|p| (p.name.clone(), p.round, p.tool.clone())),
            ),
            _ => None,
        })
        .collect();
    let critic = |round: u32, tool: Option<&str>| {
        Some(("Critic".to_string(), round, tool.map(str::to_string)))
    };
    assert_eq!(
        chip,
        vec![
            critic(1, None),
            critic(1, Some("current_time")),
            critic(2, None),
            None
        ]
    );
}

/// A landed transcript is titled at landing under either trigger point
/// (research §3.10): the run's whole exchange arrives at once, so the
/// "after the user's message" and "after the reply" moments coincide. The
/// parent is on its second exchange, so the parent's own trigger stays quiet
/// and the one title request on the engine is the transcript's.
#[tokio::test]
async fn a_landed_transcript_is_auto_titled_under_both_modes() {
    for mode in [
        crate::shared::config::AutoTitleMode::AfterUserMessage,
        crate::shared::config::AutoTitleMode::AfterAssistantReply,
    ] {
        let (dir, chat_id, _) = delegated().await;
        let mut cfg = no_auto_cfg();
        cfg.interface.auto_title = mode;
        // A second delegation in the existing chat: the parent's first-reply
        // trigger is spent; the title script answers the transcript's request.
        let backend = ScriptRecorder::new(vec![
            call("c2", "call_subagent", DELEGATE),
            text("it is one"),
            text("done: one"),
            text("Second opinion"),
        ]);
        let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(
            dir.path(),
            Some(backend.clone() as Arc<dyn EngineBackend>),
            cfg,
        );
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
            .await
            .unwrap();
        cmd_tx
            .send(AppCommand::SendMessage("delegate again".into()))
            .unwrap();
        let renamed = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatRenamed { .. }))
            .await
            .unwrap_or_else(|| panic!("no title landed under {mode:?}"));
        cmd_tx.send(AppCommand::Quit).unwrap();
        handle.await.unwrap();

        let chat = load(dir.path(), chat_id);
        let new_run = chat.children().last().unwrap();
        assert!(
            matches!(&renamed, AppEvent::ChatRenamed { id, title } if *id == new_run.id && title == "Second opinion"),
            "{renamed:?} under {mode:?}"
        );
        assert_eq!(new_run.title, "Second opinion");
        assert!(
            !new_run.renamed_manually,
            "an automatic title is not a manual one"
        );
        assert_eq!(
            chat.title, "Новый чат",
            "the parent was not retitled: {mode:?}"
        );
        // The title request was the transcript's digest, not the parent's.
        let title_req = backend.requests().last().unwrap().clone();
        assert!(
            title_req.messages[0].content.contains("it is one"),
            "{:?}",
            title_req.messages[0].content
        );
    }
}

#[tokio::test]
async fn subagent_cannot_nest() {
    // The child asks for a sub-agent of its own: refused as a disabled tool —
    // the name is not in its set, and the loop would refuse below the top anyway.
    let (dir, _backend, _events, chat_id) = run_turn(
        vec![
            call("c1", "call_subagent", DELEGATE),
            call("c2", "call_subagent", DELEGATE),
            text("gave up"),
            text("ok"),
        ],
        no_auto_cfg(),
    )
    .await;
    let chat = load(dir.path(), chat_id);
    let run = chat.messages[1].tool_calls[0].subagent.as_deref().unwrap();
    assert!(run.messages[1].tool_calls[0].subagent.is_none());
    let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::default());
    assert_eq!(
        run.messages[2].text,
        loc.tf("loop.tool_disabled", &[("name", "call_subagent")])
    );
    assert_eq!(run.outcome, Some(RunOutcome::Completed));
}

#[tokio::test]
async fn subagent_identity_effects_stay_on_the_run() {
    // `set_system_message` inside the run changes the run's persona, not the
    // parent chat's (research §3.4).
    let (dir, _backend, _events, chat_id) = run_turn(
        vec![
            call("c1", "call_subagent", DELEGATE),
            call(
                "c2",
                "set_system_message",
                r#"{"system_message":"new persona"}"#,
            ),
            text("changed"),
            text("ok"),
        ],
        no_auto_cfg(),
    )
    .await;
    let chat = load(dir.path(), chat_id);
    let run = chat.messages[1].tool_calls[0].subagent.as_deref().unwrap();
    assert_eq!(run.system_message, "new persona");
    assert_ne!(chat.system_message, "new persona");
}

#[tokio::test]
async fn subagent_timeout_lands_a_partial_run_and_tells_the_parent() {
    let mut cfg = no_auto_cfg();
    cfg.tools.subagent_run_timeout_secs = 1;
    let (dir, backend, _events, chat_id) = run_turn(
        vec![
            call("c1", "call_subagent", DELEGATE),
            hang("thinking…"),
            text("it timed out, so:"),
        ],
        cfg,
    )
    .await;
    let chat = load(dir.path(), chat_id);
    let run = chat.messages[1].tool_calls[0].subagent.as_deref().unwrap();
    assert_eq!(run.outcome, Some(RunOutcome::TimedOut));
    // The hung round never finished, so only the instruction is in the transcript.
    assert_eq!(run.messages.len(), 1);
    let result = &chat.messages[2].text;
    assert!(result.contains("chat://"), "{result}");
    assert!(result.contains("1 s") || result.contains("1 с"), "{result}");
    // The parent's turn went on to its reply.
    assert_eq!(chat.messages[3].text, "it timed out, so:");
    assert_eq!(backend.requests().len(), 3);
}

#[tokio::test]
async fn subagent_cancel_lands_the_run_as_cancelled() {
    let backend = ScriptRecorder::new(vec![
        call("c1", "call_subagent", DELEGATE),
        hang("thinking…"),
    ]);
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(
        Some(backend.clone() as Arc<dyn EngineBackend>),
        no_auto_cfg(),
    );
    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let AppEvent::ChatActivated { id: chat_id, .. } = active else {
        unreachable!()
    };
    cmd_tx
        .send(AppCommand::SendMessage("delegate this".into()))
        .unwrap();
    // Wait for the child's request to be on the wire, then press Esc.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while backend.requests().len() < 2 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the child never started"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    cmd_tx.send(AppCommand::Cancel).unwrap();
    let finished = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    assert!(matches!(
        finished,
        AppEvent::Finished {
            reason: FinishReason::Cancelled,
            ..
        }
    ));
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = load(dir.path(), chat_id);
    // The turn landed: the call's record carries the cancelled run.
    let run = chat.messages[1].tool_calls[0].subagent.as_deref().unwrap();
    assert_eq!(run.outcome, Some(RunOutcome::Cancelled));
    assert!(chat.messages[2].text.contains("chat://"));
}

#[tokio::test]
async fn subagent_with_an_empty_message_is_a_tool_error_without_a_run() {
    let (dir, backend, _events, chat_id) = run_turn(
        vec![
            call(
                "c1",
                "call_subagent",
                r#"{"system_message":"x","message":" "}"#,
            ),
            text("sorry"),
        ],
        no_auto_cfg(),
    )
    .await;
    let chat = load(dir.path(), chat_id);
    assert!(chat.messages[1].tool_calls[0].subagent.is_none());
    let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::default());
    assert!(
        chat.messages[2]
            .text
            .contains(loc.t("tool.call_subagent.err.message_empty")),
        "{}",
        chat.messages[2].text
    );
    // No child round was ever sent.
    assert_eq!(backend.requests().len(), 2);
}

// ---- the transcript while it runs (PR 7, docs/subagent-live.md) ----

/// Starts a delegation whose child does one tool round and then hangs, and
/// waits until the list shows the transcript **running with that round
/// filed** — 2 visible messages: the instruction plus the round's reply, its
/// tool row folded in (`visible_message_count`, spec §11.2). Returns
/// everything the caller needs to go on.
async fn running_delegation() -> (
    tempfile::TempDir,
    Arc<ScriptRecorder>,
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
    Uuid,
    Uuid,
    Uuid,
) {
    let backend = ScriptRecorder::new(vec![
        call("c1", "call_subagent", DELEGATE),
        call("c2", "current_time", "{}"),
        hang("thinking…"),
    ]);
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(
        Some(backend.clone() as Arc<dyn EngineBackend>),
        no_auto_cfg(),
    );
    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let AppEvent::ChatActivated { id: chat_id, .. } = active else {
        unreachable!()
    };
    // A second chat to switch *away* to (`Ctrl+N` itself never cancels a
    // turn — it activates without switching), then back to the first.
    cmd_tx
        .send(AppCommand::NewChat { profile_id: None })
        .unwrap();
    let other = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let AppEvent::ChatActivated { id: other_id, .. } = other else {
        unreachable!()
    };
    cmd_tx.send(AppCommand::SwitchChat(chat_id)).unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ChatActivated { id, .. } if *id == chat_id),
    )
    .await
    .unwrap();
    cmd_tx
        .send(AppCommand::SendMessage("delegate this".into()))
        .unwrap();
    let list = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::ChatList(chats)
            if chats.iter().any(|c| c.id == chat_id
                && c.children.iter().any(|r| r.running && r.message_count == 2)))
    })
    .await
    .expect("the running transcript with its first round on the list");
    let AppEvent::ChatList(chats) = list else {
        unreachable!()
    };
    let run_id = chats.iter().find(|c| c.id == chat_id).unwrap().children[0].id;
    (
        dir, backend, cmd_tx, evt_rx, handle, chat_id, run_id, other_id,
    )
}

/// The running transcript is a row of the list (marked *running*, its count
/// updated as rounds file), opens read-only with the rounds so far, and the
/// parent ↔ child switch — both ways — does **not** cancel the turn: the
/// turn ends only when `Esc` says so, the parent's feed is re-activated whole
/// at landing, and the landed row is the same transcript, no longer running.
#[tokio::test]
async fn a_running_transcript_is_listed_opens_and_survives_the_switch() {
    let (dir, backend, cmd_tx, mut evt_rx, handle, chat_id, run_id, _) = running_delegation().await;

    // Open the running transcript: its persona, its two rounds so far, the
    // turn named as live — and the engine is still waiting on the child.
    cmd_tx.send(AppCommand::SwitchChat(run_id)).unwrap();
    let opened = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let AppEvent::ChatActivated {
        id,
        messages,
        child,
        live_turn,
        ..
    } = opened
    else {
        unreachable!()
    };
    assert_eq!(id, run_id);
    assert_eq!(messages.len(), 3, "{messages:?}");
    assert_eq!(messages[0].text, "what time is it");
    assert_eq!(messages[1].tool_calls[0].name, "current_time");
    assert!(child.is_some());
    // The turn is in flight on this transcript, under the run's own stream,
    // and the text the hanging round has streamed so far comes along
    // (docs/history/subagent-live.md §8).
    let live = live_turn.expect("the turn is in flight on this transcript");
    assert_ne!(live.stream, live.turn);
    assert_eq!(
        live.partial.as_ref().map(|p| p.text.as_str()),
        Some("thinking…"),
        "{live:?}"
    );
    assert_eq!(
        backend.requests().len(),
        3,
        "parent, child, child (hanging)"
    );

    // Back to the parent: still generating, nothing cancelled — and the
    // round in progress comes along: the `call_subagent` card, still running
    // (docs/history/subagent-live.md §8, last bullet).
    cmd_tx.send(AppCommand::SwitchChat(chat_id)).unwrap();
    let back = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let AppEvent::ChatActivated {
        id,
        messages,
        live_turn,
        ..
    } = back
    else {
        unreachable!()
    };
    assert_eq!(id, chat_id);
    assert_eq!(messages.len(), 1, "no round of the parent has filed yet");
    let live = live_turn.expect("the turn is in flight on its chat");
    assert_eq!(live.stream, live.turn);
    let partial = live.partial.expect("the round in progress");
    assert_eq!(partial.tools.len(), 1, "{partial:?}");
    assert_eq!(partial.tools[0].name, "call_subagent");
    assert!(partial.tools[0].result.is_none(), "still running");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        !evt_rx_has_finished(&mut evt_rx),
        "the switch must not have cancelled the turn"
    );

    // Now `Esc`: the turn lands as cancelled; the feed needs no refresh —
    // the stream it resumed into carries the rest.
    cmd_tx.send(AppCommand::Cancel).unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    let list = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::ChatList(chats)
            if chats.iter().any(|c| c.id == chat_id && c.children.len() == 1 && !c.children[0].running))
    })
    .await;
    assert!(list.is_some(), "the landed row is no longer marked running");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    assert!(
        !evt_rx_has_activation(&mut evt_rx),
        "no re-activation at landing any more"
    );
    let chat = load(dir.path(), chat_id);
    // user → assistant(call) → tool; the cancelled final round writes nothing.
    assert_eq!(chat.messages.len(), 3, "{:?}", chat.messages);
    let landed = chat
        .child(run_id)
        .expect("the landed run keeps the running id");
    assert_eq!(landed.outcome, Some(RunOutcome::Cancelled));
    assert!(landed.messages.len() >= 3);
}

/// Drains what is already queued and reports whether a `ChatActivated` was among it.
fn evt_rx_has_activation(rx: &mut UnboundedReceiver<AppEvent>) -> bool {
    let mut seen = false;
    while let Ok(ev) = rx.try_recv() {
        if matches!(ev, AppEvent::ChatActivated { .. }) {
            seen = true;
        }
    }
    seen
}

/// Drains what is already queued and reports whether a `Finished` was among it.
fn evt_rx_has_finished(rx: &mut UnboundedReceiver<AppEvent>) -> bool {
    let mut seen = false;
    while let Ok(ev) = rx.try_recv() {
        if matches!(ev, AppEvent::Finished { .. }) {
            seen = true;
        }
    }
    seen
}

/// A switch that leaves the turn — any other chat — cancels as it always did.
#[tokio::test]
async fn a_switch_to_a_third_chat_still_cancels_the_turn() {
    let (_dir, _backend, cmd_tx, mut evt_rx, handle, _chat_id, _run_id, other) =
        running_delegation().await;
    cmd_tx.send(AppCommand::SwitchChat(other)).unwrap();
    let finished = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .expect("leaving the turn cancels it");
    assert!(matches!(
        finished,
        AppEvent::Finished {
            reason: FinishReason::Cancelled,
            ..
        }
    ));
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

/// Renaming the transcript while it runs edits the in-flight mirror, and the
/// title is carried onto the landed run as a manual one (fork F5).
#[tokio::test]
async fn renaming_a_running_transcript_lands_on_the_record() {
    let (dir, _backend, cmd_tx, mut evt_rx, handle, chat_id, run_id, _) =
        running_delegation().await;
    cmd_tx
        .send(AppCommand::RenameChat {
            id: run_id,
            title: "Моё имя".into(),
        })
        .unwrap();
    let renamed = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatRenamed { .. }))
        .await
        .unwrap();
    assert!(matches!(renamed, AppEvent::ChatRenamed { id, .. } if id == run_id));
    cmd_tx.send(AppCommand::Cancel).unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    let chat = load(dir.path(), chat_id);
    let run = chat.child(run_id).unwrap();
    assert_eq!(run.title, "Моё имя");
    assert!(run.renamed_manually);
}

/// A filed round of the **open** running transcript reaches the screen as
/// `TranscriptGrew`; one of a transcript that is not open only updates the
/// mirror and the list. Driven on a bare orchestrator, because the moment a
/// round files cannot be held still through the real loop.
#[test]
fn a_filed_round_grows_the_open_transcript() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let chat = Chat::from_profile(
        &crate::entities::profile::Profile::new("P", "sys"),
        "Родитель",
    );
    let parent = chat.id;
    orch.chats.push(chat);
    let generation = Uuid::new_v4();
    orch.inflight = Some(super::super::InflightTurn {
        generation,
        chat: parent,
        rounds: Vec::new(),
        partial: Default::default(),
        child: None,
        child_stream: Uuid::nil(),
        child_partial: Default::default(),
        child_line_role: MessageRole::Assistant,
        continuation: false,
    });
    let mut run = crate::entities::subagent::SubagentRun::fixture("Критик", &["задание"]);
    run.outcome = None;
    let run_id = run.id;
    orch.handle_progress(
        generation,
        super::super::generation::TurnProgress::ChildStarted(Box::new(run)),
    );
    // The list carries it, running.
    let listed = loop {
        match rx.try_recv().unwrap() {
            AppEvent::ChatList(chats) => break chats,
            _ => continue,
        }
    };
    let row = &listed.iter().find(|c| c.id == parent).unwrap().children[0];
    assert!(row.running && row.id == run_id);

    // Not open: no growth event.
    orch.active_id = Some(parent);
    orch.handle_progress(
        generation,
        super::super::generation::TurnProgress::ChildRoundFiled(vec![Message::assistant("раз")]),
    );
    let mut grew = false;
    while let Ok(ev) = rx.try_recv() {
        grew |= matches!(ev, AppEvent::TranscriptGrew { .. });
    }
    assert!(!grew);

    // Open: the round arrives.
    orch.active_id = Some(run_id);
    orch.handle_progress(
        generation,
        super::super::generation::TurnProgress::ChildRoundFiled(vec![Message::assistant("два")]),
    );
    let grew = loop {
        match rx.try_recv().unwrap() {
            AppEvent::TranscriptGrew { id, messages } => break (id, messages),
            _ => continue,
        }
    };
    assert_eq!(grew.0, run_id);
    assert_eq!(grew.1[0].text, "два");
    assert_eq!(
        orch.inflight
            .as_ref()
            .unwrap()
            .child
            .as_ref()
            .unwrap()
            .messages
            .len(),
        3
    );

    // A step from another generation is dropped.
    orch.handle_progress(
        Uuid::new_v4(),
        super::super::generation::TurnProgress::ChildRoundFiled(vec![Message::assistant("чужое")]),
    );
    assert_eq!(
        orch.inflight
            .as_ref()
            .unwrap()
            .child
            .as_ref()
            .unwrap()
            .messages
            .len(),
        3
    );
    // `view()` resolves the running transcript; the first match is none.
    assert!(orch.parent_of(run_id) == Some(parent));
    assert_eq!(orch.first_match_in_chat(run_id, "задание"), None);
}

/// The turn's own round in progress is mirrored the same way (plan §8, last
/// bullet): chunks, thoughts and the calls it opened — running or answered —
/// accumulate in `partial`, are handed to a return to the chat as the seed of
/// its feed, and are reset by a filed round or a rewrite.
#[test]
fn the_parents_round_in_progress_is_mirrored_for_a_return() {
    use super::super::generation::{StreamStep, TurnProgress};
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let chat = Chat::from_profile(&crate::entities::profile::Profile::new("P", "sys"), "Чат");
    let chat_id = chat.id;
    orch.chats.push(chat);
    let generation = Uuid::new_v4();
    orch.inflight = Some(super::super::InflightTurn {
        generation,
        chat: chat_id,
        rounds: Vec::new(),
        partial: Default::default(),
        child: None,
        child_stream: Uuid::nil(),
        child_partial: Default::default(),
        child_line_role: MessageRole::Assistant,
        continuation: false,
    });
    let step = |s: StreamStep| TurnProgress::OwnStep(s);
    orch.handle_progress(generation, step(StreamStep::Thoughts("план".into())));
    orch.handle_progress(generation, step(StreamStep::Chunk("Смотрю".into())));
    orch.handle_progress(
        generation,
        step(StreamStep::ToolStarted {
            call_id: "c1".into(),
            name: "web_search".into(),
            arguments: "{}".into(),
        }),
    );
    orch.handle_progress(
        generation,
        step(StreamStep::ToolCall {
            call_id: "c1".into(),
            name: "web_search".into(),
            arguments: "{}".into(),
            result: "ок".into(),
            images: 0,
        }),
    );
    orch.handle_progress(
        generation,
        step(StreamStep::ToolStarted {
            call_id: "c2".into(),
            name: "call_subagent".into(),
            arguments: "{}".into(),
        }),
    );
    // Nothing of it goes to the screen from here — the live loop sends the
    // screen its events itself.
    while let Ok(ev) = rx.try_recv() {
        assert!(
            !matches!(
                ev,
                AppEvent::Chunk { .. } | AppEvent::ToolCallStarted { .. }
            ),
            "{ev:?}"
        );
    }

    // A return to the chat carries the round so far.
    orch.active_id = None;
    orch.activate_focused(chat_id, None);
    let live = loop {
        match rx.try_recv().unwrap() {
            AppEvent::ChatActivated { live_turn, .. } => break live_turn.unwrap(),
            _ => continue,
        }
    };
    let partial = live.partial.unwrap();
    assert_eq!(
        (partial.text.as_str(), partial.thoughts.as_str()),
        ("Смотрю", "план")
    );
    assert_eq!(partial.tools.len(), 2);
    assert_eq!(
        partial.tools[0].result.as_ref().map(|r| r.0.as_str()),
        Some("ок")
    );
    assert!(partial.tools[1].result.is_none());

    // The round files: the mirror of it is spent.
    orch.handle_progress(
        generation,
        TurnProgress::RoundFiled(vec![Message::assistant("Смотрю")]),
    );
    let t = orch.inflight.as_ref().unwrap();
    assert_eq!(t.rounds.len(), 1);
    assert!(t.partial.text.is_empty() && t.partial.tools.is_empty());

    // A rewrite drops the round in progress.
    orch.handle_progress(generation, step(StreamStep::Chunk("черновик".into())));
    orch.handle_progress(generation, step(StreamStep::Rewrite));
    assert!(orch.inflight.as_ref().unwrap().partial.text.is_empty());
}

/// The sub-agent's stream (docs/history/subagent-live.md §8): chunks are
/// kept as the round's partial text and forwarded — under the run's own
/// stream id — only while the transcript is open; a filed round resets the
/// partial; the run's end closes the open transcript's bubble.
#[test]
fn the_childs_stream_is_kept_and_forwarded_to_the_open_transcript() {
    use super::super::generation::{StreamStep, TurnProgress};
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let chat = Chat::from_profile(
        &crate::entities::profile::Profile::new("P", "sys"),
        "Родитель",
    );
    let parent = chat.id;
    orch.chats.push(chat);
    let generation = Uuid::new_v4();
    orch.inflight = Some(super::super::InflightTurn {
        generation,
        chat: parent,
        rounds: Vec::new(),
        partial: Default::default(),
        child: None,
        child_stream: Uuid::nil(),
        child_partial: Default::default(),
        child_line_role: MessageRole::Assistant,
        continuation: false,
    });
    let mut run = crate::entities::subagent::SubagentRun::fixture("Критик", &["задание"]);
    run.outcome = None;
    let run_id = run.id;
    orch.handle_progress(generation, TurnProgress::ChildStarted(Box::new(run)));
    let stream = orch.inflight.as_ref().unwrap().child_stream;
    assert_ne!(stream, Uuid::nil(), "a stream id of its own");
    assert_ne!(stream, generation);

    // Not open: kept, not forwarded.
    orch.active_id = Some(parent);
    orch.handle_progress(
        generation,
        TurnProgress::ChildStep(StreamStep::Thoughts("думаю".into())),
    );
    orch.handle_progress(
        generation,
        TurnProgress::ChildStep(StreamStep::Chunk("нача".into())),
    );
    while let Ok(ev) = rx.try_recv() {
        assert!(
            !matches!(ev, AppEvent::Chunk { .. } | AppEvent::Thoughts { .. }),
            "not forwarded while the parent is open: {ev:?}"
        );
    }
    let partial = orch.inflight.as_ref().unwrap().child_partial.clone();
    assert_eq!(
        (partial.text.as_str(), partial.thoughts.as_str()),
        ("нача", "думаю")
    );

    // Opening the transcript seeds the round so far under the stream id.
    orch.activate_focused(run_id, None);
    let activated = loop {
        match rx.try_recv().unwrap() {
            AppEvent::ChatActivated { live_turn, .. } => break live_turn.unwrap(),
            _ => continue,
        }
    };
    assert_eq!((activated.turn, activated.stream), (generation, stream));
    assert_eq!(activated.partial.unwrap().text, "нача");

    // Open: forwarded under the stream id.
    orch.handle_progress(
        generation,
        TurnProgress::ChildStep(StreamStep::Chunk("ло".into())),
    );
    orch.handle_progress(
        generation,
        TurnProgress::ChildStep(StreamStep::ToolStarted {
            call_id: "c9".into(),
            name: "web_search".into(),
            arguments: "{}".into(),
        }),
    );
    let mut seen = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        match ev {
            AppEvent::Chunk {
                generation_id,
                text,
            } => seen.push(("chunk", generation_id, text)),
            AppEvent::ToolCallStarted {
                generation_id,
                name,
                ..
            } => seen.push(("started", generation_id, name)),
            _ => {}
        }
    }
    assert_eq!(
        seen,
        vec![
            ("chunk", stream, "ло".to_string()),
            ("started", stream, "web_search".to_string())
        ]
    );
    assert_eq!(orch.inflight.as_ref().unwrap().child_partial.text, "начало");

    // A filed round resets the partial; the run's end closes the bubble.
    orch.handle_progress(
        generation,
        TurnProgress::ChildRoundFiled(vec![Message::assistant("начало")]),
    );
    assert!(
        orch.inflight
            .as_ref()
            .unwrap()
            .child_partial
            .text
            .is_empty()
    );
    orch.handle_progress(
        generation,
        TurnProgress::ChildEnded {
            outcome: RunOutcome::Completed,
            finished_at: chrono::Utc::now(),
            tokens: 3,
        },
    );
    let finished = loop {
        match rx.try_recv().unwrap() {
            AppEvent::Finished {
                generation_id,
                reason,
                ..
            } => break (generation_id, reason),
            _ => continue,
        }
    };
    assert_eq!(finished, (stream, FinishReason::Stop));
}

// ---- the transcript as a chat of the list (PR 4, research §3.7–§3.8) ----

/// Runs the standard delegation turn and returns the root and the ids of the
/// parent and the landed transcript.
async fn delegated() -> (tempfile::TempDir, Uuid, Uuid) {
    let (dir, _backend, _events, chat_id) = run_turn(
        vec![
            call("c1", "call_subagent", DELEGATE),
            text("it is noon"),
            text("done: noon"),
        ],
        no_auto_cfg(),
    )
    .await;
    let chat = load(dir.path(), chat_id);
    let run_id = chat.messages[1].tool_calls[0]
        .subagent
        .as_deref()
        .unwrap()
        .id;
    (dir, chat_id, run_id)
}

/// Reopens the orchestrator on `root` with a silent engine and waits out the
/// bootstrap; returns the channels, the list snapshot the bootstrap emitted
/// (it precedes the activation, so it must be caught here) and the first
/// activation event.
pub(super) async fn reopen(
    root: &std::path::Path,
) -> (
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
    Vec<crate::entities::chat::ChatSummary>,
    AppEvent,
) {
    let backend = ScriptRecorder::new(Vec::new()) as Arc<dyn EngineBackend>;
    // The config as the previous session left it on disk (the remembered
    // chat lives there), with the automatic titling off as every test here.
    let mut cfg = Storage::open(Paths::with_root(root))
        .unwrap()
        .json()
        .load_config()
        .unwrap_or_default();
    cfg.interface.auto_title = crate::shared::config::AutoTitleMode::Off;
    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(root, Some(backend), cfg);
    let mut list = Vec::new();
    let first = loop {
        let ev = tokio::time::timeout(std::time::Duration::from_secs(10), evt_rx.recv())
            .await
            .expect("the bootstrap activation")
            .expect("the event channel");
        match ev {
            AppEvent::ChatList(chats) => list = chats,
            AppEvent::ChatActivated { .. } => break ev,
            _ => {}
        }
    };
    (cmd_tx, evt_rx, handle, list, first)
}

#[tokio::test]
async fn the_list_nests_the_transcript_under_its_parent() {
    let (dir, chat_id, run_id) = delegated().await;
    let (cmd_tx, _evt_rx, handle, chats, _) = reopen(dir.path()).await;
    let parent = chats.iter().find(|c| c.id == chat_id).unwrap();
    assert_eq!(parent.children.len(), 1);
    assert_eq!(parent.children[0].id, run_id);
    assert_eq!(parent.children[0].title, "Critic");
    assert_eq!(parent.children[0].outcome, Some(RunOutcome::Completed));
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn opening_a_transcript_activates_it_read_only_with_its_names() {
    let (dir, chat_id, run_id) = delegated().await;
    let (cmd_tx, mut evt_rx, handle, _, _) = reopen(dir.path()).await;
    cmd_tx.send(AppCommand::SwitchChat(run_id)).unwrap();
    let ev = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ChatActivated { id, .. } if *id == run_id),
    )
    .await
    .unwrap();
    let AppEvent::ChatActivated {
        title,
        messages,
        draft,
        child,
        compaction,
        ..
    } = ev
    else {
        unreachable!()
    };
    assert_eq!(title, "Critic");
    assert_eq!(messages.len(), 2);
    assert!(draft.is_empty());
    assert!(compaction.is_none());
    let child = child.expect("a transcript announces its parent");
    assert_eq!(child.parent, chat_id);
    assert_eq!(child.system_message, "be harsh");
    // The names: the parent persona writes the instruction, the sub-agent answers.
    let names = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::CharacterNames(_)))
        .await
        .unwrap();
    let AppEvent::CharacterNames(names) = names else {
        unreachable!()
    };
    assert_eq!(names.assistant, "Critic");
    assert!(!names.user.is_empty());

    // Sending into it is refused with the text returned.
    cmd_tx
        .send(AppCommand::SendMessage("hello?".into()))
        .unwrap();
    let restored = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::RestoreInput(_)))
        .await
        .unwrap();
    assert!(matches!(restored, AppEvent::RestoreInput(t) if t == "hello?"));
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // The parent is untouched and the transcript was remembered as the last open.
    let chat = load(dir.path(), chat_id);
    assert_eq!(chat.messages.len(), 4);
    let cfg = Storage::open(Paths::with_root(dir.path()))
        .unwrap()
        .json()
        .load_config()
        .unwrap();
    assert_eq!(cfg.last_active_chat, Some(run_id));
}

#[tokio::test]
async fn a_remembered_transcript_is_restored_at_startup() {
    let (dir, _chat_id, run_id) = delegated().await;
    let (cmd_tx, _evt_rx, handle, _, _) = reopen(dir.path()).await;
    cmd_tx.send(AppCommand::SwitchChat(run_id)).unwrap();
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    let (cmd_tx, _evt_rx, handle, _, first) = reopen(dir.path()).await;
    assert!(matches!(first, AppEvent::ChatActivated { id, child: Some(_), .. } if id == run_id));
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn renaming_a_transcript_sticks_and_is_manual() {
    let (dir, chat_id, run_id) = delegated().await;
    let (cmd_tx, mut evt_rx, handle, _, _) = reopen(dir.path()).await;
    cmd_tx
        .send(AppCommand::RenameChat {
            id: run_id,
            title: "Harsh critic".into(),
        })
        .unwrap();
    let renamed = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatRenamed { .. }))
        .await
        .unwrap();
    assert!(
        matches!(renamed, AppEvent::ChatRenamed { id, title } if id == run_id && title == "Harsh critic")
    );
    // The parent's own title is untouched; the flush happens on quit.
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    let chat = load(dir.path(), chat_id);
    let run = chat.child(run_id).unwrap();
    assert_eq!(run.title, "Harsh critic");
    assert!(run.renamed_manually);
    assert_ne!(chat.title, "Harsh critic");
}

#[tokio::test]
async fn delete_and_clone_refuse_a_transcript_and_a_cloned_parent_reids_its_runs() {
    let (dir, chat_id, run_id) = delegated().await;
    let (cmd_tx, mut evt_rx, handle, _, _) = reopen(dir.path()).await;
    for cmd in [
        AppCommand::DeleteChat(run_id),
        AppCommand::CloneChat(run_id),
    ] {
        cmd_tx.send(cmd).unwrap();
        let err = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatListError(_)))
            .await
            .unwrap();
        assert!(matches!(err, AppEvent::ChatListError(_)));
    }
    cmd_tx.send(AppCommand::CloneChat(chat_id)).unwrap();
    let list = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ChatList(c) if c.len() == 2),
    )
    .await
    .unwrap();
    let AppEvent::ChatList(chats) = list else {
        unreachable!()
    };
    let clone = chats.iter().find(|c| c.id != chat_id).unwrap();
    assert_eq!(clone.children.len(), 1, "the transcript was copied along");
    assert_ne!(clone.children[0].id, run_id, "under an id of its own");
    // The original's transcript is still there, untouched.
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    let chat = load(dir.path(), chat_id);
    assert!(chat.child(run_id).is_some());
}

#[tokio::test]
async fn copying_a_transcript_labels_the_roles_as_its_own() {
    let (dir, _chat_id, run_id) = delegated().await;
    let (cmd_tx, mut evt_rx, handle, _, _) = reopen(dir.path()).await;
    cmd_tx.send(AppCommand::CopyChat(run_id)).unwrap();
    let copied = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::CopyToClipboard(_)))
        .await
        .unwrap();
    let AppEvent::CopyToClipboard(text) = copied else {
        unreachable!()
    };
    assert!(text.contains("Critic"), "{text}");
    assert!(text.contains("it is noon"), "{text}");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn a_requested_title_for_a_transcript_lands_on_it() {
    let (dir, chat_id, run_id) = delegated().await;
    let backend = ScriptRecorder::new(vec![text("Noon check")]) as Arc<dyn EngineBackend>;
    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(dir.path(), Some(backend), no_auto_cfg());
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    cmd_tx.send(AppCommand::AutoRenameChat(run_id)).unwrap();
    let renamed = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatRenamed { .. }))
        .await
        .unwrap();
    assert!(
        matches!(renamed, AppEvent::ChatRenamed { id, title } if id == run_id && title == "Noon check")
    );
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    let chat = load(dir.path(), chat_id);
    assert_eq!(chat.child(run_id).unwrap().title, "Noon check");
    assert!(!chat.child(run_id).unwrap().renamed_manually);
}

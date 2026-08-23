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
struct Script {
    chunks: Vec<ChatChunk>,
    hang: bool,
}

/// An engine that records every request it is given and plays the scripts in
/// order, one per `chat_stream` call — the parent's rounds and the child's
/// interleave on one engine exactly as they do in the application.
struct ScriptRecorder {
    requests: Mutex<Vec<ChatRequest>>,
    scripts: Mutex<VecDeque<Script>>,
}

impl ScriptRecorder {
    fn new(scripts: Vec<Script>) -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
            scripts: Mutex::new(scripts.into()),
        })
    }
    fn requests(&self) -> Vec<ChatRequest> {
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

fn call(id: &str, name: &str, args: &str) -> Script {
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

fn text(t: &str) -> Script {
    Script {
        chunks: vec![
            ChatChunk::Text(t.into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
        hang: false,
    }
}

fn hang(prefix: &str) -> Script {
    Script {
        chunks: vec![ChatChunk::Text(prefix.into())],
        hang: true,
    }
}

const DELEGATE: &str =
    r#"{"name":"Critic","system_message":"be harsh","message":"what time is it"}"#;

/// Runs one parent turn over `scripts`; returns the root, the recorder, every
/// event the UI saw, and the chat id.
async fn run_turn(
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

fn load(root: &std::path::Path, id: Uuid) -> Chat {
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
    assert!(matches!(cards[0], AppEvent::ToolCall { name, .. } if name == "call_subagent"));
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
async fn reopen(
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

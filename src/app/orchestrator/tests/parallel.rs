//! Orchestrator tests — the parallel group (spec §9.3.2,
//! docs/research/parallel-subagents.md §4.1–§4.5): the `call_subagent` calls
//! of one round run at once, at most `tools.subagent_parallel` of them, their
//! streams sharing the engine's `sessions`; the records land in the model's
//! order; the mirror keeps every running run apart. Part of the [`super`]
//! module (fixtures in mod.rs; the scripted engine's pieces in subagent.rs).

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::subagent::{Script, hang, load, text};
use super::*;
use crate::entities::subagent::RunOutcome;
use crate::shared::api::ChatRequest;
use crate::shared::api::contract::{ChatStream, ToolCallDelta};

/// An engine whose scripts are chosen by the request's **system message**
/// rather than by call order: two children running at once reach the engine
/// in whatever order the scheduler picks, so a queue per persona is the only
/// way to give each its own reply. The empty key is the parent's queue. It
/// also counts how many streams are open at once — the load-bearing number
/// for every assertion about `subagent_parallel` and `sessions` — and plays
/// each chunk after `delay_ms`, so two streams can overlap at all.
struct KeyedRecorder {
    queues: Mutex<Vec<(String, VecDeque<Script>)>>,
    requests: Mutex<Vec<ChatRequest>>,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: AtomicUsize,
    delay_ms: u64,
}

/// Decrements the open-stream count when the stream is dropped or finished.
struct OpenStream(Arc<AtomicUsize>);

impl Drop for OpenStream {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl KeyedRecorder {
    fn new(queues: Vec<(&str, Vec<Script>)>, delay_ms: u64) -> Arc<Self> {
        Arc::new(Self {
            queues: Mutex::new(
                queues
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v.into()))
                    .collect(),
            ),
            requests: Mutex::new(Vec::new()),
            in_flight: Arc::new(AtomicUsize::new(0)),
            max_in_flight: AtomicUsize::new(0),
            delay_ms,
        })
    }

    fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().unwrap().clone()
    }

    fn max_in_flight(&self) -> usize {
        self.max_in_flight.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl EngineBackend for KeyedRecorder {
    async fn chat_stream(
        &self,
        req: ChatRequest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<ChatStream> {
        self.requests.lock().unwrap().push(req.clone());
        let system = req.system.clone().unwrap_or_default();
        let script = {
            let mut queues = self.queues.lock().unwrap();
            let keyed = queues
                .iter()
                .position(|(k, _)| !k.is_empty() && system.contains(k.as_str()))
                .or_else(|| queues.iter().position(|(k, _)| k.is_empty()));
            keyed
                .and_then(|i| queues[i].1.pop_front())
                .unwrap_or(Script {
                    chunks: vec![ChatChunk::Finished(FinishReason::Stop)],
                    hang: false,
                })
        };
        let open = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(open, Ordering::SeqCst);
        let guard = OpenStream(self.in_flight.clone());
        let delay = self.delay_ms;
        let s = async_stream::stream! {
            let _open = guard;
            for chunk in script.chunks {
                if delay > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                }
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

/// One round with two tool calls — the shape a model that delegates two
/// tasks in one reply produces.
fn two_calls(a: (&str, &str), b: (&str, &str)) -> Script {
    let delta = |index: usize, (id, args): (&str, &str)| {
        ChatChunk::ToolCall(ToolCallDelta {
            thought_signature: None,
            index,
            id: Some(id.into()),
            name: Some("call_subagent".into()),
            arguments: args.into(),
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

const CRITIC: &str = r#"{"name":"Critic","system_message":"be harsh","message":"rate X"}"#;
const FAN: &str = r#"{"name":"Fan","system_message":"be kind","message":"rate X"}"#;

/// Runs one parent turn on `backend`; returns the root, every event the UI
/// saw, and the chat id.
async fn run_turn_on(
    backend: Arc<KeyedRecorder>,
    cfg: AppConfig,
) -> (tempfile::TempDir, Vec<AppEvent>, Uuid) {
    let (dir, cmd_tx, mut evt_rx, handle) =
        spawn_orch_cfg(Some(backend.clone() as Arc<dyn EngineBackend>), cfg);
    let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let AppEvent::ChatActivated { id: chat_id, .. } = active else {
        unreachable!()
    };
    cmd_tx
        .send(AppCommand::SendMessage("delegate both".into()))
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
    (dir, events, chat_id)
}

/// A backend for the two-sibling turn: the parent delegates twice in one
/// reply, each persona answers with its own text, the parent sums up.
fn two_siblings(critic: Script, fan: Script, delay_ms: u64) -> Arc<KeyedRecorder> {
    KeyedRecorder::new(
        vec![
            (
                "",
                vec![
                    two_calls(("c1", CRITIC), ("c2", FAN)),
                    text("both views in"),
                ],
            ),
            ("be harsh", vec![critic]),
            ("be kind", vec![fan]),
        ],
        delay_ms,
    )
}

fn cfg(parallel: u32, sessions: u32) -> AppConfig {
    let mut cfg = no_auto_cfg();
    cfg.tools.subagent_parallel = parallel;
    cfg.engine.managed.sessions = sessions;
    cfg
}

/// The two runs of a landed two-sibling turn, in the model's order.
fn landed_runs(chat: &Chat) -> Vec<crate::entities::subagent::SubagentRun> {
    chat.messages[1]
        .tool_calls
        .iter()
        .map(|r| {
            r.subagent
                .as_deref()
                .cloned()
                .expect("a run on every call's record")
        })
        .collect()
}

/// Two siblings at `subagent_parallel = 2` with two sessions stream **at
/// once**, and land as two records in the model's order — each with its own
/// persona's reply, the tool messages behind them in call order, the parent
/// summing up after both. The cards open together and close as each lands.
#[tokio::test]
async fn two_siblings_run_at_once_and_land_in_the_models_order() {
    let backend = two_siblings(text("harsh view"), text("kind view"), 40);
    let (dir, events, chat_id) = run_turn_on(backend.clone(), cfg(2, 2)).await;
    assert_eq!(
        backend.max_in_flight(),
        2,
        "both children streamed together"
    );

    let chat = load(dir.path(), chat_id);
    // user → assistant(two calls) → tool(c1) → tool(c2) → assistant(final)
    assert_eq!(chat.messages.len(), 5, "{:?}", chat.messages);
    let runs = landed_runs(&chat);
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].name.as_deref(), Some("Critic"));
    assert_eq!(runs[0].final_reply(), Some("harsh view"));
    assert_eq!(runs[1].name.as_deref(), Some("Fan"));
    assert_eq!(runs[1].final_reply(), Some("kind view"));
    assert!(
        runs.iter()
            .all(|r| r.outcome == Some(RunOutcome::Completed))
    );
    assert_eq!(chat.messages[1].tool_calls[0].id, "c1");
    assert_eq!(chat.messages[1].tool_calls[1].id, "c2");
    assert!(
        chat.messages[2].text.starts_with("harsh view"),
        "{}",
        chat.messages[2].text
    );
    assert!(
        chat.messages[3].text.starts_with("kind view"),
        "{}",
        chat.messages[3].text
    );
    assert_eq!(chat.messages[4].text, "both views in");

    // The parent's second request carries both tool results, in call order —
    // what a strict provider checks.
    let reqs = backend.requests();
    let last = reqs.last().unwrap();
    let tools: Vec<&str> = last
        .messages
        .iter()
        .filter_map(|m| m.tool_call_id.as_deref())
        .collect();
    assert_eq!(tools, vec!["c1", "c2"], "{last:?}");

    // Both cards opened before either closed.
    let started: Vec<usize> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| matches!(e, AppEvent::ToolCallStarted { .. }).then_some(i))
        .collect();
    let closed: Vec<usize> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| matches!(e, AppEvent::ToolCall { .. }).then_some(i))
        .collect();
    assert_eq!((started.len(), closed.len()), (2, 2), "{events:?}");
    assert!(
        started[1] < closed[0],
        "cards open together: {started:?} / {closed:?}"
    );
}

/// At the default `subagent_parallel = 1` the same reply runs its two
/// sub-agents one after the other, in the model's order — the behaviour the
/// tool shipped with, bit for bit.
#[tokio::test]
async fn parallel_one_runs_the_siblings_one_after_another() {
    let backend = two_siblings(text("harsh view"), text("kind view"), 20);
    let (dir, _events, chat_id) = run_turn_on(backend.clone(), cfg(1, 2)).await;
    assert_eq!(backend.max_in_flight(), 1, "one child at a time");
    let reqs = backend.requests();
    assert_eq!(reqs.len(), 4, "parent, Critic, Fan, parent: {reqs:?}");
    assert!(reqs[1].system.as_deref().unwrap().contains("be harsh"));
    assert!(reqs[2].system.as_deref().unwrap().contains("be kind"));
    let runs = landed_runs(&load(dir.path(), chat_id));
    assert_eq!(
        (runs[0].final_reply(), runs[1].final_reply()),
        (Some("harsh view"), Some("kind view"))
    );
}

/// Two sub-agents alive on **one** session take turns at the engine: never
/// two streams open, yet both runs are in flight — the list shows both
/// running before either has ended (research §4.2, fork F3).
#[tokio::test]
async fn siblings_share_the_one_session_but_are_both_alive() {
    let backend = two_siblings(text("harsh view"), text("kind view"), 40);
    let (dir, events, chat_id) = run_turn_on(backend.clone(), cfg(2, 1)).await;
    assert_eq!(
        backend.max_in_flight(),
        1,
        "one session, one stream at a time"
    );
    let both_running = events.iter().any(|e| match e {
        AppEvent::ChatList(chats) => chats.iter().any(|c| {
            c.id == chat_id && c.children.len() == 2 && c.children.iter().all(|r| r.running)
        }),
        _ => false,
    });
    assert!(
        both_running,
        "both runs listed as running at once: {events:?}"
    );
    let runs = landed_runs(&load(dir.path(), chat_id));
    assert!(
        runs.iter()
            .all(|r| r.outcome == Some(RunOutcome::Completed))
    );
}

/// One sibling's timeout ends that run alone: it lands `TimedOut` with what
/// it had, the other completes, and the parent is told about both.
#[tokio::test]
async fn one_siblings_timeout_leaves_the_other_to_complete() {
    let backend = two_siblings(hang("thinking"), text("kind view"), 10);
    let mut c = cfg(2, 2);
    c.tools.subagent_run_timeout_secs = 1;
    let (dir, _events, chat_id) = run_turn_on(backend.clone(), c).await;
    let chat = load(dir.path(), chat_id);
    let runs = landed_runs(&chat);
    assert_eq!(runs[0].outcome, Some(RunOutcome::TimedOut), "{:?}", runs[0]);
    assert_eq!(runs[1].outcome, Some(RunOutcome::Completed));
    assert_eq!(runs[1].final_reply(), Some("kind view"));
    assert!(
        chat.messages[2].text.contains("1"),
        "the parent is told the limit: {}",
        chat.messages[2].text
    );
    assert_eq!(chat.messages[4].text, "both views in");
}

/// The mirror keeps every running run apart (research §4.5): two rows in the
/// list, each transcript's stream forwarded only when it is the open
/// conversation and under its own stream id, and a switch between the parent
/// and either transcript staying inside the turn.
#[test]
fn two_running_transcripts_are_mirrored_and_forwarded_apart() {
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
        children: Vec::new(),
        continuation: false,
    });
    let mut a = crate::entities::subagent::SubagentRun::fixture("Критик", &["задание"]);
    a.outcome = None;
    let mut b = crate::entities::subagent::SubagentRun::fixture("Поэт", &["ода"]);
    b.outcome = None;
    let (a_id, b_id) = (a.id, b.id);
    orch.handle_progress(generation, TurnProgress::ChildStarted(Box::new(a)));
    orch.handle_progress(generation, TurnProgress::ChildStarted(Box::new(b)));
    let listed = loop {
        match rx.try_recv().unwrap() {
            AppEvent::ChatList(chats) if chats[0].children.len() == 2 => break chats,
            _ => continue,
        }
    };
    assert!(listed[0].children.iter().all(|c| c.running));
    let streams: Vec<Uuid> = orch
        .inflight
        .as_ref()
        .unwrap()
        .children
        .iter()
        .map(|c| c.stream)
        .collect();
    assert_ne!(streams[0], streams[1], "a stream id each");

    // A is open: B's chunk is kept, not forwarded; A's is forwarded under A's stream.
    orch.active_id = Some(a_id);
    orch.handle_progress(
        generation,
        TurnProgress::ChildStep {
            run: b_id,
            step: StreamStep::Chunk("тихо".into()),
        },
    );
    orch.handle_progress(
        generation,
        TurnProgress::ChildStep {
            run: a_id,
            step: StreamStep::Chunk("громко".into()),
        },
    );
    let mut chunks = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let AppEvent::Chunk {
            generation_id,
            text,
        } = ev
        {
            chunks.push((generation_id, text));
        }
    }
    assert_eq!(chunks, vec![(streams[0], "громко".to_string())]);
    let turn = orch.inflight.as_ref().unwrap();
    assert_eq!(turn.child(b_id).unwrap().partial.text, "тихо");
    assert_eq!(turn.child(a_id).unwrap().partial.text, "громко");

    // Parent ↔ A ↔ B: every leg stays inside the turn.
    assert!(orch.switch_within_turn(b_id));
    orch.active_id = Some(b_id);
    assert!(orch.switch_within_turn(parent));
    orch.active_id = Some(parent);
    assert!(orch.switch_within_turn(a_id));
    assert!(
        !orch.switch_within_turn(Uuid::new_v4()),
        "a third chat cancels"
    );

    // B ends first: its row stops running, A's does not.
    orch.handle_progress(
        generation,
        TurnProgress::ChildEnded {
            run: b_id,
            outcome: RunOutcome::Completed,
            finished_at: chrono::Utc::now(),
            tokens: 2,
        },
    );
    let listed = loop {
        match rx.try_recv().unwrap() {
            AppEvent::ChatList(chats) => break chats,
            _ => continue,
        }
    };
    let rows = &listed[0].children;
    assert!(rows.iter().find(|c| c.id == a_id).unwrap().running);
    assert!(!rows.iter().find(|c| c.id == b_id).unwrap().running);
}

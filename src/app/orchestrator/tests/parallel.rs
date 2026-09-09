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
pub(super) struct KeyedRecorder {
    queues: Mutex<Vec<(String, VecDeque<Script>)>>,
    requests: Mutex<Vec<ChatRequest>>,
    /// Per request, in arrival order: its system message and how many
    /// streams were open when it arrived — what a stream that waited for
    /// room in the pool (admission-by-budget §4.1) looks like from the engine.
    arrivals: Mutex<Vec<(String, usize)>>,
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
    pub(super) fn new(queues: Vec<(&str, Vec<Script>)>, delay_ms: u64) -> Arc<Self> {
        Arc::new(Self {
            queues: Mutex::new(
                queues
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v.into()))
                    .collect(),
            ),
            requests: Mutex::new(Vec::new()),
            arrivals: Mutex::new(Vec::new()),
            in_flight: Arc::new(AtomicUsize::new(0)),
            max_in_flight: AtomicUsize::new(0),
            delay_ms,
        })
    }

    pub(super) fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().unwrap().clone()
    }

    /// How many streams were open when each request of `persona` arrived,
    /// in its request order.
    pub(super) fn open_at_arrival(&self, persona: &str) -> Vec<usize> {
        self.arrivals
            .lock()
            .unwrap()
            .iter()
            .filter(|(system, _)| system.contains(persona))
            .map(|(_, open)| *open)
            .collect()
    }

    pub(super) fn max_in_flight(&self) -> usize {
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
        self.arrivals.lock().unwrap().push((system, open - 1));
        self.max_in_flight.fetch_max(open, Ordering::SeqCst);
        let guard = OpenStream(self.in_flight.clone());
        let delay = self.delay_ms;
        let s = async_stream::stream! {
            let _open = guard;
            let mut cancelled = false;
            for chunk in script.chunks {
                if delay > 0 {
                    // A delayed script ends the way a real stream does when
                    // its token fires mid-way — `Finished(Cancelled)` at once
                    // — so a displaced silent stream (silent-preemption §4)
                    // reads on the recorder as it reads on the engine.
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_millis(delay)) => {}
                        _ = cancel.cancelled() => {
                            cancelled = true;
                        }
                    }
                }
                if cancelled {
                    yield ChatChunk::Finished(FinishReason::Cancelled);
                    break;
                }
                yield chunk;
            }
            if script.hang && !cancelled {
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
    // A managed server above one session has a shared KV pool of `-c`, and
    // every child reserves at least its reply cap in it — 2048 here, the
    // default profile's `max_tokens`, below `subagent_max_tokens`
    // (admission-by-budget §4.2). A roomy pool keeps the permit count the
    // only bound the group's tests measure; the pool tests at the end of the
    // file size it on purpose.
    cfg.engine.managed.context_size = 65_536;
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

// ---------------------------------------------------------------------------
// Admission by budget (docs/research/admission-by-budget.md §4, §7): a
// managed server above one session shares one KV pool of `-c`, and a child's
// stream reserves its prompt plus its reply cap in it — waiting for room
// rather than taking a permit it would overflow the pool with.
// ---------------------------------------------------------------------------

/// A script that reports an exact `usage` before it finishes — what a real
/// server does, and what the budget's floor and calibration read.
pub(super) fn sized(mut script: Script, prompt_tokens: u32, completion_tokens: u32) -> Script {
    let end = script.chunks.pop().expect("a script ends with Finished");
    script
        .chunks
        .push(ChatChunk::Usage(crate::shared::api::contract::TokenUsage {
            prompt_tokens,
            completion_tokens,
            reasoning_tokens: 0,
            prefill: None,
        }));
    script.chunks.push(end);
    script
}

/// A reply of `n` text chunks — long enough on the recorder's delay to still
/// be streaming when a sibling comes back for its second round.
pub(super) fn long_text(n: usize) -> Script {
    let mut chunks: Vec<ChatChunk> = (0..n).map(|_| ChatChunk::Text("kind ".into())).collect();
    chunks.push(ChatChunk::Finished(FinishReason::Stop));
    Script {
        chunks,
        hang: false,
    }
}

/// A child whose reservation is at least its 2048-token reply cap: two such
/// do not fit a pool of 4000, so with two sessions they still stream one at
/// a time — and both complete, in the model's order.
#[tokio::test]
async fn siblings_that_do_not_fit_the_pool_take_turns() {
    let backend = two_siblings(text("harsh view"), text("kind view"), 40);
    let mut c = cfg(2, 2);
    c.engine.managed.context_size = 4000;
    let (dir, _events, chat_id) = run_turn_on(backend.clone(), c).await;
    assert_eq!(
        backend.max_in_flight(),
        1,
        "two sessions, but two 2048-capped reservations exceed 4000"
    );
    let runs = landed_runs(&load(dir.path(), chat_id));
    assert_eq!(
        (runs[0].final_reply(), runs[1].final_reply()),
        (Some("harsh view"), Some("kind view"))
    );
    assert!(
        runs.iter()
            .all(|r| r.outcome == Some(RunOutcome::Completed))
    );
}

/// A call to a tool nobody registered: the round is a tool round (a second
/// request follows it) and nothing runs in between.
fn call_unknown() -> Script {
    Script {
        chunks: vec![
            ChatChunk::ToolCall(ToolCallDelta {
                thought_signature: None,
                index: 0,
                id: Some("k1".into()),
                name: Some("no_such_tool".into()),
                arguments: "{}".into(),
            }),
            ChatChunk::Finished(FinishReason::ToolCalls),
        ],
        hang: false,
    }
}

/// A child run's rounds are their own population for the budget's ratio
/// (docs/research/title-impersonation-usage.md §3.1, fork F4): the parent's
/// first round reporting an exact prompt far above its estimate scales the
/// parent's later reservations and not the children's — two children that
/// fit a pool of 12000 on the raw estimate still run together — while a
/// child's own round reporting the same scales the children's: Critic's
/// second round, priced by it, waits for Fan's stream to end.
#[tokio::test]
async fn a_childs_ratio_is_the_childrens_not_the_parents() {
    let siblings = |parent_sized: bool| {
        let first = two_calls(("c1", CRITIC), ("c2", FAN));
        let first = if parent_sized {
            sized(first, 1_000_000, 1)
        } else {
            first
        };
        KeyedRecorder::new(
            vec![
                ("", vec![first, text("both views in")]),
                ("be harsh", vec![text("harsh view")]),
                ("be kind", vec![text("kind view")]),
            ],
            40,
        )
    };
    let mut c = cfg(2, 2);
    c.engine.managed.context_size = 12_000;

    let raw = siblings(false);
    let (_d, _e, _id) = run_turn_on(raw.clone(), c.clone()).await;
    assert_eq!(raw.max_in_flight(), 2, "on the raw estimate both fit");

    let parent = siblings(true);
    let (_d, _e, _id) = run_turn_on(parent.clone(), c.clone()).await;
    assert_eq!(
        parent.max_in_flight(),
        2,
        "the parent's usage is the parent's kind's: the children still fit"
    );

    // A child's own round reports the under-count: Critic's second request
    // is priced by the children's ratio — far above the pool — and waits
    // for Fan's long reply, arriving with no stream open.
    let child = KeyedRecorder::new(
        vec![
            (
                "",
                vec![
                    two_calls(("c1", CRITIC), ("c2", FAN)),
                    text("both views in"),
                ],
            ),
            (
                "be harsh",
                vec![sized(call_unknown(), 1_000_000, 1), text("harsh view")],
            ),
            ("be kind", vec![long_text(12)]),
        ],
        40,
    );
    let (dir, _events, chat_id) = run_turn_on(child.clone(), c).await;
    assert_eq!(
        child.open_at_arrival("be harsh").get(1),
        Some(&0),
        "Critic's second round, priced by the children's own ratio, waited for Fan"
    );
    let runs = landed_runs(&load(dir.path(), chat_id));
    assert!(
        runs.iter()
            .all(|r| r.outcome == Some(RunOutcome::Completed))
    );
}

/// A child's second round reserves at least what the server said its first
/// round was plus what it generated (research §4.2's floor): Critic's first
/// round is tiny by estimate but reports 5000 generated tokens, so its
/// second request must wait for Fan's long reply to end — it arrives with
/// no stream open — where the raw estimate would have fit next to it.
#[tokio::test]
async fn a_childs_second_round_reserves_at_least_its_last_exact_size() {
    let recorder = |floored: bool| {
        // An exact prompt *below* the estimate keeps the density at 1.0, so
        // only the floor can make the difference here.
        let first = if floored {
            sized(call_unknown(), 1, 12_000)
        } else {
            call_unknown()
        };
        KeyedRecorder::new(
            vec![
                (
                    "",
                    vec![
                        two_calls(("c1", CRITIC), ("c2", FAN)),
                        text("both views in"),
                    ],
                ),
                ("be harsh", vec![first, text("harsh view")]),
                ("be kind", vec![long_text(12)]),
            ],
            40,
        )
    };
    // Fan's stream reserves its estimate — the tool schemas included, about
    // 4600 (docs/research/roll-usage-calibration.md §3.1) — plus the 2048
    // cap; Critic's second round the same raw, 12 001 + 2048 floored: two
    // raw fit a pool of 16 000, the floored one does not.
    let mut c = cfg(2, 2);
    c.engine.managed.context_size = 16_000;

    let raw = recorder(false);
    let (_d, _e, _id) = run_turn_on(raw.clone(), c.clone()).await;
    // The first requests of the two children race for the engine, so only
    // the second round's arrival is asserted.
    assert_eq!(
        raw.open_at_arrival("be harsh").get(1),
        Some(&1),
        "raw: Critic's second round streams next to Fan's reply"
    );

    let floored = recorder(true);
    let (dir, _events, chat_id) = run_turn_on(floored.clone(), c).await;
    assert_eq!(
        floored.open_at_arrival("be harsh").get(1),
        Some(&0),
        "floored: 5001 + 2048 does not fit next to Fan — it waited for Fan to end"
    );
    let runs = landed_runs(&load(dir.path(), chat_id));
    assert_eq!(runs[0].final_reply(), Some("harsh view"));
    assert!(
        runs.iter()
            .all(|r| r.outcome == Some(RunOutcome::Completed))
    );
}

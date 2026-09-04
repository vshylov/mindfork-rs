//! Orchestrator tests — the concurrent segment (spec §6.3,
//! docs/research/concurrent-tools.md §4.2–§4.3): a round's consecutive
//! concurrent-marked calls run at once, at most `concurrent_calls` of them;
//! the results and their effects land in the model's order whatever finishes
//! first; anything unmarked breaks the segment and runs alone. Part of the
//! [`super`] module (fixtures in mod.rs; the scripted engine in subagent.rs).

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::subagent::{Script, ScriptRecorder, load, text};
use super::*;
use crate::entities::profile::ToolId;
use crate::features::tools::meta::ToolGroup;
use crate::features::tools::{ChatEffect, Tool, ToolContext, ToolOutcome};
use crate::shared::api::contract::{FinishReason, ToolCallDelta};

/// A tool that says how long it takes and whether it may run with others. It
/// counts how many of its calls are in flight at once — the load-bearing
/// number of every assertion about the width — writes every start and end
/// into a log shared with its siblings, and answers with its own argument,
/// so a result recorded against the wrong call is visible (docs/lessons.md
/// §2: on a degrading path, assert *which* result, never `is_ok()`).
struct Probe {
    id: &'static str,
    concurrent: bool,
    delay_ms: u64,
    /// Emit a `SetSystemMessage("from <path>")` effect, for the order test.
    effect: bool,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: AtomicUsize,
    log: Arc<Mutex<Vec<String>>>,
}

/// Decrements the in-flight count when the call ends — or when its future is
/// dropped by a cancellation, which is what makes the count honest.
struct Open(Arc<AtomicUsize>);

impl Drop for Open {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Probe {
    fn new(
        id: &'static str,
        concurrent: bool,
        delay_ms: u64,
        log: &Arc<Mutex<Vec<String>>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            id,
            concurrent,
            delay_ms,
            effect: false,
            in_flight: Arc::new(AtomicUsize::new(0)),
            max_in_flight: AtomicUsize::new(0),
            log: log.clone(),
        })
    }

    fn max_in_flight(&self) -> usize {
        self.max_in_flight.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl Tool for Probe {
    fn id(&self) -> ToolId {
        self.id.into()
    }
    fn description(&self, _loc: &crate::shared::i18n::Locale) -> String {
        "a probe".into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        })
    }
    async fn invoke(
        &self,
        _ctx: &ToolContext,
        args: serde_json::Value,
    ) -> anyhow::Result<ToolOutcome> {
        let path = args["path"].as_str().unwrap_or_default().to_string();
        let open = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(open, Ordering::SeqCst);
        let _open = Open(self.in_flight.clone());
        self.log.lock().unwrap().push(format!("start {path}"));
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        self.log.lock().unwrap().push(format!("end {path}"));
        let mut out = ToolOutcome::text(format!("{}:{path}", self.id));
        if self.effect {
            out.effects
                .push(ChatEffect::SetSystemMessage(format!("from {path}")));
        }
        Ok(out)
    }
    fn group(&self) -> ToolGroup {
        ToolGroup::Files
    }
    fn ui_label(&self) -> &'static str {
        "probe"
    }
    fn concurrent(&self) -> bool {
        self.concurrent
    }
}

/// One round whose reply carries the given calls — `(call id, tool, path)`.
fn calls(specs: &[(&str, &str, &str)]) -> Script {
    let mut chunks: Vec<ChatChunk> = specs
        .iter()
        .enumerate()
        .map(|(index, (id, name, path))| {
            ChatChunk::ToolCall(ToolCallDelta {
                thought_signature: None,
                index,
                id: Some((*id).into()),
                name: Some((*name).into()),
                arguments: format!(r#"{{"path":"{path}"}}"#),
            })
        })
        .collect();
    chunks.push(ChatChunk::Finished(FinishReason::ToolCalls));
    Script {
        chunks,
        hang: false,
    }
}

/// A config at the given segment width (the managed section is the default
/// mode's) with the automatic titling off.
fn cfg(width: u32) -> AppConfig {
    let mut cfg = no_auto_cfg();
    cfg.engine.managed.concurrent_calls = width;
    cfg
}

/// Everything a run leaves behind.
struct Run {
    dir: tempfile::TempDir,
    events: Vec<AppEvent>,
    chat_id: Uuid,
    backend: Arc<ScriptRecorder>,
}

/// Runs one turn on a scripted engine with the probes registered and
/// **enabled on the profile** (a tool the profile does not offer is refused
/// at the disabled gate, the way any unknown name is). `cancel_after_started`
/// sends `Esc` once that many cards have opened.
async fn run_turn(
    tools: Vec<Arc<Probe>>,
    width: u32,
    scripts: Vec<Script>,
    cancel_after_started: Option<usize>,
) -> Run {
    let backend = ScriptRecorder::new(scripts);
    let extra: Vec<Arc<dyn Tool>> = tools.iter().map(|t| t.clone() as Arc<dyn Tool>).collect();
    let enabled: Vec<ToolId> = tools.iter().map(|t| t.id.into()).collect();
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch_tools(
        Some(backend.clone() as Arc<dyn EngineBackend>),
        cfg(width),
        extra,
    );
    // The startup emits the profile list and the active chat in whichever
    // order; both are needed before the message goes out.
    let (mut pid, mut chat_id) = (None, None);
    while pid.is_none() || chat_id.is_none() {
        match tokio::time::timeout(Duration::from_secs(5), evt_rx.recv())
            .await
            .expect("startup events")
        {
            Some(AppEvent::ProfileList(v)) if !v.is_empty() => pid = Some(v[0].id),
            Some(AppEvent::ChatActivated { id, .. }) => chat_id = Some(id),
            Some(_) => {}
            None => panic!("the orchestrator went away during startup"),
        }
    }
    let chat_id = chat_id.unwrap();
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: pid.unwrap(),
            edit: Box::new(ProfileEdit {
                enabled_tools: Some(enabled),
                ..Default::default()
            }),
        })
        .unwrap();
    cmd_tx.send(AppCommand::SendMessage("go".into())).unwrap();
    let mut events = Vec::new();
    let mut started = 0;
    let collect = async {
        while let Some(ev) = evt_rx.recv().await {
            let done = matches!(ev, AppEvent::Finished { .. });
            if matches!(ev, AppEvent::ToolCallStarted { .. }) {
                started += 1;
                if cancel_after_started == Some(started) {
                    cmd_tx.send(AppCommand::Cancel).unwrap();
                }
            }
            events.push(ev);
            if done {
                break;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(20), collect)
        .await
        .expect("the turn finishes");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    Run {
        dir,
        events,
        chat_id,
        backend,
    }
}

/// Positions of the card-open and card-close events, in order.
fn cards(events: &[AppEvent]) -> (Vec<usize>, Vec<usize>) {
    let started = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| matches!(e, AppEvent::ToolCallStarted { .. }).then_some(i))
        .collect();
    let closed = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| matches!(e, AppEvent::ToolCall { .. }).then_some(i))
        .collect();
    (started, closed)
}

/// `(call id, result)` of the landed round's records, in stored order.
fn records(chat: &crate::entities::chat::Chat) -> Vec<(String, String)> {
    chat.messages[1]
        .tool_calls
        .iter()
        .map(|r| (r.id.clone(), r.result.clone().unwrap_or_default()))
        .collect()
}

/// Three reads in one reply at a width of three run **at once**, land as
/// three records in the model's order — each with the result of its own
/// call — with the tool messages behind them in call order, and the cards
/// open together before any closes.
#[tokio::test]
async fn a_segment_runs_its_reads_at_once_and_records_them_in_order() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let read = Probe::new("slow_read", true, 60, &log);
    let run = run_turn(
        vec![read.clone()],
        3,
        vec![
            calls(&[
                ("r1", "slow_read", "a"),
                ("r2", "slow_read", "b"),
                ("r3", "slow_read", "c"),
            ]),
            text("done"),
        ],
        None,
    )
    .await;
    assert_eq!(
        read.max_in_flight(),
        3,
        "all three reads were in flight together"
    );

    let chat = load(run.dir.path(), run.chat_id);
    // user → assistant(three calls) → tool ×3 → assistant(final)
    assert_eq!(chat.messages.len(), 6, "{:?}", chat.messages);
    assert_eq!(
        records(&chat),
        vec![
            ("r1".to_string(), "slow_read:a".to_string()),
            ("r2".to_string(), "slow_read:b".to_string()),
            ("r3".to_string(), "slow_read:c".to_string()),
        ]
    );
    assert_eq!(chat.messages[5].text, "done");

    // The next request carries the tool results in call order — what a strict
    // provider checks.
    let reqs = run.backend.requests();
    let ids: Vec<&str> = reqs
        .last()
        .unwrap()
        .messages
        .iter()
        .filter_map(|m| m.tool_call_id.as_deref())
        .collect();
    assert_eq!(ids, vec!["r1", "r2", "r3"]);

    let (started, closed) = cards(&run.events);
    assert_eq!((started.len(), closed.len()), (3, 3), "{:?}", run.events);
    assert!(
        started[2] < closed[0],
        "every card opened before the first closed: {started:?} / {closed:?}"
    );
}

/// At a width of one — a local engine's default — the same reply takes the
/// sequential path bit for bit: one read at a time, in the model's order,
/// each card opening only after the previous one closed.
#[tokio::test]
async fn width_one_takes_the_sequential_path() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let read = Probe::new("slow_read", true, 20, &log);
    let run = run_turn(
        vec![read.clone()],
        1,
        vec![
            calls(&[("r1", "slow_read", "a"), ("r2", "slow_read", "b")]),
            text("done"),
        ],
        None,
    )
    .await;
    assert_eq!(read.max_in_flight(), 1);
    assert_eq!(
        *log.lock().unwrap(),
        vec!["start a", "end a", "start b", "end b"]
    );
    // The sequential path closes its cards at the round's end, as it always
    // has: two opens, then two closes, in the model's order.
    let (started, closed) = cards(&run.events);
    assert_eq!((started.len(), closed.len()), (2, 2), "{:?}", run.events);
    assert!(started[1] < closed[0], "{started:?} / {closed:?}");
    let chat = load(run.dir.path(), run.chat_id);
    assert_eq!(
        records(&chat)[1],
        ("r2".to_string(), "slow_read:b".to_string())
    );
}

/// The width bounds the segment: three reads at a width of two keep two in
/// flight, the third starting as a sibling finishes.
#[tokio::test]
async fn the_width_bounds_how_many_run_at_once() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let read = Probe::new("slow_read", true, 60, &log);
    let run = run_turn(
        vec![read.clone()],
        2,
        vec![
            calls(&[
                ("r1", "slow_read", "a"),
                ("r2", "slow_read", "b"),
                ("r3", "slow_read", "c"),
            ]),
            text("done"),
        ],
        None,
    )
    .await;
    assert_eq!(read.max_in_flight(), 2);
    let chat = load(run.dir.path(), run.chat_id);
    assert_eq!(records(&chat).len(), 3);
}

/// An unmarked call in the middle ends the segment: the read before it, the
/// writer, and the read after it run one after another in the model's
/// order — a read is never moved across a write (fork F1).
#[tokio::test]
async fn an_unmarked_call_breaks_the_segment() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let read = Probe::new("slow_read", true, 40, &log);
    let write = Probe::new("slow_write", false, 40, &log);
    let run = run_turn(
        vec![read.clone(), write.clone()],
        4,
        vec![
            calls(&[
                ("r1", "slow_read", "a"),
                ("w1", "slow_write", "x"),
                ("r2", "slow_read", "b"),
            ]),
            text("done"),
        ],
        None,
    )
    .await;
    assert_eq!(read.max_in_flight(), 1, "the reads never overlapped");
    assert_eq!(
        *log.lock().unwrap(),
        vec!["start a", "end a", "start x", "end x", "start b", "end b"]
    );
    let chat = load(run.dir.path(), run.chat_id);
    assert_eq!(
        records(&chat),
        vec![
            ("r1".to_string(), "slow_read:a".to_string()),
            ("w1".to_string(), "slow_write:x".to_string()),
            ("r2".to_string(), "slow_read:b".to_string()),
        ]
    );
}

/// A marked tool the profile does not offer is refused at its position, as
/// always, and ends the segment like any other non-member.
#[tokio::test]
async fn a_disabled_tool_is_refused_and_breaks_the_segment() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let read = Probe::new("slow_read", true, 40, &log);
    // Registered, marked, but not in the profile's enabled set (only the
    // tools passed to `run_turn` are enabled).
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch_tools(
        Some(ScriptRecorder::new(vec![
            calls(&[
                ("r1", "slow_read", "a"),
                ("x1", "other_read", "x"),
                ("r2", "slow_read", "b"),
            ]),
            text("done"),
        ]) as Arc<dyn EngineBackend>),
        cfg(4),
        vec![
            read.clone() as Arc<dyn Tool>,
            Probe::new("other_read", true, 40, &log) as Arc<dyn Tool>,
        ],
    );
    let (mut pid, mut chat_id) = (None, None);
    while pid.is_none() || chat_id.is_none() {
        match evt_rx.recv().await {
            Some(AppEvent::ProfileList(v)) if !v.is_empty() => pid = Some(v[0].id),
            Some(AppEvent::ChatActivated { id, .. }) => chat_id = Some(id),
            Some(_) => {}
            None => panic!("startup"),
        }
    }
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: pid.unwrap(),
            edit: Box::new(ProfileEdit {
                enabled_tools: Some(vec!["slow_read".into()]),
                ..Default::default()
            }),
        })
        .unwrap();
    cmd_tx.send(AppCommand::SendMessage("go".into())).unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    assert_eq!(read.max_in_flight(), 1, "the reads never overlapped");
    assert_eq!(
        *log.lock().unwrap(),
        vec!["start a", "end a", "start b", "end b"]
    );
    let chat = load(dir.path(), chat_id.unwrap());
    let recs = records(&chat);
    assert_eq!(recs[0].1, "slow_read:a");
    assert_eq!(recs[2].1, "slow_read:b");
    assert!(
        recs[1].1.contains("other_read"),
        "the refusal names the tool: {}",
        recs[1].1
    );
}

/// The effects of a segment land in the **model's** order, not in the order
/// the calls finished (fork F6): a slow first call and a fast second one
/// still leave the second's effect as the last applied — the same `Chat` a
/// sequential round would leave.
#[tokio::test]
async fn effects_land_in_the_models_order() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut read = Probe::new("slow_read", true, 0, &log);
    // A slow call for "a", a fast one for "b": `b` finishes first.
    Arc::get_mut(&mut read).unwrap().effect = true;
    let slow = Probe::new("slow_effect", true, 120, &log);
    let mut slow = slow;
    Arc::get_mut(&mut slow).unwrap().effect = true;
    let run = run_turn(
        vec![slow.clone(), read.clone()],
        2,
        vec![
            calls(&[("s1", "slow_effect", "a"), ("r1", "slow_read", "b")]),
            text("done"),
        ],
        None,
    )
    .await;
    assert_eq!(
        *log.lock().unwrap(),
        vec!["start a", "start b", "end b", "end a"],
        "b finished before a"
    );
    let chat = load(run.dir.path(), run.chat_id);
    assert_eq!(
        chat.system_message, "from b",
        "the effect of the later call in the model's order wins"
    );
}

/// `Esc` while a segment runs ends every member at once: each records the
/// cancellation text, the round is filed, the turn lands `Cancelled`.
#[tokio::test]
async fn esc_mid_segment_cancels_every_member() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let read = Probe::new("slow_read", true, 10_000, &log);
    let run = run_turn(
        vec![read.clone()],
        2,
        vec![
            calls(&[("r1", "slow_read", "a"), ("r2", "slow_read", "b")]),
            text("never reached"),
        ],
        Some(2),
    )
    .await;
    assert!(
        run.events.iter().any(|e| matches!(
            e,
            AppEvent::Finished {
                reason: FinishReason::Cancelled,
                ..
            }
        )),
        "{:?}",
        run.events.last()
    );
    let cancelled = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
        .t("loop.tool_cancelled")
        .to_string();
    let chat = load(run.dir.path(), run.chat_id);
    assert_eq!(
        records(&chat),
        vec![
            ("r1".to_string(), cancelled.clone()),
            ("r2".to_string(), cancelled),
        ]
    );
    assert_eq!(
        *log.lock().unwrap(),
        vec!["start a", "start b"],
        "neither member ran to its end"
    );
    assert_eq!(
        read.in_flight.load(Ordering::SeqCst),
        0,
        "the dropped futures released their count"
    );
}

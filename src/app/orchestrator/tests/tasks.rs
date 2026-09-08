//! Orchestrator tests — the tasks screen's snapshot (spec §11.10,
//! docs/research/tasks-screen.md §7): a run out is a running row with its
//! parent and its position, ends into a landed row with its outcome; the
//! landed half comes from the chats' live records and skips the archive; the
//! status bar's count and the running rows share one predicate; the
//! snapshot answers a request and follows the silent tasks. Part of the
//! [`super`] module (fixtures in mod.rs; the keyed engine in parallel.rs;
//! the background fixtures in background.rs).

use tokio_util::sync::CancellationToken;

use super::background::{begin, cfg, next, running_run, runs_out, start};
use super::background_runs::BackgroundRun;
use super::parallel::KeyedRecorder;
use super::subagent::{hang, text};
use super::*;
use crate::app::events::{BackgroundKind, TASK_LANDED_CAP, TaskList, TaskRun};
use crate::entities::chat::DeletedExchange;
use crate::entities::message::MessageRole;
use crate::entities::subagent::{RunOutcome, SubagentRun};

/// The row for `id` in a `TaskList` event, if the event is one and holds it.
fn task_row(e: &AppEvent, id: Uuid) -> Option<&TaskRun> {
    let AppEvent::TaskList(list) = e else {
        return None;
    };
    list.runs.iter().find(|r| r.id == id)
}

/// The last snapshot a bare orchestrator sent (the queue is drained).
fn last_task_list(rx: &mut UnboundedReceiver<AppEvent>) -> TaskList {
    let mut last = None;
    while let Ok(e) = rx.try_recv() {
        if let AppEvent::TaskList(list) = e {
            last = Some(*list);
        }
    }
    last.expect("a TaskList was sent")
}

/// A chat called `title` holding `runs` on one assistant message's records.
fn chat_with(title: &str, runs: Vec<SubagentRun>) -> Chat {
    let profile = Profile::new("P", "sys");
    let mut chat = Chat::from_profile(&profile, title);
    let mut msg = Message::assistant("");
    msg.tool_calls = runs.into_iter().map(SubagentRun::on_record).collect();
    chat.push_message(Message::user("go"));
    chat.push_message(msg);
    chat
}

/// A seat for `run` under `chat`, as [`Orchestrator::spawn_background_run`]
/// makes one — the fields the snapshot reads, a token nothing listens on.
fn seat(run: SubagentRun, chat: Uuid) -> BackgroundRun {
    BackgroundRun {
        generation: Uuid::new_v4(),
        run_id: run.id,
        chat,
        cancel: CancellationToken::new(),
        child: InflightChild {
            run,
            stream: Uuid::new_v4(),
            partial: Default::default(),
            line_role: MessageRole::Assistant,
            position: None,
        },
    }
}

/// A run out in the background is a running row naming its parent and,
/// once its loop has reported, its position; stopped, it becomes a landed
/// row with its outcome and no position — the row that was running is the
/// row that landed, by id.
#[tokio::test]
async fn a_run_out_is_a_running_row_and_lands_with_its_outcome() {
    let backend = KeyedRecorder::new(
        vec![
            ("", vec![start("c1"), text("started it"), text("noted")]),
            ("be harsh", vec![hang("thinking")]),
        ],
        10,
    );
    let (_dir, cmd_tx, mut rx, handle, chat_id) = begin(backend, cfg(2)).await;
    let run_id = running_run(&mut rx).await;
    // The child's loop reports its first round before it streams: the
    // snapshot that carries it says where the run is.
    let list = next(&mut rx, |e| {
        task_row(e, run_id).is_some_and(|r| r.running && r.position.is_some())
    })
    .await;
    let row = task_row(&list, run_id).unwrap();
    assert_eq!(row.parent, chat_id);
    assert!(row.background, "a seat of its own");
    assert_eq!(row.outcome, None);
    assert_eq!(row.position.as_ref().unwrap().round, 1);
    assert_eq!(row.position.as_ref().unwrap().name, "Critic");
    let AppEvent::TaskList(list) = &list else {
        unreachable!()
    };
    assert_eq!(list.app.len(), 4, "every silent task has a row");
    assert!(list.app.iter().all(|t| !t.running));

    cmd_tx
        .send(AppCommand::StopSubagentRun { id: run_id })
        .unwrap();
    next(&mut rx, runs_out(0)).await;
    let list = next(&mut rx, |e| task_row(e, run_id).is_some_and(|r| !r.running)).await;
    let row = task_row(&list, run_id).unwrap();
    assert_eq!(row.outcome, Some(RunOutcome::Cancelled));
    assert!(row.finished_at.is_some());
    assert_eq!(row.position, None, "a landed row has no position");
    assert!(row.background);
    let AppEvent::TaskList(list) = &list else {
        unreachable!()
    };
    assert!(
        list.runs.iter().all(|r| !r.running),
        "nothing is running once the only run landed"
    );
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

/// The landed half is the chats' **live** records: a run whose exchange was
/// taken back sits in the `deleted` archive and is on the screen no more
/// than in the conversation (research §5).
#[test]
fn the_landed_half_comes_from_the_live_records_and_skips_the_archive() {
    let (_d, mut orch) = bare_orch();
    let done = SubagentRun::fixture("Reviewer", &["q", "a"]);
    let done_id = done.id;
    let mut chat = chat_with("Plans", vec![done]);
    let gone = SubagentRun::fixture("Gone", &["q", "a"]);
    let gone_id = gone.id;
    let mut gone_msg = Message::assistant("");
    gone_msg.tool_calls = vec![gone.on_record()];
    chat.deleted.push(DeletedExchange {
        deleted_at: chrono::Utc::now(),
        messages: vec![gone_msg],
        draft: String::new(),
        cause: None,
    });
    orch.chats.push(chat);

    let list = orch.task_list();
    assert_eq!(list.runs.len(), 1, "{:?}", list.runs);
    let row = &list.runs[0];
    assert_eq!(row.id, done_id);
    assert_eq!(row.parent_title, "Plans");
    assert!(!row.running);
    assert_eq!(row.outcome, Some(RunOutcome::Completed));
    assert!(!row.background, "a turn's child, not a seat");
    assert!(list.runs.iter().all(|r| r.id != gone_id));
    assert_eq!(list.more_landed, 0);
}

/// Landed runs are newest first and capped, with the remainder counted
/// rather than dropped in silence (fork F1).
#[test]
fn landed_runs_are_newest_first_and_capped_with_a_count() {
    let (_d, mut orch) = bare_orch();
    let now = chrono::Utc::now();
    let runs: Vec<SubagentRun> = (0..TASK_LANDED_CAP + 10)
        .map(|i| {
            let mut run = SubagentRun::fixture(&format!("run-{i:02}"), &["q", "a"]);
            // Written oldest-first, so a list that merely kept record order
            // would show the oldest on top.
            run.finished_at =
                Some(now - chrono::Duration::minutes((TASK_LANDED_CAP + 10 - i) as i64));
            run
        })
        .collect();
    orch.chats.push(chat_with("Plans", runs));

    let list = orch.task_list();
    assert_eq!(list.runs.len(), TASK_LANDED_CAP);
    assert_eq!(list.more_landed, 10);
    assert_eq!(
        list.runs[0].title,
        format!("run-{:02}", TASK_LANDED_CAP + 9)
    );
    assert!(
        list.runs
            .windows(2)
            .all(|w| w[0].finished_at >= w[1].finished_at),
        "newest first"
    );
}

/// The status bar's count and the screen's running rows are one predicate
/// ([`BackgroundRun::is_out`]): a seat whose run has ended but not landed
/// is a landed row — from the mirror, which knows the outcome, not from the
/// placeholder record, which would read *unfinished* — and not counted.
#[test]
fn the_bar_count_and_the_running_rows_agree() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let out = SubagentRun {
        outcome: None,
        finished_at: None,
        background: true,
        ..SubagentRun::fixture("Critic", &["q"])
    };
    let ended = SubagentRun {
        outcome: Some(RunOutcome::Cancelled),
        background: true,
        ..SubagentRun::fixture("Judge", &["q", "a"])
    };
    let (out_id, ended_id) = (out.id, ended.id);
    // The placeholders the turn landed with: no outcome, `background`.
    let placeholders = [&out, &ended]
        .into_iter()
        .map(|r| SubagentRun {
            outcome: None,
            finished_at: None,
            messages: Vec::new(),
            ..r.clone()
        })
        .collect();
    let chat = chat_with("Space talk", placeholders);
    let chat_id = chat.id;
    orch.chats.push(chat);
    orch.background_runs.push(seat(out, chat_id));
    orch.background_runs.push(seat(ended, chat_id));

    orch.emit_background_runs();
    let mut bar = None;
    while let Ok(e) = rx.try_recv() {
        if let AppEvent::BackgroundRuns { out } = e {
            bar = Some(out);
        }
    }
    let list = orch.task_list();
    let running: Vec<&TaskRun> = list.runs.iter().filter(|r| r.running).collect();
    assert_eq!(bar, Some(running.len() as u32));
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].id, out_id);
    assert!(running[0].background);

    let ended_rows: Vec<&TaskRun> = list.runs.iter().filter(|r| r.id == ended_id).collect();
    assert_eq!(ended_rows.len(), 1, "one row per run, the mirror's");
    assert_eq!(ended_rows[0].outcome, Some(RunOutcome::Cancelled));
    assert!(!ended_rows[0].running);
    assert_eq!(list.runs.len(), 2);
}

/// A request is answered with the snapshot; a silent task beginning and
/// ending re-sends it with the task's row flipped.
#[test]
fn a_request_is_answered_and_the_silent_tasks_are_followed() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    orch.handle_command(AppCommand::RequestTasks);
    let list = last_task_list(&mut rx);
    assert!(list.runs.is_empty());
    assert_eq!(list.app.len(), 4);
    assert!(list.app.iter().all(|t| !t.running));

    orch.begin_bg(BackgroundKind::Reflection, CancellationToken::new(), None);
    let list = last_task_list(&mut rx);
    let reflection = list
        .app
        .iter()
        .find(|t| t.kind == BackgroundKind::Reflection)
        .unwrap();
    assert!(reflection.running);
    assert_eq!(list.app.iter().filter(|t| t.running).count(), 1);

    orch.handle_bg_done(
        BackgroundKind::Reflection,
        super::super::background::BgOutcome::Done,
        None,
    );
    let list = last_task_list(&mut rx);
    assert!(list.app.iter().all(|t| !t.running));
}

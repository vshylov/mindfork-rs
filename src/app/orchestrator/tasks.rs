//! The tasks screen's snapshot (spec §11.10, docs/research/tasks-screen.md
//! §4.3): everything the app is doing in the background, as rows — the
//! sub-agent and dialogue runs in flight, with their position, then the ones
//! that have landed, with their outcome, then the app's own silent tasks.
//!
//! A projection and nothing else (research R4): the running half is read off
//! the mirrors the orchestrator already keeps — the turn's children and the
//! background seats — and the landed half off the records on the chats,
//! through `Chat::children()` so a run whose exchange was taken back (it is
//! in the `deleted` archive) is not on the screen either. No index, no store,
//! no walk the chat list does not already make on the same events (R6):
//! [`Orchestrator::emit_chat_list`] calls the emit, and the position step
//! ([`super::generation::TurnProgress::ChildProgress`]) and the silent tasks'
//! begin/done are the only emits of its own.

use std::collections::HashSet;

use uuid::Uuid;

use crate::app::events::{AppEvent, AppTask, BackgroundKind, TASK_LANDED_CAP, TaskList, TaskRun};
use crate::entities::subagent::SubagentRun;

use super::{InflightChild, Orchestrator};

/// The silent tasks in the order the screen lists them — one row per kind,
/// always all four, so the screen's second section never looks like "nothing
/// exists" when it means "nothing is running" (docs/lessons.md §4).
const APP_TASKS: [BackgroundKind; 4] = [
    BackgroundKind::Reflection,
    BackgroundKind::Consolidation,
    BackgroundKind::SelfConsolidation,
    BackgroundKind::Compaction,
];

impl Orchestrator {
    /// Sends the tasks screen's snapshot ([`AppEvent::TaskList`]).
    pub(super) fn emit_task_list(&self) {
        let _ = self
            .evt_tx
            .send(AppEvent::TaskList(Box::new(self.task_list())));
    }

    /// Builds the snapshot: running first (newest first), then landed
    /// (newest first, capped at [`TASK_LANDED_CAP`] with the remainder
    /// counted), then the silent tasks (fork F8 of the research).
    ///
    /// A run in flight is listed from its mirror, whether it is a child of
    /// the turn or a background seat; a run that ended but has not landed
    /// yet (its turn is still running) is listed from the mirror too, as
    /// landed — the record on the chat is still the placeholder, which
    /// would read *unfinished*. Everything else comes from the chats.
    pub(super) fn task_list(&self) -> TaskList {
        let mut runs: Vec<TaskRun> = Vec::new();
        let mut mirrored: HashSet<Uuid> = HashSet::new();
        // The background seats: `is_out` is the status bar's predicate, so
        // the bar's count and the running rows agree by construction.
        for seat in &self.background_runs {
            let Some(row) = self.mirror_row(&seat.child, seat.chat, seat.is_out(), true) else {
                continue;
            };
            mirrored.insert(row.id);
            runs.push(row);
        }
        if let Some(turn) = &self.inflight {
            for child in &turn.children {
                let running = child.run.outcome.is_none();
                let Some(row) = self.mirror_row(child, turn.chat, running, false) else {
                    continue;
                };
                mirrored.insert(row.id);
                runs.push(row);
            }
        }
        for chat in &self.chats {
            for run in chat.children().filter(|r| !mirrored.contains(&r.id)) {
                runs.push(record_row(run, chat.id, chat.title.clone(), None, false));
            }
        }
        let (mut running, mut landed): (Vec<TaskRun>, Vec<TaskRun>) =
            runs.into_iter().partition(|r| r.running);
        running.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        landed.sort_by_key(|r| std::cmp::Reverse(r.finished_at.unwrap_or(r.created_at)));
        let more_landed = landed.len().saturating_sub(TASK_LANDED_CAP);
        landed.truncate(TASK_LANDED_CAP);
        running.extend(landed);
        TaskList {
            runs: running,
            more_landed,
            app: APP_TASKS
                .iter()
                .map(|&kind| AppTask {
                    kind,
                    running: self.bg_running(kind),
                })
                .collect(),
        }
    }

    /// A row off a mirror, named after its parent chat. A mirror whose chat
    /// is gone (deleted under a run that is still out) has no row: the run
    /// lands nowhere and the list shows it nowhere either.
    fn mirror_row(
        &self,
        child: &InflightChild,
        parent: Uuid,
        running: bool,
        background: bool,
    ) -> Option<TaskRun> {
        let title = self.chats.iter().find(|c| c.id == parent)?.title.clone();
        let position = running.then(|| child.position.clone()).flatten();
        Some(
            record_row(&child.run, parent, title, position, running)
                .with_background(background || child.run.background),
        )
    }
}

/// The row for one run record (or mirror).
fn record_row(
    run: &SubagentRun,
    parent: Uuid,
    parent_title: String,
    position: Option<crate::app::events::SubagentProgress>,
    running: bool,
) -> TaskRun {
    TaskRun {
        id: run.id,
        kind: run.kind,
        title: run.title.clone(),
        parent,
        parent_title,
        created_at: run.created_at,
        finished_at: run.finished_at,
        outcome: run.outcome,
        running,
        background: run.background,
        tokens: run.tokens,
        position,
    }
}

impl TaskRun {
    fn with_background(mut self, background: bool) -> Self {
        self.background = background;
        self
    }
}

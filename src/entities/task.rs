//! The app's own silent tasks and the tasks screen's snapshot (spec §11.10,
//! docs/research/tasks-screen.md): the kinds of background work there are
//! ([`BackgroundKind`]), one such task's state ([`AppTask`]), one sub-agent or
//! dialogue run as the screen lists it ([`TaskRun`]), and the list itself
//! ([`TaskList`]). Plain data: the orchestrator projects it and the screens
//! draw it, so it lives here — below both — rather than beside the event that
//! carries it, which `app::events` re-exports it for (docs/architecture.md §2,
//! docs/research/fsd-layer-gate.md).

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::entities::subagent::{RunKind, RunOutcome, SubagentProgress};

/// The kind of background task for the status-bar indicator
/// (`AppEvent::BackgroundTask`). `Hash` — used as the key of the background-task slot
/// registry (`orchestrator::background`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackgroundKind {
    /// Auto-reflection of the "self-model".
    Reflection,
    /// Auto-consolidation of notes ("sleep").
    Consolidation,
    /// Auto-consolidation of the "self-model" (self-model "sleep"): merge duplicate
    /// observations, compress a bloated description, link contradictions. See
    /// docs/history/self-model-consolidation.md.
    SelfConsolidation,
    /// Compressing the older part of a conversation into a rolling summary
    /// (`/compact`, spec §6.7).
    Compaction,
}

impl BackgroundKind {
    /// The bundle key of the task's name in the interface language — the
    /// tasks screen's row label, and the name a `/tasks stop` note quotes
    /// (spec §11.10, §11.7). One function for both surfaces, so the two
    /// cannot call one task two things
    /// (docs/research/tasks-stop-command.md §3.2).
    pub fn label_key(self) -> &'static str {
        match self {
            BackgroundKind::Reflection => "ui.tasks.app.reflection",
            BackgroundKind::Consolidation => "ui.tasks.app.consolidation",
            BackgroundKind::SelfConsolidation => "ui.tasks.app.self_consolidation",
            BackgroundKind::Compaction => "ui.tasks.app.compaction",
        }
    }
}

/// One sub-agent or dialogue run as the tasks screen lists it
/// ([`AppEvent::TaskList`](crate::app::events::AppEvent), spec §11.10): a projection of the orchestrator's
/// mirror while the run is in flight, and of the record on its parent chat
/// once it has landed — nothing is stored for the screen's sake
/// (docs/research/tasks-screen.md R4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRun {
    /// The run's id — what `Enter` opens and `F6` stops.
    pub id: Uuid,
    pub kind: RunKind,
    pub title: String,
    /// The chat whose exchange started it, and its title for the row.
    pub parent: Uuid,
    pub parent_title: String,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// How the run ended; `None` while it runs — or, with `running` false,
    /// a run that never reported (the chat list's *interrupted* /
    /// *unfinished* rows, spec §11.2).
    pub outcome: Option<RunOutcome>,
    /// The run is in flight right now — the orchestrator holds its mirror.
    pub running: bool,
    /// Out in the background (a seat of its own, spec §9.3.2) rather than a
    /// child of the turn in flight. Only a running background run can be
    /// stopped from the screen: `AppCommand::StopSubagentRun` names seats.
    pub background: bool,
    /// Completion tokens so far (the run's own count while it runs).
    pub tokens: u64,
    /// Where a running run stands — its latest [`SubagentProgress`] step,
    /// stored on the mirror by the orchestrator. `None` before its first
    /// round, and always on a landed row.
    pub position: Option<SubagentProgress>,
}

impl TaskRun {
    /// A background sub-agent run out right now, under a chat called
    /// "Plans", started ten minutes ago with no position yet. Shared by the
    /// tasks screen's and the runtime's tests — each keeping a copy of this
    /// literal is the sliding self-duplication the gate measures
    /// (docs/lessons.md §2); tests mutate the fields they are about.
    #[cfg(test)]
    pub fn fixture(title: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind: RunKind::Subagent,
            title: title.into(),
            parent: Uuid::new_v4(),
            parent_title: "Plans".into(),
            created_at: Utc::now() - chrono::Duration::minutes(10),
            finished_at: None,
            outcome: None,
            running: true,
            background: true,
            tokens: 0,
            position: None,
        }
    }
}

/// One of the app's own silent tasks on the tasks screen: running or idle,
/// which is all the slot registry can honestly say
/// (docs/research/tasks-screen.md §2.4, fork F2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppTask {
    pub kind: BackgroundKind,
    pub running: bool,
    /// Running, but not streaming: waiting for the silent lane's permit
    /// behind another task, or for room in the pool beside an interactive
    /// stream (docs/research/silent-tasks-budget.md §4.6). Read off the
    /// budget, never stored.
    pub waiting: bool,
}

/// The tasks screen's snapshot ([`AppEvent::TaskList`](crate::app::events::AppEvent)): the runs — running
/// first, then landed, each half newest first, the landed half capped — and
/// the app's own silent tasks.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskList {
    pub runs: Vec<TaskRun>,
    /// How many landed runs fell past the cap (`TASK_LANDED_CAP`); the
    /// screen says "…and n more" rather than truncating silently.
    pub more_landed: usize,
    /// Every [`BackgroundKind`], in one fixed order.
    pub app: Vec<AppTask>,
}

/// How many landed runs the tasks screen lists (docs/research/tasks-screen.md
/// fork F1): the most recent ones, with a counted remainder line.
pub const TASK_LANDED_CAP: usize = 50;

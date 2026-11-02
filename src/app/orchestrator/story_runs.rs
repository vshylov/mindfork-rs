//! A story run out in the background (docs/fabula.md §4, PR 8;
//! docs/research/fabula-engine.md §14, *The chat tools and the human in the
//! loop*): the third payload beside the sub-agent's and the dialogue's
//! ([`super::generation::RunSpec`]).
//!
//! It shares every later step with them — the seat, the landing by id, the
//! notification with its wake, `/subagents stop`, the cancel on a quit — and
//! takes a background slot as a dialogue does (docs/fabula.md §6, T7). What
//! differs is what runs and what lands:
//!
//! - **The engine runs, not a loop.** [`crate::features::story::run::run`]
//!   takes the story from where it stands to its next pause, on the chat's own
//!   backend under the app's session budget, within `tools.story_run_timeout_secs`
//!   and the chapter's budget.
//! - **The chapter lands as an attachment** (T3 (a)), one per chapter, so a
//!   revised version replaces the one before it; the assistant reads it in full
//!   or by reference, and the person opens it as any attachment.
//! - **The notification carries the pause** — the summary, the picks, the
//!   assumptions and the question — in the profile's language, under the prefix
//!   every task notification has: it is not a message from the user and grants
//!   nothing (spec §14, *The question*).
//! - **The record says only that the run happened and how it ended.** The
//!   story's state is its database; a run cut short by a quit names the step it
//!   was at, and the next run resumes from the first unfinished one.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::entities::attachment::{AttachMode, Attachment};
use crate::entities::message::Message;
use crate::entities::subagent::{RunKind, RunOutcome, SubagentRun};
use crate::features::story::board::StoryBoard;
use crate::features::story::chapter::Stop;
use crate::features::story::run::{Ending, Landed, StoryRun};
use crate::features::tools::ChatEffect;
use crate::features::tools::story as words;
use crate::shared::api::contract::EngineBackend;
use crate::shared::i18n::Locale;
use crate::shared::session_budget::SessionBudget;

/// A story run about to start: the run, what its record is called, and the
/// language its landing is told in.
pub(super) struct StorySpec {
    pub(super) run: StoryRun,
    /// The record's title and the transcript's name for the story.
    pub(super) title: String,
    /// What the run was asked to do, the record's first message.
    pub(super) brief: String,
    /// The profile's language: the model reads the landing (axis A).
    pub(super) loc: &'static Locale,
    pub(super) board: Arc<StoryBoard>,
    /// The run's own token: `/subagents stop`, the chat's deletion, a quit.
    pub(super) cancel: CancellationToken,
}

impl StorySpec {
    /// A run of `run`'s story over a fresh token.
    #[allow(dead_code)] // `story_continue` starts one (docs/fabula.md §4, PR 9)
    pub(super) fn new(
        run: StoryRun,
        title: String,
        brief: String,
        loc: &'static Locale,
        board: Arc<StoryBoard>,
    ) -> Self {
        Self {
            run,
            title,
            brief,
            loc,
            board,
            cancel: CancellationToken::new(),
        }
    }

    pub(super) fn run_id(&self) -> Uuid {
        self.run.run
    }

    /// The run as it will land, before it runs: its brief, `kind: Story`.
    pub(super) fn placeholder(&self) -> SubagentRun {
        SubagentRun {
            id: self.run.run,
            kind: RunKind::Story,
            title: self.title.clone(),
            renamed_manually: false,
            name: Some(self.title.clone()),
            created_at: chrono::Utc::now(),
            finished_at: None,
            system_message: String::new(),
            sampling_override: None,
            messages: vec![Message::user(self.brief.clone())],
            outcome: None,
            tokens: 0,
            participants: Vec::new(),
            background: true,
        }
    }

    /// Runs the story and builds its landing: the record filled in, the
    /// notification's body, and the chapter's attachment when one landed.
    pub(super) async fn run(
        self,
        backend: Arc<dyn EngineBackend>,
        sessions: Arc<SessionBudget>,
    ) -> (SubagentRun, String, Vec<ChatEffect>) {
        let landed = crate::features::story::run::run(
            &self.run,
            backend,
            Some(sessions),
            &self.board,
            self.cancel.clone(),
        )
        .await;
        self.land(&landed)
    }

    pub(super) fn land(&self, landed: &Landed) -> (SubagentRun, String, Vec<ChatEffect>) {
        let attachment = landed.chapter.as_ref().map(|chapter| {
            let name = words::attachment_name(self.loc, &self.run.story, chapter.number);
            let source = words::attachment_source(
                &self.run.chat.to_string(),
                &self.run.story,
                chapter.number,
            );
            // Inline for now: the landing decides against the chat's budget,
            // which only the orchestrator sees ([`decide_mode`]).
            Attachment::new(
                name,
                source,
                chapter.text.clone(),
                chapter.text.len(),
                AttachMode::Inline,
            )
        });
        let body = words::landing_text(
            self.loc,
            landed,
            attachment.as_ref().map(|a| a.name.as_str()),
        );
        let mut run = self.placeholder();
        run.outcome = Some(outcome_of(&landed.ending));
        run.finished_at = Some(chrono::Utc::now());
        run.tokens = landed.spent.output_tokens;
        run.messages.push(Message::assistant(body.clone()));
        let effects = attachment
            .map(|a| ChatEffect::AddAttachment(Box::new(a)))
            .into_iter()
            .collect();
        (run, body, effects)
    }
}

/// How the record reads a run's end. A spent budget reads as failed in the
/// list; the landing says what was spent.
fn outcome_of(ending: &Ending) -> RunOutcome {
    match ending {
        Ending::Paused(_) | Ending::Finished => RunOutcome::Completed,
        Ending::Stopped {
            why: Stop::Cancelled,
            ..
        } => RunOutcome::Cancelled,
        Ending::TimedOut { .. } => RunOutcome::TimedOut,
        Ending::Stopped { .. } | Ending::Failed(_) => RunOutcome::Failed,
    }
}

/// The mode a landed chapter reaches the model in: decided against what the
/// chat already carries inline, the chapter it replaces left out
/// (`attachment::decide_mode`, the rule `/file attach` follows).
pub(super) fn decide_mode(
    attachment: &mut Attachment,
    attached: &[Attachment],
    cfg: &crate::shared::config::AttachmentSettings,
) {
    let used = crate::entities::attachment::inline_tokens_excluding(attached, &attachment.source);
    attachment.mode = crate::entities::attachment::decide_mode(attachment.est_tokens, used, cfg);
}

/// The line a story run cut short by a quit leaves on its record, in the
/// profile's language: the step it was at, from the board.
pub(super) fn quit_line(loc: &'static Locale, board: &StoryBoard, run: Uuid) -> Message {
    let step = board.of_run(run).and_then(|view| view.step);
    Message::assistant(words::quit_text(loc, step.as_deref()))
}

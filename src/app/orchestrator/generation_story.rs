//! `story_continue` as the turn's loop runs it (docs/history/fabula.md §4, PR 9;
//! docs/research/fabula-engine.md §14, *The answer is the user's own words*).
//!
//! The loop intercepts the call, as it intercepts `start_dialogue`: the
//! answer to the story's question is read from the person's own rows, which
//! the turn snapshotted when it began, never from the call's arguments; the
//! run that follows goes out as the third background payload
//! ([`RunSpec::Story`]). A child of the generation module, so it reaches the
//! loop's own parts.
//!
//! The order keeps every refusal cheap and every start bounded: the slot is
//! taken before the reading's model call, so a capped background never pays
//! for a reading it cannot act on, and the story is claimed for the run
//! before its files are opened, so nothing writes it between the answer and
//! the run.

use uuid::Uuid;

use super::super::story_runs::StorySpec;
use super::{BackgroundStart, CallResult, RunSpec, SharedParts, TurnLoop, TurnProgress};
use crate::features::story::conversation::{self, Answering, Continued, Listener};
use crate::features::tools::story::STORY_CONTINUE_ID;
use crate::features::tools::story::conversation as words;
use crate::shared::api::contract::ApiToolCall;

impl TurnLoop<'_> {
    /// Whether a call of this round is `story_continue` the profile offers, in
    /// a round that is not being discarded.
    pub(super) fn is_story_continue(&self, call: &ApiToolCall, rewrite: bool) -> bool {
        call.name == STORY_CONTINUE_ID && !rewrite && self.allowed_has(&call.name)
    }

    /// Answers the story's pending question from the person's rows and starts
    /// its next run in the background; the call's result is the *started*
    /// line with the run's address, or why nothing started.
    pub(super) async fn resolve_story_continue(&mut self, call: &ApiToolCall) -> CallResult {
        self.announce_call(call);
        self.report_progress(Some(&call.name));
        let loc = self.ctx.loc;
        let args = match words::ContinueArgs::parse(&Self::call_args(call)) {
            Ok(args) => args,
            Err(err) => {
                return loc
                    .tf(
                        "loop.tool_error",
                        &[("name", STORY_CONTINUE_ID), ("err", &err)],
                    )
                    .into();
            }
        };
        if self.depth > 0 {
            return loc
                .tf("loop.tool_disabled", &[("name", STORY_CONTINUE_ID)])
                .into();
        }
        let id = args.story.as_str();
        if !words::has_story(&self.ctx, id) {
            return loc.tf("tool.story.result.unknown", &[("id", id)]).into();
        }
        if let Err(out) = self.shared.background.take() {
            return loc
                .tf(
                    "tool.start_subagent.result.too_many",
                    &[("n", &out.to_string())],
                )
                .into();
        }
        let run = Uuid::new_v4();
        let files = words::files_of(&self.ctx, id);
        let model = self.shared.model_name.clone().unwrap_or_default();
        let snapshot = self.ctx.story_pending.clone();
        let pending = snapshot.iter().find(|p| p.story == id);
        let prepared = conversation::prepare(
            &files,
            &self.ctx.story_board,
            (self.ctx.chat_id, id, run),
            Answering {
                pending,
                answering: self.ctx.user_times.answering.is_some(),
                notes: args.notes.as_deref(),
            },
            Listener {
                backend: self.shared.backend.clone(),
                model: model.clone(),
                sessions: Some(self.shared.sessions.clone()),
                cancel: self.cancel.child_token(),
                fallback_only: self.ctx.story_params.fallback_only,
            },
            self.ctx.story_params.budget(),
        )
        .await;
        let resolved = match prepared {
            Ok(Continued::Start(resolved)) => resolved,
            Ok(Continued::CanonChange(notes)) => {
                self.shared.background.release();
                return words::canon_change_text(loc, id, &notes).into();
            }
            Ok(Continued::Finished(chapter)) => {
                self.shared.background.release();
                return words::finished_text(loc, id, chapter).into();
            }
            Err(refusal) => {
                self.shared.background.release();
                return words::continue_refusal_text(loc, id, &refusal, pending).into();
            }
        };
        let story_run = self
            .ctx
            .story_params
            .run((self.ctx.chat_id, id, run), &files, &model);
        let spec = StorySpec::new(
            story_run,
            id.to_owned(),
            words::brief_text(loc, resolved.as_ref()),
            loc,
            self.ctx.story_board.clone(),
        );
        let placeholder = spec.placeholder();
        let text = words::started_text(
            loc,
            id,
            resolved.as_ref(),
            &crate::features::chat_links::uri(run),
        );
        self.progress(TurnProgress::BackgroundStart(Box::new(BackgroundStart {
            spec: RunSpec::Story(Box::new(spec)),
            parts: SharedParts::of(self.shared),
        })));
        CallResult {
            text,
            images: Vec::new(),
            subagent: Some(Box::new(placeholder)),
        }
    }
}

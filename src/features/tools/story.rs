//! The story tools (docs/fabula.md §3; docs/research/fabula-engine.md §14,
//! *Tools*): their schemas, descriptions and results, en and ru like every
//! tool's, and the words a story run's landing is told in. Offered only while
//! `tools.story` is on ([`ToolGate::Story`]). PR 8 built `story_status` and
//! the landing; PR 9 `story_create`, `story_continue` (a loop-executed tool,
//! like `start_dialogue`: the orchestrator reads the person's answer and
//! starts the run) and the request's reminder line.
//!
//! **Model-facing, so in the profile's language** (axis A): the status, the
//! notification and the question it carries are read by the assistant, which
//! puts the question to the user in its own words. The engine's own prompts
//! stay English (spec §15).

use std::path::Path;

use anyhow::Result;

use super::meta::{ToolGate, ToolGroup};
use super::{Tool, ToolContext, ToolOutcome};
use crate::entities::profile::ToolId;
use crate::entities::story::{Question, QuestionBody};
use crate::features::story::board::RunView;
use crate::features::story::chapter::Stop;
use crate::features::story::run::{Ending, Landed};
use crate::shared::i18n::Locale;
use crate::shared::storage::story::{Spend, StoryStatus, story_status_of};

pub mod conversation;

pub use conversation::{STORY_CONTINUE_ID, STORY_CREATE_ID, StoryContinueTool, StoryCreateTool};

/// `story_status`: where the chat's stories stand.
pub const STORY_STATUS_ID: &str = "story_status";

/// `story_status` (spec §14, *Tools*): synchronous and read-only, so "what is
/// my story doing" never gets a guess. Reports every story of the chat.
pub struct StoryStatusTool;

#[async_trait::async_trait]
impl Tool for StoryStatusTool {
    fn id(&self) -> ToolId {
        STORY_STATUS_ID.into()
    }
    fn group(&self) -> ToolGroup {
        ToolGroup::Subagent
    }
    fn ui_label(&self) -> &'static str {
        "story status"
    }
    fn gate(&self) -> Option<ToolGate> {
        Some(ToolGate::Story)
    }
    fn description(&self, loc: &Locale) -> String {
        loc.t("tool.story_status.desc").into()
    }
    fn parameters(&self, _loc: &Locale) -> serde_json::Value {
        super::introspection::empty_object()
    }
    async fn invoke(&self, ctx: &ToolContext, _args: serde_json::Value) -> Result<ToolOutcome> {
        let chat = ctx.chat_id.to_string();
        let storage = ctx.storage.clone();
        // The directory and the SQLite reads block; a read may wait out a
        // run's write under the busy timeout.
        let stories = tokio::task::spawn_blocking(move || {
            let json = storage.json();
            story_ids(&json.chat_stories_dir(&chat))
                .into_iter()
                .map(|id| {
                    let (db, log) = (json.story_db(&chat, &id), json.story_log(&chat, &id));
                    let status = story_status_of(&db, &log).map_err(|e| e.to_string());
                    (id, status)
                })
                .collect::<Vec<_>>()
        })
        .await?;
        let runs = ctx.story_board.of_chat(ctx.chat_id);
        Ok(ToolOutcome::text(report(ctx.loc, &stories, &runs)))
    }
}

/// The ids of the stories in a chat's directory, `<id>.db`, sorted.
pub fn story_ids(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "db"))
        .filter_map(|p| {
            p.file_stem()
                .and_then(std::ffi::OsStr::to_str)
                .map(str::to_owned)
        })
        .collect();
    ids.sort();
    ids
}

/// The status of `stories` with the runs out in their chat.
pub fn report(
    loc: &Locale,
    stories: &[(String, Result<StoryStatus, String>)],
    runs: &[RunView],
) -> String {
    if stories.is_empty() {
        return loc.t("tool.story_status.result.none").to_owned();
    }
    stories
        .iter()
        .map(|(id, status)| {
            let run = runs.iter().find(|r| &r.story == id);
            match status {
                Ok(status) => story_lines(loc, id, status, run),
                Err(err) => loc.tf(
                    "tool.story_status.result.unreadable",
                    &[("id", id), ("err", err)],
                ),
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn story_lines(loc: &Locale, id: &str, status: &StoryStatus, run: Option<&RunView>) -> String {
    let mut lines = Vec::new();
    match &status.premise {
        Some(premise) => lines.push(loc.tf(
            "tool.story_status.result.story",
            &[("id", id), ("premise", premise)],
        )),
        None => lines.push(loc.tf("tool.story_status.result.no_canon", &[("id", id)])),
    }
    if status.premise.is_some() && !status.canon_approved {
        lines.push(loc.t("tool.story_status.result.canon_pending").to_owned());
    }
    if status.planned > 0 {
        let accepted = status.chapters.iter().filter(|c| c.accepted).count();
        lines.push(loc.tf(
            "tool.story_status.result.chapters",
            &[
                ("accepted", &accepted.to_string()),
                ("planned", &status.planned.to_string()),
            ],
        ));
    }
    match status.open_chapter {
        Some(n) => {
            if let Some(open) = status.chapters.iter().find(|c| c.number == n) {
                lines.push(loc.tf(
                    "tool.story_status.result.drafted",
                    &[
                        ("n", &n.to_string()),
                        ("versions", &open.versions.to_string()),
                    ],
                ));
            }
            if status.spend.calls > 0 {
                lines.push(loc.tf(
                    "tool.story_status.result.spent",
                    &[
                        ("n", &n.to_string()),
                        ("tokens", &status.spend.tokens().to_string()),
                        ("cost", &cost(&status.spend)),
                    ],
                ));
            }
        }
        None if status.planned > 0 => {
            lines.push(loc.t("tool.story_status.result.finished").to_owned());
        }
        None => {}
    }
    lines.push(match run {
        Some(RunView {
            started_at,
            step: Some(step),
            ..
        }) => loc.tf(
            "tool.story_status.result.running",
            &[("since", &since(started_at)), ("step", step)],
        ),
        Some(RunView { started_at, .. }) => loc.tf(
            "tool.story_status.result.running_start",
            &[("since", &since(started_at))],
        ),
        None => loc.t("tool.story_status.result.idle").to_owned(),
    });
    if let Some(question) = &status.pending {
        lines.push(loc.tf(
            "tool.story_status.result.question",
            &[("question", &question_text(loc, &question.body))],
        ));
    }
    lines.join("\n")
}

fn since(at: &chrono::DateTime<chrono::Utc>) -> String {
    at.format("%Y-%m-%d %H:%M UTC").to_string()
}

/// `, $0.42` where the provider reported a cost, nothing elsewhere or for
/// nothing spent; below a cent, to the hundredth of one, so a run that spent a
/// little does not read as free.
fn cost(spend: &Spend) -> String {
    match spend.cost_usd.filter(|usd| *usd > 0.0) {
        Some(usd) if usd < 0.01 => format!(", ${usd:.4}"),
        Some(usd) => format!(", ${usd:.2}"),
        None => String::new(),
    }
}

/// What a pause asks, as the assistant reads it.
pub fn question_text(loc: &Locale, body: &QuestionBody) -> String {
    match body {
        QuestionBody::Canon => loc.t("tool.story.question.canon").to_owned(),
        QuestionBody::Chapter { chapter, version } => loc.tf(
            "tool.story.question.chapter",
            &[
                ("n", &chapter.to_string()),
                ("version", &version.to_string()),
            ],
        ),
        QuestionBody::Scene {
            chapter,
            scene,
            errors,
            ..
        } => loc.tf(
            "tool.story.question.scene",
            &[
                ("chapter", &chapter.to_string()),
                ("scene", &scene.to_string()),
                ("errors", &errors.join("; ")),
            ],
        ),
    }
}

/// The name a story's chapter is attached to its chat under.
pub fn attachment_name(loc: &Locale, story: &str, chapter: u32) -> String {
    loc.tf(
        "tool.story.attachment_name",
        &[("story", story), ("n", &chapter.to_string())],
    )
}

/// The source a chapter's attachment carries: one per chapter, so a revised
/// version replaces the one before it (re-attaching a source replaces it).
pub fn attachment_source(chat: &str, story: &str, chapter: u32) -> String {
    format!("story://{chat}/{story}/chapter/{chapter}")
}

/// The body of a run's notification (spec §14, *The question*): what the run
/// brought, what it costs, and what waits for the user. `attached` names the
/// chapter's attachment when the run landed one.
pub fn landing_text(loc: &Locale, landed: &Landed, attached: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let (Some(chapter), Some(file)) = (&landed.chapter, attached) {
        parts.push(loc.tf(
            "tool.story.landing.chapter",
            &[
                ("n", &chapter.number.to_string()),
                ("version", &chapter.version.to_string()),
                ("file", file),
            ],
        ));
        if let Some(summary) = chapter.summary.as_deref().filter(|s| !s.trim().is_empty()) {
            parts.push(loc.tf("tool.story.landing.summary", &[("summary", summary.trim())]));
        }
    }
    if !landed.picks.is_empty() {
        parts.push(listed(
            loc.t("tool.story.landing.picks"),
            landed.picks.iter().map(|p| p.trim().to_owned()),
        ));
    }
    if !landed.assumptions.is_empty() {
        parts.push(listed(
            loc.t("tool.story.landing.assumptions"),
            landed.assumptions.iter().map(|a| {
                loc.tf(
                    "tool.story.landing.assumption",
                    &[("doubt", a.doubt.trim()), ("choice", a.choice.trim())],
                )
            }),
        ));
    }
    parts.push(ending_text(loc, &landed.ending));
    if landed.spent.calls > 0 {
        parts.push(loc.tf(
            "tool.story.landing.spent",
            &[
                ("calls", &landed.spent.calls.to_string()),
                ("tokens", &landed.spent.tokens().to_string()),
                ("cost", &cost(&landed.spent)),
            ],
        ));
    }
    parts.join("\n\n")
}

fn listed(head: &str, items: impl Iterator<Item = String>) -> String {
    let mut out = head.to_owned();
    for item in items {
        out.push_str("\n- ");
        out.push_str(&item);
    }
    out
}

/// How the run ended, and what the user is asked.
fn ending_text(loc: &Locale, ending: &Ending) -> String {
    match ending {
        Ending::Paused(Question { body, .. }) => loc.tf(
            "tool.story.landing.question",
            &[("question", &question_text(loc, body))],
        ),
        Ending::Finished => loc.t("tool.story.landing.finished").to_owned(),
        Ending::Stopped {
            step,
            why: Stop::Cancelled,
        } => loc.tf("tool.story.landing.cancelled", &[("step", step)]),
        Ending::Stopped {
            step,
            why: Stop::Budget(spent),
        } => loc.tf(
            "tool.story.landing.budget",
            &[("step", step), ("spent", spent)],
        ),
        Ending::Stopped {
            step,
            why: Stop::Failed(err),
        } => loc.tf(
            "tool.story.landing.failed_step",
            &[("step", step), ("err", err)],
        ),
        Ending::TimedOut { step } => loc.tf("tool.story.landing.timed_out", &[("step", step)]),
        Ending::Failed(err) => loc.tf("tool.story.landing.failed", &[("err", err)]),
    }
}

/// The name of the human's language as the engine's English prompts put it
/// (`ChapterConfig::human_language`): what a story's summaries, picks and
/// questions are written in — the profile's (spec §15).
pub fn human_language(loc: &Locale) -> &str {
    loc.t("tool.story.language")
}

/// The line a run cut short by a quit leaves on its record: where it was.
pub fn quit_text(loc: &Locale, step: Option<&str>) -> String {
    match step {
        Some(step) => loc.tf("tool.story.quit", &[("step", step)]),
        None => loc.t("tool.story.quit_start").to_owned(),
    }
}

#[cfg(test)]
#[path = "story_tests.rs"]
mod tests;

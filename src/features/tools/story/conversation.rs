//! `story_create` and `story_continue` (docs/research/fabula-engine.md §14,
//! *Tools*; docs/history/fabula.md §4, PR 9): their schemas, descriptions and words,
//! en and ru like every tool's.
//!
//! - `story_create` is an ordinary, synchronous tool: the canon arrives as
//!   the call's arguments, the engine's checks refuse it with every problem
//!   named, and a canon that passes is stored as a draft with P1 open.
//! - `story_continue` is **loop-executed**, like `start_dialogue`: the
//!   orchestrator intercepts it, because the answer is read from the person's
//!   own messages, never from the arguments, and the run it starts goes out
//!   as a background payload. Its `invoke`, what a caller outside the loop
//!   gets, says so.

use anyhow::Result;
use serde_json::{Value, json};

use super::super::meta::{ToolGate, ToolGroup};
use super::super::{Tool, ToolContext, ToolOutcome};
use super::{question_text, story_ids};
use crate::entities::profile::ToolId;
use crate::entities::story::{Canon, Decision, QuestionBody};
use crate::features::story::conversation::{
    ContinueRefusal, CreateRefusal, PendingAnswer, Resolved, StoryFiles, create, story_id,
};
use crate::shared::i18n::Locale;

/// `story_create`: a story's canon, stored as a draft.
pub const STORY_CREATE_ID: &str = "story_create";

/// `story_continue`: the pending question answered from the person's words,
/// and the next run started.
pub const STORY_CONTINUE_ID: &str = "story_continue";

/// `story_create` — see the module doc.
pub struct StoryCreateTool;

/// `story_continue` — see the module doc.
pub struct StoryContinueTool;

/// What `story_create` was called with.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateArgs {
    /// The new story's title; ignored on a revision.
    pub title: Option<String>,
    /// The story whose draft canon this revises.
    pub story: Option<String>,
    /// The canon.
    pub canon: Canon,
}

/// A string argument, trimmed; `None` when absent or empty.
fn text_arg(args: &Value, name: &str) -> Option<String> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

impl CreateArgs {
    /// Reads the call's arguments. A canon sent as a JSON string, as some
    /// models send a nested object, is read as the object it holds.
    pub fn parse(args: &Value) -> Result<Self, String> {
        let mut canon = match args.get("canon") {
            Some(Value::String(s)) => {
                serde_json::from_str::<Value>(s).map_err(|e| e.to_string())?
            }
            Some(v @ Value::Object(_)) => v.clone(),
            Some(_) => return Err("`canon` must be an object".to_owned()),
            None => return Err("`canon` is missing".to_owned()),
        };
        // A title written inside the canon is the story's title: Qwen 3.8 put
        // it there (live, PR 9), and the canon itself has no such field.
        let inner = canon
            .as_object_mut()
            .and_then(|c| c.remove("title"))
            .and_then(|t| t.as_str().map(str::trim).map(str::to_owned))
            .filter(|t| !t.is_empty());
        let canon: Canon = serde_json::from_value(canon).map_err(|e| e.to_string())?;
        Ok(Self {
            title: text_arg(args, "title").or(inner),
            story: text_arg(args, "story"),
            canon,
        })
    }
}

/// What `story_continue` was called with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinueArgs {
    /// The story.
    pub story: String,
    /// The assistant's own notes for the Author.
    pub notes: Option<String>,
}

impl ContinueArgs {
    /// Reads the call's arguments: `story` is required.
    pub fn parse(args: &Value) -> Result<Self, String> {
        Ok(Self {
            story: text_arg(args, "story").ok_or_else(|| "`story` is missing".to_owned())?,
            notes: text_arg(args, "notes"),
        })
    }
}

/// Whether the chat `chat` has story `id`: the id must be one of the chat's
/// own files, so an argument never names a path of its choosing.
pub fn has_story(ctx: &ToolContext, id: &str) -> bool {
    let json = ctx.storage.json();
    story_ids(&json.chat_stories_dir(&ctx.chat_id.to_string()))
        .iter()
        .any(|s| s == id)
}

/// The files of story `id` of the turn's chat.
pub fn files_of(ctx: &ToolContext, id: &str) -> StoryFiles {
    let json = ctx.storage.json();
    let chat = ctx.chat_id.to_string();
    StoryFiles {
        db: json.story_db(&chat, id),
        log: json.story_log(&chat, id),
    }
}

#[async_trait::async_trait]
impl Tool for StoryCreateTool {
    fn id(&self) -> ToolId {
        STORY_CREATE_ID.into()
    }
    fn group(&self) -> ToolGroup {
        ToolGroup::Subagent
    }
    fn ui_label(&self) -> &'static str {
        "story create"
    }
    fn gate(&self) -> Option<ToolGate> {
        Some(ToolGate::Story)
    }
    fn description(&self, loc: &Locale) -> String {
        loc.t("tool.story_create.desc").into()
    }
    fn parameters(&self, loc: &Locale) -> Value {
        create_parameters(loc)
    }
    async fn invoke(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome> {
        let loc = ctx.loc;
        let parsed = match CreateArgs::parse(&args) {
            Ok(parsed) => parsed,
            Err(err) => {
                return Ok(ToolOutcome::text(
                    loc.tf("tool.story_create.result.unreadable", &[("err", &err)]),
                ));
            }
        };
        let (id, revise) = match (&parsed.story, &parsed.title) {
            (Some(id), _) if has_story(ctx, id) => (id.clone(), true),
            (Some(id), _) => {
                return Ok(ToolOutcome::text(
                    loc.tf("tool.story.result.unknown", &[("id", id)]),
                ));
            }
            (None, Some(title)) => {
                let json = ctx.storage.json();
                let chat = ctx.chat_id.to_string();
                let id = story_id(title, |id| {
                    json.story_db(&chat, id).exists() || json.story_log(&chat, id).exists()
                });
                (id, false)
            }
            (None, None) => {
                return Ok(ToolOutcome::text(
                    loc.t("tool.story_create.result.no_title"),
                ));
            }
        };
        let files = files_of(ctx, &id);
        let created = create(
            &files,
            &ctx.story_board,
            (ctx.chat_id, &id),
            &parsed.canon,
            revise,
        )
        .await;
        Ok(ToolOutcome::text(match created {
            Ok(_) if revise => loc.tf("tool.story_create.result.revised", &[("id", &id)]),
            Ok(_) => loc.tf(
                "tool.story_create.result.created",
                &[
                    ("title", parsed.title.as_deref().unwrap_or(&id)),
                    ("id", &id),
                ],
            ),
            Err(refusal) => create_refusal_text(loc, &id, &refusal, revise),
        }))
    }
}

#[async_trait::async_trait]
impl Tool for StoryContinueTool {
    fn id(&self) -> ToolId {
        STORY_CONTINUE_ID.into()
    }
    fn group(&self) -> ToolGroup {
        ToolGroup::Subagent
    }
    fn ui_label(&self) -> &'static str {
        "story continue"
    }
    fn gate(&self) -> Option<ToolGate> {
        Some(ToolGate::Story)
    }
    fn description(&self, loc: &Locale) -> String {
        loc.t("tool.story_continue.desc").into()
    }
    fn parameters(&self, loc: &Locale) -> Value {
        json!({
            "type": "object",
            "properties": {
                "story": {"type": "string", "description": loc.t("tool.story_continue.param.story")},
                "notes": {"type": "string", "description": loc.t("tool.story_continue.param.notes")}
            },
            "required": ["story"]
        })
    }
    /// Never the executor — the loop reads the answer and starts the run (see
    /// the module doc).
    async fn invoke(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome> {
        ContinueArgs::parse(&args).map_err(anyhow::Error::msg)?;
        Ok(ToolOutcome::text(
            ctx.loc.t("tool.story_continue.result.loop_only"),
        ))
    }
}

/// `story_create`'s parameters: the title, the story revised, and the canon in
/// the shape [`Canon`] reads, every field described for the model.
fn create_parameters(loc: &Locale) -> Value {
    let d = |key: &str| loc.t(key).to_owned();
    let text = |key: &str| json!({"type": "string", "description": d(key)});
    let texts =
        |key: &str| json!({"type": "array", "items": {"type": "string"}, "description": d(key)});
    json!({
        "type": "object",
        "properties": {
            "title": text("tool.story_create.param.title"),
            "story": text("tool.story_create.param.story"),
            "canon": {
                "type": "object",
                "description": d("tool.story_create.param.canon"),
                "properties": {
                    "world": {
                        "type": "object",
                        "properties": {"rules": texts("tool.story_create.param.rules")},
                        "required": ["rules"]
                    },
                    "locations": {
                        "type": "array",
                        "description": d("tool.story_create.param.locations"),
                        "items": {
                            "type": "object",
                            "properties": {
                                "key": text("tool.story_create.param.location_key"),
                                "name": text("tool.story_create.param.location_name"),
                                "description": text("tool.story_create.param.location_description"),
                                "routes": {
                                    "type": "array",
                                    "description": d("tool.story_create.param.routes"),
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "to": text("tool.story_create.param.route_to"),
                                            "ticks": {"type": "integer", "description": d("tool.story_create.param.route_ticks")}
                                        },
                                        "required": ["to", "ticks"]
                                    }
                                },
                                "items": texts("tool.story_create.param.location_items")
                            },
                            "required": ["key", "name"]
                        }
                    },
                    "characters": {
                        "type": "array",
                        "description": d("tool.story_create.param.characters"),
                        "items": {
                            "type": "object",
                            "properties": {
                                "key": text("tool.story_create.param.character_key"),
                                "name": text("tool.story_create.param.character_name"),
                                "public": text("tool.story_create.param.character_public"),
                                "voice": text("tool.story_create.param.character_voice"),
                                "goals": texts("tool.story_create.param.character_goals"),
                                "secrets": {
                                    "type": "array",
                                    "description": d("tool.story_create.param.secrets"),
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "key": text("tool.story_create.param.secret_key"),
                                            "text": text("tool.story_create.param.secret_text")
                                        },
                                        "required": ["key", "text"]
                                    }
                                },
                                "start": {
                                    "type": "object",
                                    "properties": {
                                        "location": text("tool.story_create.param.start_location"),
                                        "holds": texts("tool.story_create.param.start_holds")
                                    },
                                    "required": ["location"]
                                }
                            },
                            "required": ["key", "public", "start"]
                        }
                    },
                    "style": {
                        "type": "object",
                        "properties": {
                            "pov": text("tool.story_create.param.pov"),
                            "tense": text("tool.story_create.param.tense"),
                            "language": text("tool.story_create.param.language")
                        }
                    },
                    "arc": {
                        "type": "object",
                        "properties": {
                            "premise": text("tool.story_create.param.premise"),
                            "chapters": {
                                "type": "array",
                                "description": d("tool.story_create.param.chapters"),
                                "items": {
                                    "type": "object",
                                    "properties": {"goal": text("tool.story_create.param.chapter_goal")},
                                    "required": ["goal"]
                                }
                            }
                        },
                        "required": ["premise", "chapters"]
                    }
                },
                "required": ["world", "locations", "characters", "arc"]
            }
        },
        "required": ["canon"]
    })
}

/// Why `story_create` stored nothing, for the model. `revise` says whether
/// the call named a story: a new story's refusal says it has no id yet, so the
/// next call is not taken for a revision (live, PR 9).
pub fn create_refusal_text(
    loc: &Locale,
    id: &str,
    refusal: &CreateRefusal,
    revise: bool,
) -> String {
    match refusal {
        CreateRefusal::Problems(problems) => {
            let mut list = String::new();
            for p in problems {
                list.push_str("\n- ");
                list.push_str(p);
            }
            if revise {
                loc.tf(
                    "tool.story_create.result.problems_revise",
                    &[("problems", &list), ("id", id)],
                )
            } else {
                loc.tf("tool.story_create.result.problems", &[("problems", &list)])
            }
        }
        CreateRefusal::Unknown => loc.tf("tool.story.result.unknown", &[("id", id)]),
        CreateRefusal::Approved => loc.tf("tool.story_create.result.approved", &[("id", id)]),
        CreateRefusal::Busy(_) => loc.tf("tool.story.result.busy", &[("id", id)]),
        CreateRefusal::Failed(err) => {
            loc.tf("tool.story.result.failed", &[("id", id), ("err", err)])
        }
    }
}

/// What a started run goes on with, in the started line.
fn resolved_text(loc: &Locale, resolved: Option<&Resolved>) -> String {
    let Some(resolved) = resolved else {
        return loc.t("tool.story_continue.resolved.resume").to_owned();
    };
    match (&resolved.body, resolved.decision) {
        (QuestionBody::Canon, _) => loc.t("tool.story_continue.resolved.canon").to_owned(),
        (QuestionBody::Chapter { chapter, .. }, Decision::Approve) => loc.tf(
            "tool.story_continue.resolved.accepted",
            &[("n", &chapter.to_string())],
        ),
        (QuestionBody::Chapter { chapter, .. }, Decision::Change) => loc.tf(
            "tool.story_continue.resolved.revise",
            &[("n", &chapter.to_string())],
        ),
    }
}

/// `story_continue`'s result when the run started: what the person decided,
/// and the run's address.
pub fn started_text(loc: &Locale, id: &str, resolved: Option<&Resolved>, address: &str) -> String {
    loc.tf(
        "tool.story_continue.result.started",
        &[
            ("id", id),
            ("what", &resolved_text(loc, resolved)),
            ("address", address),
        ],
    )
}

/// What the run's record is asked, its first message: the same words.
pub fn brief_text(loc: &Locale, resolved: Option<&Resolved>) -> String {
    resolved_text(loc, resolved)
}

/// `story_continue`'s result when the person accepted the last chapter.
pub fn finished_text(loc: &Locale, id: &str, chapter: u32) -> String {
    loc.tf(
        "tool.story_continue.result.done",
        &[("id", id), ("n", &chapter.to_string())],
    )
}

/// `story_continue`'s result when the person asked for the canon to change.
pub fn canon_change_text(loc: &Locale, id: &str, notes: &str) -> String {
    loc.tf(
        "tool.story_continue.result.canon_change",
        &[("id", id), ("changes", notes.trim())],
    )
}

/// Why `story_continue` started nothing, for the model; `pending` is the
/// question that waits, when the turn knows it.
pub fn continue_refusal_text(
    loc: &Locale,
    id: &str,
    refusal: &ContinueRefusal,
    pending: Option<&PendingAnswer>,
) -> String {
    let question = || pending.map_or_else(String::new, |p| question_text(loc, &p.question.body));
    match refusal {
        ContinueRefusal::Unknown => loc.tf("tool.story.result.unknown", &[("id", id)]),
        ContinueRefusal::Busy(_) => loc.tf("tool.story.result.busy", &[("id", id)]),
        ContinueRefusal::Finished => loc.tf("tool.story_continue.result.finished", &[("id", id)]),
        ContinueRefusal::BudgetSpent { chapter, spent } => loc.tf(
            "tool.story_continue.result.budget",
            &[("id", id), ("n", &chapter.to_string()), ("spent", spent)],
        ),
        ContinueRefusal::NotAsked => loc.tf("tool.story_continue.result.not_asked", &[("id", id)]),
        ContinueRefusal::NotAnswered => loc.tf(
            "tool.story_continue.result.not_answered",
            &[("id", id), ("question", &question())],
        ),
        ContinueRefusal::Unclear(_) => loc.tf(
            "tool.story_continue.result.unclear",
            &[("id", id), ("question", &question())],
        ),
        ContinueRefusal::Failed(err) => {
            loc.tf("tool.story.result.failed", &[("id", id), ("err", err)])
        }
    }
}

/// The line every request carries while a story's question waits (spec §14,
/// *The question*): at the request's tail, so a reply that answers it meets
/// it in the same turn; `None` when nothing waits.
pub fn reminder(loc: &Locale, pending: &[PendingAnswer]) -> Option<String> {
    let lines: Vec<String> = pending
        .iter()
        .map(|p| {
            loc.tf(
                "tool.story.reminder",
                &[
                    ("id", &p.story),
                    ("question", &question_text(loc, &p.question.body)),
                ],
            )
        })
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

#[cfg(test)]
#[path = "conversation_tests.rs"]
mod tests;

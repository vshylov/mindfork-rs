//! `run_dialogue` (spec §9.13): two personas with caller-written system
//! messages talk **to each other** — each sees the other's lines as `user`
//! turns — while a model-driven director steers the scene and decides when it
//! is over (docs/research/two-agent-dialogue.md; the seams were left by the
//! sub-agent track, research §3.14).
//!
//! Like `call_subagent`, this is a **loop-executed tool**: the agentic loop
//! recognises the name and runs the dialogue itself
//! (`TurnLoop::run_dialogue`); the `Tool` impl below exists for the schema,
//! the catalog and the profile toggle. This module also owns the pure halves
//! the loop calls: the request **derivation** (the role swap, the `user`
//! prologue, the same-role merge — research §3.2) and the director's verdict
//! vocabulary (research §3.4), so both are unit-testable without an engine.

use anyhow::Result;

use crate::entities::message::{Message, MessageRole};
use crate::entities::profile::ToolId;
use crate::shared::api::contract::{ApiMessage, ApiToolCall, ToolSchema};
use crate::shared::i18n::Locale;

use super::{Tool, ToolContext, ToolOutcome};

/// The tool's name — what the loop recognises.
pub const RUN_DIALOGUE_ID: &str = "run_dialogue";

/// The schema-level ceiling on `max_messages` — a backstop far above any
/// dialogue a turn should stage, not a working value.
pub const MAX_MESSAGES_CAP: usize = 64;
/// Default `max_messages` when the call names none.
pub const DEFAULT_MAX_MESSAGES: usize = 16;
/// Default checkpoint cadence (`moderate_every`): after every exchange.
pub const DEFAULT_MODERATE_EVERY: usize = 2;
/// The cadence's schema ceiling.
pub const MODERATE_EVERY_CAP: usize = 8;

/// One persona of the call, as parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialoguePersona {
    pub name: Option<String>,
    pub system_message: String,
}

/// The parsed arguments of one `run_dialogue` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogueArgs {
    pub a: DialoguePersona,
    pub b: DialoguePersona,
    /// Who speaks the caller-authored opening line (fork F7).
    pub opening_by_a: bool,
    pub opening: String,
    /// A shared setting both personas see as their `user` prologue (§3.2).
    pub scene: Option<String>,
    /// The director's brief — on top of the parent persona and the
    /// conversation brief it carries anyway (fork F6).
    pub direction: Option<String>,
    pub max_messages: usize,
    pub moderate_every: usize,
}

fn persona_of(args: &serde_json::Value, key: &str) -> Option<DialoguePersona> {
    let obj = args.get(key)?;
    let system_message = obj
        .get("system_message")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_string();
    let name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Some(DialoguePersona {
        name,
        system_message,
    })
}

impl DialogueArgs {
    /// Reads the call's arguments. Missing personas or an empty opening are
    /// usage errors — the tool was called wrong, and saying so is what lets
    /// the next call succeed.
    pub fn parse(args: &serde_json::Value, loc: &Locale) -> Result<Self> {
        let a = persona_of(args, "a");
        let b = persona_of(args, "b");
        let (Some(a), Some(b)) = (a, b) else {
            return Err(anyhow::anyhow!(
                loc.t("tool.run_dialogue.err.personas").to_string()
            ));
        };
        let opening_obj = args.get("opening");
        let opening = opening_obj
            .and_then(|o| o.get("text"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!(loc.t("tool.run_dialogue.err.opening").to_string()))?
            .to_string();
        let opening_by_a = opening_obj
            .and_then(|o| o.get("speaker"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            != Some("b");
        let text_of = |key: &str| {
            args.get(key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let clamped = |key: &str, default: usize, lo: usize, hi: usize| {
            args.get(key)
                .and_then(serde_json::Value::as_u64)
                .map_or(default, |v| (v as usize).clamp(lo, hi))
        };
        Ok(Self {
            a,
            b,
            opening_by_a,
            opening,
            scene: text_of("scene"),
            direction: text_of("direction"),
            max_messages: clamped("max_messages", DEFAULT_MAX_MESSAGES, 2, MAX_MESSAGES_CAP),
            moderate_every: clamped(
                "moderate_every",
                DEFAULT_MODERATE_EVERY,
                1,
                MODERATE_EVERY_CAP,
            ),
        })
    }

    /// A participant's display label: the given name, else the localized
    /// fallback. `loc` is whichever axis the label is for (the transcript's
    /// headers resolve through the UI locale, the director's script through
    /// the run's).
    pub fn label(&self, speaker_a: bool, loc: &Locale) -> String {
        let (persona, key) = if speaker_a {
            (&self.a, "tool.run_dialogue.fallback_a")
        } else {
            (&self.b, "tool.run_dialogue.fallback_b")
        };
        persona
            .name
            .clone()
            .unwrap_or_else(|| loc.t(key).to_string())
    }

    /// The run's initial title: `A ↔ B` from the labels — something to stand
    /// on before a person or the model names it (spec §11.2).
    pub fn initial_title(&self, loc: &Locale) -> String {
        let title = format!("{} ↔ {}", self.label(true, loc), self.label(false, loc));
        crate::shared::title::sanitize_title(&title).unwrap_or_else(|| RUN_DIALOGUE_ID.to_string())
    }
}

// ---------------------------------------------------------------------------
// Derivation (research §3.2)

/// The localized fixtures every derived view shares (axis A): the caller's
/// shared scene, the prologue fallback, and the note prefix.
pub struct ViewText<'a> {
    pub scene: Option<&'a str>,
    pub begins: &'a str,
    pub note_prefix: &'a str,
}

/// Builds one participant's request view over the role-encoded transcript
/// (`a` = `Assistant`, `b` = `User` — research §3.5): own lines stay/become
/// `assistant`, the other's `user`; director interventions (`System` entries)
/// are excluded — they reach a participant only as the system appendix below;
/// a `user` **prologue** (the scene, else `begins`) is prepended when the
/// scene is set or the first mapped line would be `assistant`; and adjacent
/// same-role entries are **merged**, so every view is strictly alternating —
/// what the strictest chat template (Gemma's jinja) requires, measured in the
/// stage-0 probe (research §5.1).
pub fn participant_view(
    transcript: &[Message],
    speaker_a: bool,
    persona_system: &str,
    notes: &[String],
    one_shot_note: Option<&str>,
    text: &ViewText<'_>,
) -> (String, Vec<ApiMessage>) {
    let mut system = persona_system.to_string();
    for note in notes.iter().map(String::as_str).chain(one_shot_note) {
        system.push_str("\n\n");
        system.push_str(text.note_prefix);
        system.push(' ');
        system.push_str(note);
    }

    let lines = transcript.iter().filter(|m| m.role != MessageRole::System);
    let mut mapped: Vec<(bool, &str)> = Vec::new(); // (is_own, text)
    for m in lines {
        let by_a = m.role == MessageRole::Assistant;
        mapped.push((by_a == speaker_a, &m.text));
    }
    let mut messages: Vec<ApiMessage> = Vec::new();
    if text.scene.is_some() || mapped.first().is_some_and(|(own, _)| *own) {
        messages.push(ApiMessage::user(text.scene.unwrap_or(text.begins)));
    }
    for (own, text) in mapped {
        let (role_matches, make): (bool, fn(&str) -> ApiMessage) = if own {
            (
                messages
                    .last()
                    .is_some_and(|l| l.role == crate::shared::api::contract::ApiRole::Assistant),
                |t| ApiMessage::assistant(t),
            )
        } else {
            (
                messages
                    .last()
                    .is_some_and(|l| l.role == crate::shared::api::contract::ApiRole::User),
                |t| ApiMessage::user(t),
            )
        };
        match messages.last_mut() {
            Some(last) if role_matches => {
                last.content.push_str("\n\n");
                last.content.push_str(text);
            }
            _ => messages.push(make(text)),
        }
    }
    (system, messages)
}

/// Who speaks next: the other side of the last spoken line (director
/// interventions don't take turns). `None` — the transcript has no spoken
/// line yet, which cannot happen after the required opening.
pub fn next_speaker_a(transcript: &[Message]) -> Option<bool> {
    transcript
        .iter()
        .rev()
        .find(|m| m.role != MessageRole::System)
        .map(|m| m.role != MessageRole::Assistant)
}

// ---------------------------------------------------------------------------
// The director's verdict vocabulary (research §3.4)

/// One parsed director action, applied in call order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Continue,
    Stop {
        reason: String,
        summary: Option<String>,
    },
    Note {
        to_a: bool,
        to_b: bool,
        text: String,
    },
    Retry {
        note: Option<String>,
    },
    Rewrite {
        text: String,
    },
}

/// Parses the checkpoint reply's calls into verdicts, in order. Returns the
/// verdicts and how many calls were unknown names (counted with the prose
/// fallback — research §3.3).
/// The director's verdict as its own turn in its persistent conversation
/// (docs/research/dialogue-director-history.md §3.1): the model's text, if
/// any, then each call as `name(arguments)` on its own line — the fact the
/// tool-call turn carried, as text, so the history alternates on a template
/// without a tool role. Gemma 3's rendered the `tool` result that followed a
/// tool-call turn as a second user turn in a row and refused the pair; the
/// clouds' wires merge such pairs, which is why they never saw it. Empty
/// arguments (`{}`) are left out of the parentheses.
pub fn verdict_turn(text: &str, calls: &[ApiToolCall]) -> String {
    let mut out = text.trim().to_string();
    for call in calls {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&call.name);
        out.push('(');
        let args = call.arguments.trim();
        if args != "{}" {
            out.push_str(args);
        }
        out.push(')');
    }
    out
}

pub fn parse_verdicts(calls: &[ApiToolCall]) -> (Vec<Verdict>, usize) {
    let mut verdicts = Vec::new();
    let mut unknown = 0;
    for call in calls {
        let args: serde_json::Value =
            serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);
        let text_of = |key: &str| args.get(key).and_then(|v| v.as_str()).map(str::to_string);
        match call.name.as_str() {
            "dialogue_continue" => verdicts.push(Verdict::Continue),
            "dialogue_stop" => verdicts.push(Verdict::Stop {
                reason: text_of("reason").unwrap_or_default(),
                summary: text_of("summary").filter(|s| !s.trim().is_empty()),
            }),
            "dialogue_note" => {
                let to = text_of("to").unwrap_or_else(|| "both".into());
                let text = text_of("text").unwrap_or_default();
                if !text.trim().is_empty() {
                    verdicts.push(Verdict::Note {
                        to_a: to != "b",
                        to_b: to != "a",
                        text,
                    });
                }
            }
            "dialogue_retry" => verdicts.push(Verdict::Retry {
                note: text_of("note").filter(|s| !s.trim().is_empty()),
            }),
            "dialogue_rewrite" => {
                let text = text_of("text").unwrap_or_default();
                if !text.trim().is_empty() {
                    verdicts.push(Verdict::Rewrite { text });
                }
            }
            _ => unknown += 1,
        }
    }
    (verdicts, unknown)
}

/// The verdict tools' schemas, sent only inside checkpoint requests — they
/// are the director's vocabulary, not part of the profile catalog.
pub fn verdict_tools(loc: &Locale, a_label: &str, b_label: &str) -> Vec<ToolSchema> {
    let obj = |props: serde_json::Value, required: &[&str]| serde_json::json!({ "type": "object", "properties": props, "required": required });
    vec![
        ToolSchema {
            name: "dialogue_continue".into(),
            description: loc.t("tool.run_dialogue.verdict.continue").into(),
            parameters: obj(serde_json::json!({}), &[]),
        },
        ToolSchema {
            name: "dialogue_stop".into(),
            description: loc.t("tool.run_dialogue.verdict.stop").into(),
            parameters: obj(
                serde_json::json!({
                    "reason": { "type": "string", "description": loc.t("tool.run_dialogue.verdict.stop_reason") },
                    "summary": { "type": "string", "description": loc.t("tool.run_dialogue.verdict.stop_summary") }
                }),
                &["reason"],
            ),
        },
        ToolSchema {
            name: "dialogue_note".into(),
            description: loc.t("tool.run_dialogue.verdict.note").into(),
            parameters: obj(
                serde_json::json!({
                    "to": {
                        "type": "string", "enum": ["a", "b", "both"],
                        "description": loc.tf("tool.run_dialogue.verdict.note_to", &[("a", a_label), ("b", b_label)])
                    },
                    "text": { "type": "string" }
                }),
                &["to", "text"],
            ),
        },
        ToolSchema {
            name: "dialogue_retry".into(),
            description: loc.t("tool.run_dialogue.verdict.retry").into(),
            parameters: obj(
                serde_json::json!({ "note": { "type": "string", "description": loc.t("tool.run_dialogue.verdict.retry_note") } }),
                &[],
            ),
        },
        ToolSchema {
            name: "dialogue_rewrite".into(),
            description: loc.t("tool.run_dialogue.verdict.rewrite").into(),
            parameters: obj(
                serde_json::json!({ "text": { "type": "string" } }),
                &["text"],
            ),
        },
    ]
}

/// The **background** twin (spec §9.13, docs/research/background-dialogues.md
/// §4.1): the same scene, the same arguments, but the call returns at once
/// with the transcript's address and the director's closing result arrives
/// later as a task notification. Offered only when `tools.subagent_background`
/// is on — the one switch for "a run that outlives the turn" (fork F2). A
/// second tool rather than a flag for the reason the sub-agent twin measured:
/// an optional boolean is silently omitted by one provider
/// (background-subagents.md §3.1).
pub const START_DIALOGUE_ID: &str = "start_dialogue";

/// `run_dialogue` — see the module doc.
pub struct RunDialogue;

/// `start_dialogue` — the background twin (see [`START_DIALOGUE_ID`]). Like
/// its foreground sibling a **loop-executed** tool: this impl carries the
/// schema, the catalog entry and the profile toggle, and its `invoke` — what
/// a caller outside the loop gets — says so rather than running a scene.
pub struct StartDialogue;

#[async_trait::async_trait]
impl Tool for StartDialogue {
    fn id(&self) -> ToolId {
        START_DIALOGUE_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Subagent
    }
    fn gate(&self) -> Option<crate::features::tools::meta::ToolGate> {
        Some(crate::features::tools::meta::ToolGate::Background)
    }
    fn ui_label(&self) -> &'static str {
        "background dialogue"
    }
    fn description(&self, loc: &Locale) -> String {
        loc.t("tool.start_dialogue.desc").into()
    }
    fn parameters(&self, loc: &Locale) -> serde_json::Value {
        RunDialogue.parameters(loc)
    }
    /// Never the executor — the loop runs the scene (see the module doc).
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        DialogueArgs::parse(&args, ctx.loc)?;
        Ok(ToolOutcome::text(
            ctx.loc.t("tool.run_dialogue.result.loop_only"),
        ))
    }
}

#[async_trait::async_trait]
impl Tool for RunDialogue {
    fn id(&self) -> ToolId {
        RUN_DIALOGUE_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Subagent
    }
    fn ui_label(&self) -> &'static str {
        "dialogue run"
    }
    fn description(&self, loc: &Locale) -> String {
        loc.t("tool.run_dialogue.desc").into()
    }
    fn parameters(&self, loc: &Locale) -> serde_json::Value {
        let persona = |desc: &str| {
            serde_json::json!({
                "type": "object",
                "description": desc,
                "properties": {
                    "name": { "type": "string", "description": loc.t("tool.run_dialogue.param.persona_name") },
                    "system_message": { "type": "string", "description": loc.t("tool.run_dialogue.param.persona_system") }
                },
                "required": ["system_message"]
            })
        };
        serde_json::json!({
            "type": "object",
            "properties": {
                "a": persona(loc.t("tool.run_dialogue.param.a")),
                "b": persona(loc.t("tool.run_dialogue.param.b")),
                "opening": {
                    "type": "object",
                    "description": loc.t("tool.run_dialogue.param.opening"),
                    "properties": {
                        "speaker": { "type": "string", "enum": ["a", "b"], "description": loc.t("tool.run_dialogue.param.opening_speaker") },
                        "text": { "type": "string" }
                    },
                    "required": ["text"]
                },
                "scene": { "type": "string", "description": loc.t("tool.run_dialogue.param.scene") },
                "direction": { "type": "string", "description": loc.t("tool.run_dialogue.param.direction") },
                "max_messages": {
                    "type": "integer", "minimum": 2, "maximum": MAX_MESSAGES_CAP,
                    "description": loc.t("tool.run_dialogue.param.max_messages")
                },
                "moderate_every": {
                    "type": "integer", "minimum": 1, "maximum": MODERATE_EVERY_CAP,
                    "description": loc.t("tool.run_dialogue.param.moderate_every")
                }
            },
            "required": ["a", "b", "opening"]
        })
    }
    /// Never the executor — the loop runs the dialogue (see the module doc).
    /// Validates the arguments and then says so.
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        DialogueArgs::parse(&args, ctx.loc)?;
        Ok(ToolOutcome::text(
            ctx.loc.t("tool.run_dialogue.result.loop_only"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};

    fn en() -> &'static Locale {
        locale(Lang::En)
    }

    fn args(json: serde_json::Value) -> DialogueArgs {
        DialogueArgs::parse(&json, en()).unwrap()
    }

    fn full_args() -> DialogueArgs {
        args(serde_json::json!({
            "a": { "name": "Mara", "system_message": "barista" },
            "b": { "name": "Jonas", "system_message": "customer" },
            "opening": { "speaker": "b", "text": "Wrong drink?" },
            "scene": "A café.",
            "direction": "Stop at the goodbye.",
            "max_messages": 10,
            "moderate_every": 3
        }))
    }

    #[test]
    fn args_parse_reads_the_full_shape() {
        let a = full_args();
        assert_eq!(a.a.name.as_deref(), Some("Mara"));
        assert_eq!(a.b.system_message, "customer");
        assert!(!a.opening_by_a);
        assert_eq!(a.opening, "Wrong drink?");
        assert_eq!(a.scene.as_deref(), Some("A café."));
        assert_eq!(a.max_messages, 10);
        assert_eq!(a.moderate_every, 3);
    }

    #[test]
    fn args_defaults_and_clamps() {
        let a = args(serde_json::json!({
            "a": { "system_message": "x" },
            "b": { "system_message": "y" },
            "opening": { "text": "hi" },
            "max_messages": 1000,
            "moderate_every": 0
        }));
        assert!(a.opening_by_a, "speaker defaults to a");
        assert_eq!(a.max_messages, MAX_MESSAGES_CAP);
        assert_eq!(a.moderate_every, 1);
        assert_eq!(a.scene, None);
        let d = args(serde_json::json!({
            "a": { "system_message": "x" },
            "b": { "system_message": "y" },
            "opening": { "text": "hi" }
        }));
        assert_eq!(d.max_messages, DEFAULT_MAX_MESSAGES);
        assert_eq!(d.moderate_every, DEFAULT_MODERATE_EVERY);
    }

    #[test]
    fn args_reject_missing_personas_and_empty_opening() {
        assert!(
            DialogueArgs::parse(
                &serde_json::json!({ "a": { "system_message": "x" }, "opening": { "text": "hi" } }),
                en()
            )
            .is_err()
        );
        assert!(
            DialogueArgs::parse(
                &serde_json::json!({
                    "a": { "system_message": "x" }, "b": { "system_message": "y" },
                    "opening": { "text": "  " }
                }),
                en()
            )
            .is_err()
        );
    }

    #[test]
    fn labels_fall_back_localized_and_title_joins_them() {
        let a = args(serde_json::json!({
            "a": { "system_message": "x" },
            "b": { "name": "Bob", "system_message": "y" },
            "opening": { "text": "hi" }
        }));
        assert_eq!(a.label(true, en()), en().t("tool.run_dialogue.fallback_a"));
        assert_eq!(a.label(false, en()), "Bob");
        assert!(a.initial_title(en()).contains(" ↔ Bob"));
    }

    // -- derivation ---------------------------------------------------------

    fn line(by_a: bool, text: &str) -> Message {
        if by_a {
            Message::assistant(text)
        } else {
            Message::user(text)
        }
    }

    fn roles(messages: &[ApiMessage]) -> Vec<crate::shared::api::contract::ApiRole> {
        messages.iter().map(|m| m.role).collect()
    }

    /// The opener's own view starts `user` (the prologue), the other side's
    /// starts with the opener's line merged after the scene — strict
    /// alternation on both, which is what Gemma's template requires.
    #[test]
    fn views_alternate_strictly_from_either_side() {
        use crate::shared::api::contract::ApiRole::{Assistant, User};
        let transcript = vec![line(false, "b1"), line(true, "a1"), line(false, "b2")];
        let scene = ViewText {
            scene: Some("scene"),
            begins: "(b)",
            note_prefix: "Note:",
        };
        let (_, for_a) = participant_view(&transcript, true, "sys", &[], None, &scene);
        assert_eq!(roles(&for_a), vec![User, Assistant, User]);
        assert_eq!(for_a[0].content, "scene\n\nb1");
        let (_, for_b) = participant_view(&transcript, false, "sys", &[], None, &scene);
        assert_eq!(roles(&for_b), vec![User, Assistant, User, Assistant]);
        assert_eq!(for_b[0].content, "scene");
        assert_eq!(for_b[1].content, "b1");
    }

    /// Without a scene the prologue appears only where the first mapped line
    /// would otherwise be `assistant` — the opener's own view. It stays a
    /// `user` turn of its own: the opener's first line is assistant-side and
    /// never merges into it.
    #[test]
    fn prologue_only_where_needed_without_a_scene() {
        use crate::shared::api::contract::ApiRole::{Assistant, User};
        let transcript = vec![line(false, "b1"), line(true, "a1")];
        let (_, for_b) = participant_view(
            &transcript,
            false,
            "sys",
            &[],
            None,
            &ViewText {
                scene: None,
                begins: "(b)",
                note_prefix: "N:",
            },
        );
        assert_eq!(roles(&for_b), vec![User, Assistant, User]);
        assert_eq!(for_b[0].content, "(b)");
        assert_eq!(for_b[1].content, "b1");
        let (_, for_a) = participant_view(
            &transcript,
            true,
            "sys",
            &[],
            None,
            &ViewText {
                scene: None,
                begins: "(b)",
                note_prefix: "N:",
            },
        );
        assert_eq!(
            for_a[0].content, "b1",
            "no prologue when user-first already"
        );
    }

    /// Director interventions never reach a participant's message list; notes
    /// land in the system appendix, one-shot notes after the standing ones.
    #[test]
    fn system_entries_excluded_and_notes_appended() {
        let transcript = vec![
            line(false, "b1"),
            Message::new(MessageRole::System, "Director: wrap up"),
            line(true, "a1"),
        ];
        let notes = vec!["stay polite".to_string()];
        let (system, messages) = participant_view(
            &transcript,
            true,
            "sys",
            &notes,
            Some("one shot"),
            &ViewText {
                scene: None,
                begins: "(b)",
                note_prefix: "Director's note:",
            },
        );
        assert!(!messages.iter().any(|m| m.content.contains("wrap up")));
        let polite = system.find("stay polite").unwrap();
        let shot = system.find("one shot").unwrap();
        assert!(system.starts_with("sys"));
        assert!(polite < shot);
    }

    #[test]
    fn next_speaker_alternates_and_skips_interventions() {
        let mut transcript = vec![line(false, "b1")];
        assert_eq!(next_speaker_a(&transcript), Some(true));
        transcript.push(Message::new(MessageRole::System, "note"));
        assert_eq!(
            next_speaker_a(&transcript),
            Some(true),
            "system has no turn"
        );
        transcript.push(line(true, "a1"));
        assert_eq!(next_speaker_a(&transcript), Some(false));
    }

    // -- verdicts -----------------------------------------------------------

    fn call(name: &str, args: serde_json::Value) -> ApiToolCall {
        ApiToolCall {
            id: "c1".into(),
            name: name.into(),
            arguments: args.to_string(),
            thought_signature: None,
        }
    }

    #[test]
    fn verdicts_parse_in_order_and_count_unknowns() {
        let calls = vec![
            call(
                "dialogue_note",
                serde_json::json!({"to": "a", "text": "cut it"}),
            ),
            call("dialogue_continue", serde_json::json!({})),
            call("made_up", serde_json::json!({})),
        ];
        let (verdicts, unknown) = parse_verdicts(&calls);
        assert_eq!(unknown, 1);
        assert_eq!(
            verdicts,
            vec![
                Verdict::Note {
                    to_a: true,
                    to_b: false,
                    text: "cut it".into()
                },
                Verdict::Continue,
            ]
        );
    }

    #[test]
    fn stop_carries_reason_and_optional_summary() {
        let (v, _) = parse_verdicts(&[call(
            "dialogue_stop",
            serde_json::json!({"reason": "done", "summary": "  "}),
        )]);
        assert_eq!(
            v,
            vec![Verdict::Stop {
                reason: "done".into(),
                summary: None
            }]
        );
        let (v, _) = parse_verdicts(&[call(
            "dialogue_stop",
            serde_json::json!({"reason": "done", "summary": "they agreed"}),
        )]);
        assert!(matches!(&v[0], Verdict::Stop { summary: Some(s), .. } if s == "they agreed"));
    }

    /// An empty note or rewrite is dropped rather than applied as nothing —
    /// a director that calls with no text has said nothing.
    #[test]
    fn empty_note_and_rewrite_are_dropped() {
        let (v, unknown) = parse_verdicts(&[
            call(
                "dialogue_note",
                serde_json::json!({"to": "both", "text": " "}),
            ),
            call("dialogue_rewrite", serde_json::json!({"text": ""})),
        ]);
        assert!(v.is_empty());
        assert_eq!(unknown, 0);
    }

    #[test]
    fn verdict_tools_name_the_participants_in_the_note_schema() {
        let tools = verdict_tools(en(), "Mara", "Jonas");
        assert_eq!(tools.len(), 5);
        let note = tools.iter().find(|t| t.name == "dialogue_note").unwrap();
        let desc = note.parameters["properties"]["to"]["description"]
            .as_str()
            .unwrap();
        assert!(desc.contains("Mara") && desc.contains("Jonas"), "{desc}");
    }

    /// Outside the loop the tool runs nothing and says so; a malformed call
    /// is still a hard error, as for every tool.
    #[tokio::test]
    async fn invoke_outside_the_loop_refuses_without_running() {
        let (_dir, _storage, ctx) = super::super::testkit::ctx_with_storage(uuid::Uuid::new_v4());
        let ok = serde_json::json!({
            "a": { "system_message": "x" }, "b": { "system_message": "y" },
            "opening": { "text": "hi" }
        });
        let out = RunDialogue.invoke(&ctx, ok).await.unwrap();
        assert_eq!(out.result, ctx.loc.t("tool.run_dialogue.result.loop_only"));
        assert!(out.effects.is_empty());
        assert!(
            RunDialogue
                .invoke(&ctx, serde_json::json!({ "opening": { "text": "hi" } }))
                .await
                .is_err()
        );
    }

    /// The verdict turn (docs/research/dialogue-director-history.md §3.1):
    /// a call with arguments, two calls, text with a call, text alone, and
    /// nothing at all.
    #[test]
    fn verdict_turn_renders_the_calls_as_the_directors_own_words() {
        assert_eq!(
            verdict_turn("", &[call("dialogue_continue", serde_json::json!({}))]),
            "dialogue_continue()"
        );
        assert_eq!(
            verdict_turn(
                "",
                &[
                    ApiToolCall {
                        id: "c1".into(),
                        name: "dialogue_note".into(),
                        arguments: r#"{"to":"a","text":"wrap up"}"#.into(),
                        thought_signature: None,
                    },
                    call("dialogue_continue", serde_json::json!({})),
                ]
            ),
            "dialogue_note({\"to\":\"a\",\"text\":\"wrap up\"})\ndialogue_continue()"
        );
        assert_eq!(
            verdict_turn(
                "Nearly there. ",
                &[call("dialogue_continue", serde_json::json!({}))]
            ),
            "Nearly there.\ndialogue_continue()"
        );
        assert_eq!(verdict_turn("dialogue_continue", &[]), "dialogue_continue");
        assert_eq!(verdict_turn("  ", &[]), "");
    }
}

//! `call_subagent` (spec §9.3.2): delegate a task to a **sub-agent** — the same
//! model under a system message the main agent composes, with the main agent's
//! own tools (minus `call_subagent` itself, the history read-back pair and the
//! self-model family), no history of this chat, and its own round and time
//! budgets. The run's transcript is kept on the call's record
//! ([`crate::entities::subagent::SubagentRun`]) and shows in the chat list as a
//! child of this chat (docs/research/subagent-chats.md).
//!
//! Like the conversation-control tools (`control.rs`), this is a **loop-executed
//! tool**: the agentic loop (`app/orchestrator/generation.rs`) recognises the
//! name and runs a nested loop itself — the registry, the confirmation channel
//! and the UI sender a tool-using sub-agent needs live there, not in
//! [`ToolContext`]. The `Tool` impl below exists for the schema, the catalog and
//! the profile toggle; its `invoke` is what a caller *outside* the loop gets
//! (the silent background loops never offer it), and it says so rather than
//! running anything.

use anyhow::Result;

use crate::entities::profile::ToolId;

use super::{Tool, ToolContext, ToolOutcome};

/// The tool's name — what the loop recognises.
pub const CALL_SUBAGENT_ID: &str = "call_subagent";

/// The **background** twin (spec §9.3.2, docs/research/background-subagents.md
/// §4.1): the same delegation, but the call returns at once with the run's
/// address and the run outlives the turn — its result arrives later as a task
/// notification. Offered only when `tools.subagent_background` is on. A
/// second tool rather than a flag on the first because a flag is silently
/// omitted by one provider (research §3.1: 0/3 on Claude), where a tool of
/// its own is used 3/3 on every cloud and 5/5 locally.
pub const START_SUBAGENT_ID: &str = "start_subagent";

/// Whether a tool of the turn is **withheld** from a sub-agent
/// (docs/research/subagent-chats.md §3.3): the nested-run family itself (no
/// nesting — `run_dialogue` and the background twin included, spec §9.13),
/// the read-back pair over the *parent's* folded history, and the self-model
/// family — the profile persona's identity, which a persona the parent
/// composed must not write into. Everything else the turn offers, the
/// sub-agent gets.
pub fn withheld_from_subagent(id: &str) -> bool {
    id == CALL_SUBAGENT_ID
        || id == START_SUBAGENT_ID
        || id == super::dialogue::RUN_DIALOGUE_ID
        || id == super::history::HISTORY_READ_ID
        || id == super::history::HISTORY_SEARCH_ID
        || super::self_model::is_self_model_tool(id)
}

/// The parsed arguments of one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentArgs {
    /// The persona's display name, when the caller gave one.
    pub name: Option<String>,
    /// The sub-agent's system message (may be empty — then the persona is the
    /// instruction alone).
    pub system_message: String,
    /// The one user message the sub-agent is asked.
    pub message: String,
}

impl SubagentArgs {
    /// Reads the call's arguments. An empty `message` is a usage error rather
    /// than an empty run: the tool was called wrong, and saying so is what lets
    /// the next call succeed.
    pub fn parse(args: &serde_json::Value, loc: &crate::shared::i18n::Locale) -> Result<Self> {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let system_message = args
            .get("system_message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(loc.t("tool.call_subagent.err.message_empty").to_string())
            })?
            .to_string();
        Ok(Self {
            name,
            system_message,
            message,
        })
    }

    /// The run's initial title: the persona's name, else the first line of the
    /// instruction (spec §11.2) — something to stand on before a person or the
    /// model names it.
    pub fn initial_title(&self) -> String {
        self.name
            .as_deref()
            .and_then(crate::shared::title::sanitize_title)
            .or_else(|| {
                self.message
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .and_then(crate::shared::title::sanitize_title)
            })
            .unwrap_or_else(|| CALL_SUBAGENT_ID.to_string())
    }
}

/// `call_subagent` — see the module doc.
pub struct CallSubagent {
    /// `tools.subagent_parallel`: how many of one reply's sub-agents run at
    /// once. Above 1 the description tells the model so, with the number
    /// (docs/research/parallel-subagents.md §4.6, fork F5 — measured to
    /// move a small model from three two-call replies in five to five); at 1
    /// the description is byte-identical to what it was. The registry is
    /// rebuilt on every settings edit, so the number is never stale.
    pub parallel: u32,
}

#[async_trait::async_trait]
impl Tool for CallSubagent {
    fn id(&self) -> ToolId {
        CALL_SUBAGENT_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Subagent
    }
    fn ui_label(&self) -> &'static str {
        "subagent request"
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        let base = loc.t("tool.call_subagent.desc");
        if self.parallel > 1 {
            format!(
                "{base} {}",
                loc.tf(
                    "tool.call_subagent.desc.parallel",
                    &[("n", &self.parallel.to_string())]
                )
            )
        } else {
            base.into()
        }
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        subagent_parameters(loc)
    }
    /// Never the executor — see the module doc. Validates the arguments (so a
    /// malformed call is refused the same way everywhere) and then says the run
    /// is only available inside a chat turn.
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        SubagentArgs::parse(&args, ctx.loc)?;
        Ok(ToolOutcome::text(
            ctx.loc.t("tool.call_subagent.result.loop_only"),
        ))
    }
}

/// The schema both delegation tools share: a persona (`name`,
/// `system_message`) and the one message. The background twin takes exactly
/// the same call — what differs is when the answer comes.
fn subagent_parameters(loc: &crate::shared::i18n::Locale) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": loc.t("tool.call_subagent.param.name")
            },
            "system_message": {
                "type": "string",
                "description": loc.t("tool.call_subagent.param.system_message")
            },
            "message": {
                "type": "string",
                "description": loc.t("tool.call_subagent.param.message")
            }
        },
        "required": ["system_message", "message"]
    })
}

/// `start_subagent` — the background delegation (see [`START_SUBAGENT_ID`]).
/// Registered only when `tools.subagent_background` is on, so the catalog a
/// turn offers at the default is byte-identical to what it was. Like its
/// twin, a loop-executed tool: `invoke` is what a caller outside the loop
/// gets.
pub struct StartSubagent;

#[async_trait::async_trait]
impl Tool for StartSubagent {
    fn id(&self) -> ToolId {
        START_SUBAGENT_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Subagent
    }
    fn ui_label(&self) -> &'static str {
        "background subagent"
    }
    fn gate(&self) -> Option<crate::features::tools::meta::ToolGate> {
        Some(crate::features::tools::meta::ToolGate::Background)
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.start_subagent.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        subagent_parameters(loc)
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        SubagentArgs::parse(&args, ctx.loc)?;
        Ok(ToolOutcome::text(
            ctx.loc.t("tool.call_subagent.result.loop_only"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};

    fn en() -> &'static crate::shared::i18n::Locale {
        locale(Lang::En)
    }

    #[test]
    fn args_parse_trims_and_reads_the_optional_name() {
        let a = SubagentArgs::parse(
            &serde_json::json!({"name": " Critic ", "system_message": " be harsh ", "message": " rate X "}),
            en(),
        )
        .unwrap();
        assert_eq!(a.name.as_deref(), Some("Critic"));
        assert_eq!(a.system_message, "be harsh");
        assert_eq!(a.message, "rate X");
        let b = SubagentArgs::parse(&serde_json::json!({"message": "x"}), en()).unwrap();
        assert_eq!(b.name, None);
        assert_eq!(b.system_message, "");
    }

    #[test]
    fn args_reject_an_empty_message() {
        assert!(
            SubagentArgs::parse(
                &serde_json::json!({"system_message": "x", "message": "  "}),
                en()
            )
            .is_err()
        );
        assert!(SubagentArgs::parse(&serde_json::json!({"system_message": "x"}), en()).is_err());
    }

    #[test]
    fn initial_title_prefers_the_name_then_the_first_line() {
        let named = SubagentArgs {
            name: Some("Critic".into()),
            system_message: String::new(),
            message: "rate X\nin detail".into(),
        };
        assert_eq!(named.initial_title(), "Critic");
        let unnamed = SubagentArgs {
            name: None,
            ..named
        };
        assert_eq!(unnamed.initial_title(), "rate X");
    }

    /// Outside the loop the tool runs nothing and says so; a malformed call is
    /// still a hard error, as for every tool.
    #[tokio::test]
    async fn invoke_outside_the_loop_refuses_without_running() {
        let (_dir, _storage, ctx) = super::super::testkit::ctx_with_storage(uuid::Uuid::new_v4());
        let out = CallSubagent { parallel: 1 }
            .invoke(
                &ctx,
                serde_json::json!({"system_message": "x", "message": "y"}),
            )
            .await
            .unwrap();
        assert_eq!(out.result, ctx.loc.t("tool.call_subagent.result.loop_only"));
        assert!(out.effects.is_empty());
        assert!(
            CallSubagent { parallel: 1 }
                .invoke(
                    &ctx,
                    serde_json::json!({"system_message": "x", "message": " "})
                )
                .await
                .is_err()
        );
    }
    /// The description carries the parallel contract only above one, with
    /// the number (docs/research/parallel-subagents.md fork F5): at one it
    /// is byte-identical to the base text, in both languages.
    #[test]
    fn the_description_names_parallel_delegation_only_above_one() {
        let base = CallSubagent { parallel: 1 }.description(en());
        assert_eq!(base, en().t("tool.call_subagent.desc"));
        let three = CallSubagent { parallel: 3 }.description(en());
        assert!(three.starts_with(&base) && three.contains('3'), "{three}");
        assert!(three.len() > base.len());
        let ru = locale(Lang::Ru);
        assert_eq!(
            CallSubagent { parallel: 1 }.description(ru),
            ru.t("tool.call_subagent.desc")
        );
        assert!(CallSubagent { parallel: 2 }.description(ru).contains('2'));
    }

    /// The background twin (docs/research/background-subagents.md §4.1): the
    /// same parameters as `call_subagent`, its own description, the
    /// `Background` gate, and withheld from a sub-agent like the rest of the
    /// nested-run family.
    #[test]
    fn start_subagent_is_the_twin_with_its_own_gate() {
        use crate::features::tools::meta::ToolGate;
        assert_eq!(StartSubagent.id(), START_SUBAGENT_ID);
        assert_eq!(
            StartSubagent.parameters(en()),
            CallSubagent { parallel: 1 }.parameters(en())
        );
        assert_eq!(
            StartSubagent.description(en()),
            en().t("tool.start_subagent.desc")
        );
        assert_eq!(StartSubagent.gate(), Some(ToolGate::Background));
        assert!(withheld_from_subagent(START_SUBAGENT_ID));
        assert!(!StartSubagent.danger());
    }
}

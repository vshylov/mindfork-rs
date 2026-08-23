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

/// Whether a tool of the turn is **withheld** from a sub-agent
/// (docs/research/subagent-chats.md §3.3): the tool itself (no nesting), the
/// read-back pair over the *parent's* folded history, and the self-model
/// family — the profile persona's identity, which a persona the parent
/// composed must not write into. Everything else the turn offers, the
/// sub-agent gets.
pub fn withheld_from_subagent(id: &str) -> bool {
    id == CALL_SUBAGENT_ID
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
pub struct CallSubagent;

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
        loc.t("tool.call_subagent.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
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
        let out = CallSubagent
            .invoke(
                &ctx,
                serde_json::json!({"system_message": "x", "message": "y"}),
            )
            .await
            .unwrap();
        assert_eq!(out.result, ctx.loc.t("tool.call_subagent.result.loop_only"));
        assert!(out.effects.is_empty());
        assert!(
            CallSubagent
                .invoke(
                    &ctx,
                    serde_json::json!({"system_message": "x", "message": " "})
                )
                .await
                .is_err()
        );
    }
}

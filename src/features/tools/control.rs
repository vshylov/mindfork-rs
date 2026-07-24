//! Conversation control tools: "write another message"
//! ([`SendFollowupMessage`]) and "rewrite the message" ([`RewriteCurrentMessage`]).
//! See spec §9.3.
//!
//! Unlike regular tools (which return a text result and don't touch the
//! conversation's structure), these are **control-flow**: the orchestrator's
//! agentic loop itself recognizes them (`app/orchestrator/generation.rs`),
//! not `Tool::invoke`. The `Tool` implementations only exist for the schema/
//! description/registration/gating; `invoke` returns the same "permission"
//! text that the loop synthesizes (in case of a direct call — normally the
//! loop intercepts them by name).
//!
//! Both tools are **optional** and off by default: they're absent from
//! [`super::default_tool_ids`], only in [`super::all_tool_ids`] (the profile
//! toggle catalog). The user enables them in the profile's settings.

use anyhow::Result;

use super::{Tool, ToolContext, ToolOutcome};
use crate::entities::profile::ToolId;

/// The name of the "write another message" tool.
pub const SEND_FOLLOWUP_ID: &str = "send_followup_message";
/// The name of the "rewrite my own message" tool.
///
/// The name is `rewrite_current_message` (not `..._last_message`): the model
/// anchors on the function name, and it read "last" as "the last message in
/// history" (the user's turn or its own previous reply) — hence refusals and a
/// "gaslighting" reading. "current" unambiguously points to the message the
/// assistant is writing **in the current turn**.
pub const REWRITE_CURRENT_ID: &str = "rewrite_current_message";

/// Whether the tool is a control tool (intercepted by the agentic loop).
pub fn is_control_tool(name: &str) -> bool {
    name == SEND_FOLLOWUP_ID || name == REWRITE_CURRENT_ID
}

/// The "permission" text the loop places as the result of a control-tool call
/// (seen by the model in the next round's history). In the scaffold language `loc`.
pub fn control_permission_text(name: &str, loc: &crate::shared::i18n::Locale) -> String {
    match name {
        SEND_FOLLOWUP_ID => loc.t("control.permission.followup").to_string(),
        REWRITE_CURRENT_ID => loc.t("control.permission.rewrite").to_string(),
        _ => String::new(),
    }
}

/// "Write another message": the assistant can finish the current message,
/// then continue with a second reply (shown as a separate bubble right after
/// the first). See spec §9.3.
pub struct SendFollowupMessage;

#[async_trait::async_trait]
impl Tool for SendFollowupMessage {
    fn id(&self) -> ToolId {
        SEND_FOLLOWUP_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Conversation
    }
    fn ui_label(&self) -> &'static str {
        "continue message"
    }
    fn enabled_by_default(&self) -> bool {
        false
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.send_followup_message.desc").into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    async fn invoke(&self, ctx: &ToolContext, _args: serde_json::Value) -> Result<ToolOutcome> {
        Ok(ToolOutcome::text(control_permission_text(
            SEND_FOLLOWUP_ID,
            ctx.loc,
        )))
    }
}

/// "Rewrite my current message": if partway through writing the assistant
/// realizes the answer is wrong, it calls this tool — the current message is
/// discarded right away, and the assistant writes it from scratch. The
/// previous version is hidden and doesn't take part in further inference.
/// See spec §9.3.
pub struct RewriteCurrentMessage;

#[async_trait::async_trait]
impl Tool for RewriteCurrentMessage {
    fn id(&self) -> ToolId {
        REWRITE_CURRENT_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Conversation
    }
    fn ui_label(&self) -> &'static str {
        "rewrite reply"
    }
    fn enabled_by_default(&self) -> bool {
        false
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.rewrite_current_message.desc").into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    async fn invoke(&self, ctx: &ToolContext, _args: serde_json::Value) -> Result<ToolOutcome> {
        Ok(ToolOutcome::text(control_permission_text(
            REWRITE_CURRENT_ID,
            ctx.loc,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_tool_names_recognized() {
        assert!(is_control_tool(SEND_FOLLOWUP_ID));
        assert!(is_control_tool(REWRITE_CURRENT_ID));
        assert!(!is_control_tool("note_save"));
    }

    #[test]
    fn permission_text_per_tool() {
        let ru = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
        assert!(!control_permission_text(SEND_FOLLOWUP_ID, ru).is_empty());
        assert!(!control_permission_text(REWRITE_CURRENT_ID, ru).is_empty());
        assert!(control_permission_text("note_save", ru).is_empty());
    }
}

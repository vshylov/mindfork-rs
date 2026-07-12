//! Управляющие инструменты беседы: «написать ещё сообщение»
//! ([`SendFollowupMessage`]) и «переписать сообщение» ([`RewriteCurrentMessage`]).
//! См. spec §9.3.
//!
//! В отличие от обычных инструментов (возвращают текстовый результат и не
//! трогают структуру беседы), эти — **управляющие** (control-flow): их
//! распознаёт сам agentic-loop оркестратора (`app/orchestrator/generation.rs`),
//! а не `Tool::invoke`. Реализации `Tool` нужны лишь для схемы/описания/
//! регистрации/гейтинга; `invoke` возвращает то же «разрешение», что синтезирует
//! петля (на случай прямого вызова — но в норме петля перехватывает их по имени).
//!
//! Оба инструмента **опциональны** и по умолчанию выключены: их нет в
//! [`super::default_tool_ids`], только в [`super::all_tool_ids`] (каталог тумблеров
//! профиля). Пользователь включает их в настройках профиля.

use anyhow::Result;

use super::{Tool, ToolContext, ToolOutcome};
use crate::entities::profile::ToolId;

/// Имя инструмента «написать ещё сообщение».
pub const SEND_FOLLOWUP_ID: &str = "send_followup_message";
/// Имя инструмента «переписать своё сообщение».
///
/// Имя — `rewrite_current_message` (а не `..._last_message`): модель якорится на
/// имя функции, и «last» она трактовала как «последнее сообщение в истории»
/// (реплику пользователя или свой прошлый ответ) — отсюда отказы и трактовка
/// «газлайтинг». «current» однозначно указывает на сообщение, которое ассистент
/// пишет **в текущем ходе**.
pub const REWRITE_CURRENT_ID: &str = "rewrite_current_message";

/// Является ли инструмент управляющим (перехватывается agentic-loop'ом).
pub fn is_control_tool(name: &str) -> bool {
    name == SEND_FOLLOWUP_ID || name == REWRITE_CURRENT_ID
}

/// Текст «разрешения», который петля кладёт как результат вызова управляющего
/// инструмента (его видит модель в истории следующего раунда). На языке каркаса `loc`.
pub fn control_permission_text(name: &str, loc: &crate::shared::i18n::Locale) -> String {
    match name {
        SEND_FOLLOWUP_ID => loc.t("control.permission.followup").to_string(),
        REWRITE_CURRENT_ID => loc.t("control.permission.rewrite").to_string(),
        _ => String::new(),
    }
}

/// «Написать ещё сообщение»: ассистент может дописать текущее сообщение до конца,
/// затем продолжить второй репликой (показывается отдельным пузырём сразу за
/// первым). См. spec §9.3.
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
        "дописать сообщение"
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

/// «Переписать своё текущее сообщение»: если по ходу написания ассистент понял,
/// что ответ неверный, он вызывает этот инструмент — текущее сообщение сразу
/// обрывается, а ассистент пишет его заново. Прежняя версия скрывается и не
/// участвует в дальнейшем инференсе. См. spec §9.3.
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
        "переписать ответ"
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

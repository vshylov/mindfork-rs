//! Управляющие инструменты беседы: «написать ещё сообщение»
//! ([`SendFollowupMessage`]) и «переписать сообщение» ([`RewriteLastMessage`]).
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
pub const REWRITE_MESSAGE_ID: &str = "rewrite_last_message";

/// Является ли инструмент управляющим (перехватывается agentic-loop'ом).
pub fn is_control_tool(name: &str) -> bool {
    name == SEND_FOLLOWUP_ID || name == REWRITE_MESSAGE_ID
}

/// Текст «разрешения», который петля кладёт как результат вызова управляющего
/// инструмента (его видит модель в истории следующего раунда).
pub fn control_permission_text(name: &str) -> &'static str {
    match name {
        SEND_FOLLOWUP_ID => {
            "Хорошо. Напиши следующее сообщение — оно будет показано отдельной репликой."
        }
        REWRITE_MESSAGE_ID => "Хорошо. Напиши сообщение заново — предыдущая версия будет скрыта.",
        _ => "",
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
    fn description(&self) -> String {
        "Разрешает написать ещё одно сообщение сразу после текущего (отдельной \
         репликой). Сначала допиши текущее сообщение до конца, затем вызови этот \
         инструмент — после него можно написать вторую реплику."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    async fn invoke(&self, _ctx: &ToolContext, _args: serde_json::Value) -> Result<ToolOutcome> {
        Ok(ToolOutcome::text(control_permission_text(SEND_FOLLOWUP_ID)))
    }
}

/// «Переписать своё сообщение»: если по ходу написания ассистент понял, что
/// ответ неверный, он вызывает этот инструмент — текущее сообщение сразу
/// обрывается, а ассистент пишет его заново. Прежняя версия скрывается и не
/// участвует в дальнейшем инференсе. См. spec §9.3.
pub struct RewriteLastMessage;

#[async_trait::async_trait]
impl Tool for RewriteLastMessage {
    fn id(&self) -> ToolId {
        REWRITE_MESSAGE_ID.into()
    }
    fn description(&self) -> String {
        "Отменяет текущее сообщение и позволяет написать его заново. Вызови, если \
         понял, что начал отвечать неправильно: текущий текст будет отброшен, а \
         следующая реплика заменит его."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    async fn invoke(&self, _ctx: &ToolContext, _args: serde_json::Value) -> Result<ToolOutcome> {
        Ok(ToolOutcome::text(control_permission_text(
            REWRITE_MESSAGE_ID,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_tool_names_recognized() {
        assert!(is_control_tool(SEND_FOLLOWUP_ID));
        assert!(is_control_tool(REWRITE_MESSAGE_ID));
        assert!(!is_control_tool("note_save"));
    }

    #[test]
    fn permission_text_per_tool() {
        assert!(!control_permission_text(SEND_FOLLOWUP_ID).is_empty());
        assert!(!control_permission_text(REWRITE_MESSAGE_ID).is_empty());
        assert!(control_permission_text("note_save").is_empty());
    }
}

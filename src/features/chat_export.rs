//! Экспорт переписки чата в простой текст для копирования в буфер обмена.
//! Чистая, тестируемая без UI логика (вызывается оркестратором). См. spec §11.2.

use crate::entities::message::{Message, MessageRole};
use crate::shared::config::CopySettings;
use crate::shared::i18n::Locale;

/// Форматирует всю переписку чата в читаемый текст для буфера обмена: помечает
/// роли (`Пользователь`/`Ассистент`), сохраняет многострочный текст, пропускает
/// системные/инструментальные сообщения. Непустой `title` идёт заголовком.
///
/// По умолчанию (`CopySettings` все флаги `false`) копируется только текст сообщений.
/// Опционально (`opts`) к блоку ассистента добавляются: «мысли» (CoT, перед текстом),
/// параметры вызовов инструментов (имя + аргументы) и их результаты — берутся из
/// `Message.tool_calls` (отдельные tool-сообщения по-прежнему пропускаются, чтобы не
/// дублировать). Возвращает `None`, если содержательных блоков нет (копировать нечего).
pub fn format_conversation(
    title: &str,
    messages: &[Message],
    opts: &CopySettings,
    loc: &'static Locale,
) -> Option<String> {
    let mut blocks: Vec<String> = Vec::new();

    let title = title.trim();
    if !title.is_empty() {
        blocks.push(title.to_string());
    }

    for m in messages {
        match m.role {
            MessageRole::User => {
                let text = m.text.trim();
                if !text.is_empty() {
                    blocks.push(format!("{}\n{text}", loc.t("ui.export.user")));
                }
            }
            MessageRole::Assistant => {
                if let Some(block) = format_assistant(m, opts, loc) {
                    blocks.push(block);
                }
            }
            // Системные и инструментальные сообщения в переписку не входят
            // (результаты инструментов берутся из `tool_calls` ассистента).
            MessageRole::System | MessageRole::Tool => continue,
        }
    }

    // Только заголовок (или вовсе ничего) — содержательной переписки нет.
    let has_messages = blocks.len() > usize::from(!title.is_empty());
    if !has_messages {
        return None;
    }
    Some(blocks.join("\n\n"))
}

/// Собирает блок одного сообщения ассистента: опциональные «мысли», текст и
/// опциональные tool-блоки. `None`, если после фильтрации блок пуст.
fn format_assistant(m: &Message, opts: &CopySettings, loc: &'static Locale) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();

    if opts.copy_thoughts
        && let Some(thoughts) = &m.thoughts
    {
        let thoughts = thoughts.trim();
        if !thoughts.is_empty() {
            parts.push(format!("{}\n{thoughts}", loc.t("ui.export.thoughts")));
        }
    }

    let text = m.text.trim();
    if !text.is_empty() {
        parts.push(text.to_string());
    }

    if opts.copy_tool_calls || opts.copy_tool_results {
        for tc in &m.tool_calls {
            let mut lines = vec![loc.tf("ui.export.tool", &[("name", &tc.name)])];
            if opts.copy_tool_calls {
                let args = serde_json::to_string(&tc.arguments).unwrap_or_default();
                lines.push(loc.tf("ui.export.args", &[("args", &args)]));
            }
            if opts.copy_tool_results {
                let result = tc.result.as_deref().unwrap_or("").trim();
                lines.push(loc.tf("ui.export.result", &[("result", result)]));
            }
            parts.push(lines.join("\n"));
        }
    }

    if parts.is_empty() {
        return None;
    }
    Some(format!(
        "{}\n{}",
        loc.t("ui.export.assistant"),
        parts.join("\n\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::message::ToolCallRecord;

    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    /// Копирование только текста (поведение по умолчанию).
    fn plain() -> CopySettings {
        CopySettings::default()
    }

    /// Сообщение ассистента с «мыслями» и одним вызовом инструмента (имя/аргументы/
    /// результат) — для проверки опциональных блоков.
    fn assistant_with_tool() -> Message {
        let mut m = Message::assistant("ответ");
        m.thoughts = Some("я думаю".into());
        m.tool_calls = vec![ToolCallRecord {
            thought_signature: None,
            id: "c1".into(),
            name: "note_save".into(),
            arguments: serde_json::json!({"text": "заметка"}),
            result: Some("сохранено".into()),
        }];
        m
    }

    #[test]
    fn formats_roles_with_title_and_skips_system_and_tools() {
        let msgs = vec![
            Message::new(MessageRole::System, "ты ассистент"),
            Message::user("как дела?"),
            Message::assistant("хорошо"),
            Message::new(MessageRole::Tool, "tool output"),
        ];
        let out = format_conversation("Мой чат", &msgs, &plain(), ru()).unwrap();
        assert_eq!(
            out,
            "Мой чат\n\nПользователь:\nкак дела?\n\nАссистент:\nхорошо"
        );
    }

    #[test]
    fn empty_title_is_omitted() {
        let msgs = vec![Message::user("привет")];
        let out = format_conversation("   ", &msgs, &plain(), ru()).unwrap();
        assert_eq!(out, "Пользователь:\nпривет");
    }

    #[test]
    fn multiline_text_is_preserved() {
        let msgs = vec![Message::assistant("строка 1\nстрока 2")];
        let out = format_conversation("", &msgs, &plain(), ru()).unwrap();
        assert_eq!(out, "Ассистент:\nстрока 1\nстрока 2");
    }

    #[test]
    fn none_when_no_meaningful_messages() {
        // Только системное/инструментальное/пустое — копировать нечего, даже с заголовком.
        let msgs = vec![
            Message::new(MessageRole::System, "sys"),
            Message::user("   "),
            Message::new(MessageRole::Tool, "out"),
        ];
        assert!(format_conversation("Заголовок", &msgs, &plain(), ru()).is_none());
        assert!(format_conversation("", &[], &plain(), ru()).is_none());
    }

    #[test]
    fn plain_omits_thoughts_and_tools() {
        // По умолчанию «мысли» и tool-блоки не копируются — только текст.
        let out = format_conversation("", &[assistant_with_tool()], &plain(), ru()).unwrap();
        assert_eq!(out, "Ассистент:\nответ");
    }

    #[test]
    fn thoughts_included_before_text_when_enabled() {
        let opts = CopySettings {
            copy_thoughts: true,
            ..Default::default()
        };
        let out = format_conversation("", &[assistant_with_tool()], &opts, ru()).unwrap();
        assert_eq!(out, "Ассистент:\n[Мысли]\nя думаю\n\nответ");
    }

    #[test]
    fn tool_calls_include_name_and_arguments() {
        let opts = CopySettings {
            copy_tool_calls: true,
            ..Default::default()
        };
        let out = format_conversation("", &[assistant_with_tool()], &opts, ru()).unwrap();
        assert_eq!(
            out,
            "Ассистент:\nответ\n\n[Инструмент: note_save]\nАргументы: {\"text\":\"заметка\"}"
        );
    }

    #[test]
    fn tool_results_include_name_and_result() {
        let opts = CopySettings {
            copy_tool_results: true,
            ..Default::default()
        };
        let out = format_conversation("", &[assistant_with_tool()], &opts, ru()).unwrap();
        assert_eq!(
            out,
            "Ассистент:\nответ\n\n[Инструмент: note_save]\nРезультат: сохранено"
        );
    }

    #[test]
    fn calls_and_results_combine_under_one_header() {
        let opts = CopySettings {
            copy_tool_calls: true,
            copy_tool_results: true,
            ..Default::default()
        };
        let out = format_conversation("", &[assistant_with_tool()], &opts, ru()).unwrap();
        assert_eq!(
            out,
            "Ассистент:\nответ\n\n[Инструмент: note_save]\n\
             Аргументы: {\"text\":\"заметка\"}\nРезультат: сохранено"
        );
    }

    #[test]
    fn assistant_with_only_tool_calls_is_included_when_enabled() {
        // Пустой текст, но есть вызов инструмента и опция включена → блок не пропадает.
        let mut m = Message::assistant("");
        m.tool_calls = vec![ToolCallRecord {
            thought_signature: None,
            id: "c1".into(),
            name: "calculate".into(),
            arguments: serde_json::json!({"expr": "2+2"}),
            result: Some("4".into()),
        }];
        let opts = CopySettings {
            copy_tool_results: true,
            ..Default::default()
        };
        let out = format_conversation("", &[m], &opts, ru()).unwrap();
        assert_eq!(out, "Ассистент:\n[Инструмент: calculate]\nРезультат: 4");
        // Но при выключенных опциях такое сообщение пропускается целиком.
        let mut m2 = Message::assistant("");
        m2.tool_calls = vec![ToolCallRecord {
            thought_signature: None,
            id: "c1".into(),
            name: "calculate".into(),
            arguments: serde_json::json!({}),
            result: Some("4".into()),
        }];
        assert!(format_conversation("", &[m2], &plain(), ru()).is_none());
    }
}

//! Exports a chat conversation as plain text for copying to the clipboard.
//! Pure logic, testable without the UI (called by the orchestrator). See spec §11.2.

use crate::entities::message::{Message, MessageRole};
use crate::entities::profile::CharacterNames;
use crate::shared::config::CopySettings;
use crate::shared::i18n::Locale;

/// Formats a whole chat conversation into readable text for the clipboard: labels
/// roles (`User`/`Assistant`), preserves multiline text, skips
/// system/tool messages. A non-empty `title` becomes the header.
///
/// `names` — the profile's custom role names (spec §5.1): a set name replaces the
/// localized label (`Gaia:` instead of `User:`); empty fields keep the default.
///
/// By default (`CopySettings` with all flags `false`) only the message text is copied.
/// Optionally (`opts`) the assistant block also gets: "thoughts" (CoT, before the text),
/// tool-call parameters (name + arguments), and their results — taken from
/// `Message.tool_calls` (separate tool messages are still skipped, to avoid
/// duplication). Returns `None` if there are no substantive blocks (nothing to copy).
pub fn format_conversation(
    title: &str,
    messages: &[Message],
    opts: &CopySettings,
    names: &CharacterNames,
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
                    blocks.push(format!(
                        "{}\n{text}",
                        role_label(names.user_name(), "ui.export.user", loc)
                    ));
                }
            }
            MessageRole::Assistant => {
                if let Some(block) = format_assistant(m, opts, names, loc) {
                    blocks.push(block);
                }
            }
            // System and tool messages don't go into the conversation
            // (tool results are taken from the assistant's `tool_calls`).
            MessageRole::System | MessageRole::Tool => continue,
        }
    }

    // Only the title (or nothing at all) — there's no substantive conversation.
    let has_messages = blocks.len() > usize::from(!title.is_empty());
    if !has_messages {
        return None;
    }
    Some(blocks.join("\n\n"))
}

/// A role's label for the export: the custom name (with the same trailing colon the
/// localized label carries) or the localized default under `key`.
fn role_label(custom: Option<&str>, key: &str, loc: &'static Locale) -> String {
    match custom {
        Some(name) => format!("{name}:"),
        None => loc.t(key).to_string(),
    }
}

/// Builds the block for a single assistant message: optional "thoughts", text, and
/// optional tool blocks. `None` if the block is empty after filtering.
fn format_assistant(
    m: &Message,
    opts: &CopySettings,
    names: &CharacterNames,
    loc: &'static Locale,
) -> Option<String> {
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
        role_label(names.assistant_name(), "ui.export.assistant", loc),
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

    /// Text-only copy (the default behavior).
    fn plain() -> CopySettings {
        CopySettings::default()
    }

    /// No custom role names — the localized labels are used.
    fn no_names() -> CharacterNames {
        CharacterNames::default()
    }

    /// An assistant message with "thoughts" and one tool call (name/arguments/
    /// result) — for checking the optional blocks.
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
        let out = format_conversation("Мой чат", &msgs, &plain(), &no_names(), ru()).unwrap();
        assert_eq!(
            out,
            "Мой чат\n\nПользователь:\nкак дела?\n\nАссистент:\nхорошо"
        );
    }

    #[test]
    fn empty_title_is_omitted() {
        let msgs = vec![Message::user("привет")];
        let out = format_conversation("   ", &msgs, &plain(), &no_names(), ru()).unwrap();
        assert_eq!(out, "Пользователь:\nпривет");
    }

    #[test]
    fn multiline_text_is_preserved() {
        let msgs = vec![Message::assistant("строка 1\nстрока 2")];
        let out = format_conversation("", &msgs, &plain(), &no_names(), ru()).unwrap();
        assert_eq!(out, "Ассистент:\nстрока 1\nстрока 2");
    }

    #[test]
    fn none_when_no_meaningful_messages() {
        // Only system/tool/empty ones — nothing to copy, even with a title.
        let msgs = vec![
            Message::new(MessageRole::System, "sys"),
            Message::user("   "),
            Message::new(MessageRole::Tool, "out"),
        ];
        assert!(format_conversation("Заголовок", &msgs, &plain(), &no_names(), ru()).is_none());
        assert!(format_conversation("", &[], &plain(), &no_names(), ru()).is_none());
    }

    #[test]
    fn plain_omits_thoughts_and_tools() {
        // By default "thoughts" and tool blocks aren't copied — only the text.
        let out =
            format_conversation("", &[assistant_with_tool()], &plain(), &no_names(), ru()).unwrap();
        assert_eq!(out, "Ассистент:\nответ");
    }

    #[test]
    fn thoughts_included_before_text_when_enabled() {
        let opts = CopySettings {
            copy_thoughts: true,
            ..Default::default()
        };
        let out =
            format_conversation("", &[assistant_with_tool()], &opts, &no_names(), ru()).unwrap();
        assert_eq!(out, "Ассистент:\n[Мысли]\nя думаю\n\nответ");
    }

    #[test]
    fn tool_calls_include_name_and_arguments() {
        let opts = CopySettings {
            copy_tool_calls: true,
            ..Default::default()
        };
        let out =
            format_conversation("", &[assistant_with_tool()], &opts, &no_names(), ru()).unwrap();
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
        let out =
            format_conversation("", &[assistant_with_tool()], &opts, &no_names(), ru()).unwrap();
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
        let out =
            format_conversation("", &[assistant_with_tool()], &opts, &no_names(), ru()).unwrap();
        assert_eq!(
            out,
            "Ассистент:\nответ\n\n[Инструмент: note_save]\n\
             Аргументы: {\"text\":\"заметка\"}\nРезультат: сохранено"
        );
    }

    #[test]
    fn assistant_with_only_tool_calls_is_included_when_enabled() {
        // Empty text, but there's a tool call and the option is enabled → the block isn't dropped.
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
        let out = format_conversation("", &[m], &opts, &no_names(), ru()).unwrap();
        assert_eq!(out, "Ассистент:\n[Инструмент: calculate]\nРезультат: 4");
        // But with the options off, such a message is skipped entirely.
        let mut m2 = Message::assistant("");
        m2.tool_calls = vec![ToolCallRecord {
            thought_signature: None,
            id: "c1".into(),
            name: "calculate".into(),
            arguments: serde_json::json!({}),
            result: Some("4".into()),
        }];
        assert!(format_conversation("", &[m2], &plain(), &no_names(), ru()).is_none());
    }

    /// Custom role names from the profile replace the localized labels; an empty
    /// field keeps its default (a name may be set for one side only).
    #[test]
    fn custom_names_replace_role_labels() {
        let msgs = vec![Message::user("привет"), Message::assistant("здравствуй")];
        let names = CharacterNames {
            user: "Гайя".into(),
            assistant: "Анна".into(),
            system: String::new(),
        };
        let out = format_conversation("", &msgs, &plain(), &names, ru()).unwrap();
        assert_eq!(out, "Гайя:\nпривет\n\nАнна:\nздравствуй");

        let only_assistant = CharacterNames {
            assistant: "Анна".into(),
            ..Default::default()
        };
        let out = format_conversation("", &msgs, &plain(), &only_assistant, ru()).unwrap();
        assert_eq!(out, "Пользователь:\nпривет\n\nАнна:\nздравствуй");
    }

    /// A whitespace-only name counts as "not set" (the field isn't a way to blank
    /// out the label).
    #[test]
    fn blank_custom_name_falls_back_to_default() {
        let names = CharacterNames {
            user: "   ".into(),
            ..Default::default()
        };
        let out =
            format_conversation("", &[Message::user("привет")], &plain(), &names, ru()).unwrap();
        assert_eq!(out, "Пользователь:\nпривет");
    }
}

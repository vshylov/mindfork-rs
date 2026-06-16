//! Экспорт переписки чата в простой текст для копирования в буфер обмена.
//! Чистая, тестируемая без UI логика (вызывается оркестратором). См. spec §11.2.

use crate::entities::message::{Message, MessageRole};

/// Форматирует всю переписку чата в читаемый текст для буфера обмена: помечает
/// роли (`Пользователь`/`Ассистент`), сохраняет многострочный текст, пропускает
/// системные/инструментальные и пустые сообщения. Непустой `title` идёт заголовком.
/// Возвращает `None`, если содержательных сообщений нет (копировать нечего).
pub fn format_conversation(title: &str, messages: &[Message]) -> Option<String> {
    let mut blocks: Vec<String> = Vec::new();

    let title = title.trim();
    if !title.is_empty() {
        blocks.push(title.to_string());
    }

    for m in messages {
        let who = match m.role {
            MessageRole::User => "Пользователь",
            MessageRole::Assistant => "Ассистент",
            // Системные и инструментальные сообщения в переписку не входят.
            MessageRole::System | MessageRole::Tool => continue,
        };
        let text = m.text.trim();
        if text.is_empty() {
            continue;
        }
        blocks.push(format!("{who}:\n{text}"));
    }

    // Только заголовок (или вовсе ничего) — содержательной переписки нет.
    let has_messages = blocks.len() > usize::from(!title.is_empty());
    if !has_messages {
        return None;
    }
    Some(blocks.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_roles_with_title_and_skips_system_and_tools() {
        let msgs = vec![
            Message::new(MessageRole::System, "ты ассистент"),
            Message::user("как дела?"),
            Message::assistant("хорошо"),
            Message::new(MessageRole::Tool, "tool output"),
        ];
        let out = format_conversation("Мой чат", &msgs).unwrap();
        assert_eq!(
            out,
            "Мой чат\n\nПользователь:\nкак дела?\n\nАссистент:\nхорошо"
        );
    }

    #[test]
    fn empty_title_is_omitted() {
        let msgs = vec![Message::user("привет")];
        let out = format_conversation("   ", &msgs).unwrap();
        assert_eq!(out, "Пользователь:\nпривет");
    }

    #[test]
    fn multiline_text_is_preserved() {
        let msgs = vec![Message::assistant("строка 1\nстрока 2")];
        let out = format_conversation("", &msgs).unwrap();
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
        assert!(format_conversation("Заголовок", &msgs).is_none());
        assert!(format_conversation("", &[]).is_none());
    }
}

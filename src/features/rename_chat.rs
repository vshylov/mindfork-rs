//! Переименование чата: нормализация заголовка (чистая логика) и сборка
//! пересказа переписки для авто-названия моделью. См. spec §11.2.

use crate::entities::message::{Message, MessageRole};

/// Максимальная длина заголовка чата (в символах). Лишнее обрезается.
pub const MAX_TITLE_LEN: usize = 100;

/// Бюджет символов переписки, передаваемый модели для придумывания заголовка.
/// Если переписка длиннее — берём начало и конец (тема обычно задаётся в начале,
/// но может смещаться к концу разговора). См. spec §11.2.
pub const TITLE_CONTEXT_BUDGET: usize = 4000;

/// Системное сообщение модели для авто-названия чата по переписке.
pub const TITLE_SYSTEM_MESSAGE: &str = "Ты придумываешь короткий заголовок для переписки между пользователем и \
ассистентом. Прочитай переписку и ответь ТОЛЬКО заголовком из 2–6 слов на языке \
переписки: без кавычек, без точки в конце, без пояснений и без префиксов вроде \
«Заголовок:». Заголовок должен отражать главную тему разговора.";

/// Собирает компактный пересказ переписки для запроса авто-названия: помечает
/// роли, пропускает системные/инструментальные и пустые сообщения. Если суммарный
/// объём превышает [`TITLE_CONTEXT_BUDGET`], берёт начало и конец (середина
/// выкидывается). Возвращает `None`, если содержательных сообщений нет.
pub fn build_conversation_digest(messages: &[Message]) -> Option<String> {
    let lines: Vec<String> = messages
        .iter()
        .filter(|m| matches!(m.role, MessageRole::User | MessageRole::Assistant))
        .filter(|m| !m.text.trim().is_empty())
        .map(|m| {
            let who = match m.role {
                MessageRole::User => "Пользователь",
                _ => "Ассистент",
            };
            format!("{who}: {}", m.text.trim())
        })
        .collect();
    if lines.is_empty() {
        return None;
    }
    Some(truncate_middle(&lines.join("\n"), TITLE_CONTEXT_BUDGET))
}

/// Ограничивает строку `budget` символами (по Unicode-символам, не байтам): если
/// длиннее — берёт начало (≈⅔ бюджета) и конец, заменяя середину маркером.
fn truncate_middle(text: &str, budget: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= budget {
        return text.to_string();
    }
    let head = budget * 2 / 3;
    let tail = budget - head;
    let head_str: String = chars[..head].iter().collect();
    let tail_str: String = chars[chars.len() - tail..].iter().collect();
    format!("{head_str}\n…\n{tail_str}")
}

/// Чистит сгенерированный моделью заголовок: берёт первую непустую строку, срезает
/// обрамляющие кавычки/звёздочки/бэктики и нормализует через [`sanitize_title`].
/// Возвращает `None`, если после очистки строка пуста.
pub fn clean_generated_title(raw: &str) -> Option<String> {
    let first = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    let trimmed = first.trim_matches(|c: char| {
        matches!(c, '"' | '\'' | '«' | '»' | '*' | '`') || c.is_whitespace()
    });
    sanitize_title(trimmed)
}

/// Нормализует пользовательский ввод заголовка: убирает крайние пробелы,
/// схлопывает внутренние переводы строк/табы в пробел и ограничивает длину.
/// Возвращает `None`, если после нормализации строка пуста (переименование
/// отклоняется — старый заголовок сохраняется).
pub fn sanitize_title(input: &str) -> Option<String> {
    let collapsed: String = input
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    let trimmed = collapsed.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Схлопываем повторяющиеся пробелы для опрятного заголовка.
    let mut title = String::with_capacity(trimmed.len());
    let mut prev_space = false;
    for ch in trimmed.chars() {
        let is_space = ch == ' ';
        if is_space && prev_space {
            continue;
        }
        title.push(ch);
        prev_space = is_space;
    }
    Some(title.chars().take(MAX_TITLE_LEN).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_and_keeps_text() {
        assert_eq!(sanitize_title("  Мой чат  ").as_deref(), Some("Мой чат"));
    }

    #[test]
    fn empty_or_whitespace_is_rejected() {
        assert_eq!(sanitize_title(""), None);
        assert_eq!(sanitize_title("   \t\n "), None);
    }

    #[test]
    fn collapses_internal_whitespace() {
        assert_eq!(
            sanitize_title("много   \t пробелов\nи строк").as_deref(),
            Some("много пробелов и строк")
        );
    }

    #[test]
    fn truncates_to_max_len_by_chars() {
        let long = "я".repeat(MAX_TITLE_LEN + 50);
        let out = sanitize_title(&long).unwrap();
        assert_eq!(out.chars().count(), MAX_TITLE_LEN);
    }

    #[test]
    fn digest_labels_roles_and_skips_system_and_tools() {
        let msgs = vec![
            Message::new(MessageRole::System, "ты ассистент"),
            Message::user("как дела?"),
            Message::assistant("хорошо"),
            Message::new(MessageRole::Tool, "tool output"),
        ];
        let digest = build_conversation_digest(&msgs).unwrap();
        assert_eq!(digest, "Пользователь: как дела?\nАссистент: хорошо");
    }

    #[test]
    fn digest_is_none_without_meaningful_messages() {
        let msgs = vec![
            Message::new(MessageRole::System, "sys"),
            Message::user("   "),
        ];
        assert!(build_conversation_digest(&msgs).is_none());
    }

    #[test]
    fn digest_truncates_long_conversation_keeping_head_and_tail() {
        // Длинное сообщение пользователя — должно усечься с маркером середины.
        let long = "слово ".repeat(2000); // ~12000 символов
        let msgs = vec![Message::user(long)];
        let digest = build_conversation_digest(&msgs).unwrap();
        assert!(digest.chars().count() <= TITLE_CONTEXT_BUDGET + 16);
        assert!(digest.contains('…'), "маркер усечения середины");
        assert!(digest.starts_with("Пользователь:"));
    }

    #[test]
    fn clean_title_strips_quotes_and_takes_first_line() {
        assert_eq!(
            clean_generated_title("«Планы на выходные»").as_deref(),
            Some("Планы на выходные")
        );
        assert_eq!(
            clean_generated_title("\"Рецепт борща\"\nещё текст").as_deref(),
            Some("Рецепт борща")
        );
        assert_eq!(clean_generated_title("   \n  ").as_deref(), None);
    }
}

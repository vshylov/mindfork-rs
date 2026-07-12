//! Переименование чата: нормализация заголовка (чистая логика) и сборка
//! пересказа переписки для авто-названия моделью. См. spec §11.2.

use crate::entities::message::{Message, MessageRole};
use crate::shared::i18n::Locale;

/// Максимальная длина заголовка чата (в символах). Лишнее обрезается.
pub const MAX_TITLE_LEN: usize = 100;

/// Бюджет символов переписки, передаваемый модели для придумывания заголовка.
/// Если переписка длиннее — берём начало и конец (тема обычно задаётся в начале,
/// но может смещаться к концу разговора). См. spec §11.2.
pub const TITLE_CONTEXT_BUDGET: usize = 4000;

/// Системное сообщение модели для авто-названия чата на языке служебного каркаса
/// (`loc`, ось A). Заголовок всё равно просят «на языке переписки» — поэтому язык
/// каркаса не навязывает язык заголовка (docs/i18n.md, развилка 6).
pub fn title_system_message(loc: &Locale) -> String {
    loc.t("prompt.title.system").to_string()
}

/// Собирает компактный пересказ переписки для запроса авто-названия: помечает
/// роли (на языке каркаса `loc`), пропускает системные/инструментальные и пустые
/// сообщения. Если суммарный объём превышает [`TITLE_CONTEXT_BUDGET`], берёт начало
/// и конец (середина выкидывается). Возвращает `None`, если содержательных
/// сообщений нет.
pub fn build_conversation_digest(messages: &[Message], loc: &Locale) -> Option<String> {
    let lines: Vec<String> = messages
        .iter()
        .filter(|m| matches!(m.role, MessageRole::User | MessageRole::Assistant))
        .filter(|m| !m.text.trim().is_empty())
        .map(|m| {
            let who = match m.role {
                MessageRole::User => loc.t("digest.role.user"),
                _ => loc.t("digest.role.assistant"),
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

    /// Референсная локаль (ru) — ассерты на русские роли пинят ru-бандл.
    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

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
        let digest = build_conversation_digest(&msgs, ru()).unwrap();
        assert_eq!(digest, "Пользователь: как дела?\nАссистент: хорошо");
    }

    /// Per-language (§3.5): роли в дайджесте — из бандла активного языка (en-профиль
    /// получает «User:/Assistant:», а не русские роли).
    #[test]
    fn digest_roles_localized_for_all_langs() {
        let msgs = vec![Message::user("hi"), Message::assistant("yo")];
        for &lang in crate::shared::i18n::Lang::ALL {
            let l = crate::shared::i18n::locale(lang);
            let d = build_conversation_digest(&msgs, l).unwrap();
            assert!(d.contains(l.t("digest.role.user")), "{lang:?}: {d}");
            assert!(d.contains(l.t("digest.role.assistant")), "{lang:?}: {d}");
        }
    }

    #[test]
    fn digest_is_none_without_meaningful_messages() {
        let msgs = vec![
            Message::new(MessageRole::System, "sys"),
            Message::user("   "),
        ];
        assert!(build_conversation_digest(&msgs, ru()).is_none());
    }

    #[test]
    fn digest_truncates_long_conversation_keeping_head_and_tail() {
        // Длинное сообщение пользователя — должно усечься с маркером середины.
        let long = "слово ".repeat(2000); // ~12000 символов
        let msgs = vec![Message::user(long)];
        let digest = build_conversation_digest(&msgs, ru()).unwrap();
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

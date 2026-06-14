//! Переименование чата: нормализация заголовка (чистая логика). См. spec §11.2.

/// Максимальная длина заголовка чата (в символах). Лишнее обрезается.
pub const MAX_TITLE_LEN: usize = 100;

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
}

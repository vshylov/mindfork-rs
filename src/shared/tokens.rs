//! Грубая оценка числа токенов текста — для live-индикатора «токены переписки»
//! в статус-баре **до** того, как сервер пришлёт точный `usage.prompt_tokens`
//! (см. spec §11.1). Точное число всегда заменяет оценку, когда приходит.
//!
//! Эвристика: «~4 байта UTF-8 на токен». Она естественно учитывает плотность
//! письменности: латиница (1 байт/символ) → ≈4 символа на токен, кириллица
//! (2 байта/символ) → ≈2 символа на токен — что близко к поведению BPE-токенизаторов
//! Gemma/Qwen на смешанном тексте. Это лишь оценка порядка величины, не точный счёт.

/// Накладные расходы чат-шаблона на одно сообщение (маркеры роли, разделители).
const PER_MESSAGE_OVERHEAD: u64 = 4;

/// Оценка числа токенов в тексте: длина в байтах UTF-8, делённая на 4
/// (округление вверх, чтобы непустой текст давал ≥1 токен).
pub fn estimate_text(text: &str) -> u64 {
    (text.len() as u64).div_ceil(4)
}

/// Оценка числа токенов «переписки» (промпта): системное сообщение + все реплики
/// с поправкой на разметку чат-шаблона. `parts` — содержимое сообщений по порядку
/// (роль учитывается лишь накладными расходами, поэтому достаточно текста).
pub fn estimate_prompt<'a>(system: Option<&str>, parts: impl IntoIterator<Item = &'a str>) -> u64 {
    let mut total = 0;
    if let Some(s) = system {
        total += estimate_text(s) + PER_MESSAGE_OVERHEAD;
    }
    for part in parts {
        total += estimate_text(part) + PER_MESSAGE_OVERHEAD;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_is_about_four_chars_per_token() {
        // 16 ASCII-байт → 4 токена.
        assert_eq!(estimate_text("abcdefghijklmnop"), 4);
    }

    #[test]
    fn cyrillic_is_denser_per_char() {
        // 4 кириллических символа = 8 байт → 2 токена (≈2 символа на токен).
        assert_eq!(estimate_text("текс"), 2);
    }

    #[test]
    fn non_empty_text_is_at_least_one_token() {
        assert_eq!(estimate_text("a"), 1);
        assert_eq!(estimate_text(""), 0);
    }

    #[test]
    fn prompt_sums_messages_with_overhead() {
        // system "ab" (1) + overhead 4 = 5; два сообщения "cd" (1)+4 каждое = 10.
        let est = estimate_prompt(Some("ab"), ["cd", "cd"]);
        assert_eq!(est, 5 + 10);
    }
}

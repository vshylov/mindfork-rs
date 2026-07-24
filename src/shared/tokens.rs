//! Rough token-count estimate for text — for the live "conversation tokens"
//! indicator in the status bar **before** the server sends the exact
//! `usage.prompt_tokens` (see spec §11.1). The exact number always replaces
//! the estimate once it arrives.
//!
//! Heuristic: "~4 UTF-8 bytes per token". It naturally accounts for script
//! density: Latin (1 byte/char) → ≈4 chars/token, Cyrillic (2 bytes/char) →
//! ≈2 chars/token — close to the behavior of Gemma/Qwen's BPE tokenizers on
//! mixed text. This is only an order-of-magnitude estimate, not an exact count.

/// Chat-template overhead per message (role markers, separators).
const PER_MESSAGE_OVERHEAD: u64 = 4;

/// Token-count estimate for a text: length in UTF-8 bytes divided by 4
/// (rounded up, so non-empty text gives ≥1 token).
pub fn estimate_text(text: &str) -> u64 {
    (text.len() as u64).div_ceil(4)
}

/// Token-count estimate for the "conversation" (prompt): the system message +
/// all messages, adjusted for chat-template markup. `parts` — message content
/// in order (role only affects the overhead, so text alone is enough).
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
        // 16 ASCII bytes → 4 tokens.
        assert_eq!(estimate_text("abcdefghijklmnop"), 4);
    }

    #[test]
    fn cyrillic_is_denser_per_char() {
        // 4 Cyrillic characters = 8 bytes → 2 tokens (≈2 chars/token).
        assert_eq!(estimate_text("текс"), 2);
    }

    #[test]
    fn non_empty_text_is_at_least_one_token() {
        assert_eq!(estimate_text("a"), 1);
        assert_eq!(estimate_text(""), 0);
    }

    #[test]
    fn prompt_sums_messages_with_overhead() {
        // system "ab" (1) + overhead 4 = 5; two messages "cd" (1)+4 each = 10.
        let est = estimate_prompt(Some("ab"), ["cd", "cd"]);
        assert_eq!(est, 5 + 10);
    }
}

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

/// A **lower bound** on a text's token count — what the app can defend when it
/// tells a prompt the server cut in silence from the server's own figure
/// (docs/research/prompt-cut-detection.md §3.2). [`estimate_text`] cannot be
/// one: measured against Ollama's count it is over the real figure by 2.28× on
/// columns aligned with spaces and 3.36× on rule lines of `=`, because a
/// tokenizer merges a run of one character. So a run's first two characters
/// count and the rest do not, and the bytes left are divided by 8 — half the
/// estimate's four a token, which covers the largest overcount measured on
/// ordinary text (Russian prose, 1.68×). Under 0.71 of the real figure on every
/// sample measured.
pub fn floor_text(text: &str) -> u64 {
    let mut bytes: u64 = 0;
    let mut prev = None;
    let mut run = 0u32;
    for c in text.chars() {
        if Some(c) == prev {
            run += 1;
        } else {
            prev = Some(c);
            run = 1;
        }
        if run <= 2 {
            bytes += c.len_utf8() as u64;
        }
    }
    bytes / 8
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
    fn the_floor_is_half_the_estimate_on_ordinary_text() {
        // 16 bytes with no run longer than two: 16 / 8.
        assert_eq!(floor_text("abcdefghijklmnop"), 2);
        // Cyrillic counts by bytes, as the estimate does: 9 chars, 18 bytes.
        assert_eq!(floor_text("текстовый"), 2);
        assert_eq!(floor_text(""), 0);
    }

    #[test]
    fn a_run_of_one_character_counts_two() {
        // 120 '=' are one or two tokens to a tokenizer, not 30.
        let rule = "=".repeat(120);
        assert_eq!(floor_text(&rule), 0);
        // Aligned columns: the padding counts two spaces a gap.
        let padded = format!("item{}42", " ".repeat(40));
        assert_eq!(floor_text(&padded), floor_text("item  42"));
        // A doubled letter is ordinary text and counts in full.
        assert_eq!(floor_text("aabbccddeeffgghh"), 2);
    }

    #[test]
    fn the_floor_never_exceeds_the_estimate() {
        for text in [
            "",
            "a",
            "hello, world",
            "== == ==",
            "текст",
            "日本語のテキスト",
        ] {
            assert!(floor_text(text) <= estimate_text(text), "{text}");
        }
    }

    #[test]
    fn prompt_sums_messages_with_overhead() {
        // system "ab" (1) + overhead 4 = 5; two messages "cd" (1)+4 each = 10.
        let est = estimate_prompt(Some("ab"), ["cd", "cd"]);
        assert_eq!(est, 5 + 10);
    }
}

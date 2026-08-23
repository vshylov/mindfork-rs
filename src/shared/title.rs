//! Chat-title normalization — pure string logic, shared by the list's `F2`
//! editor, `/rename`, the model-written title cleaner and (from the sub-agent
//! track on) a storage migration step that names a synthesized transcript
//! (docs/research/subagent-chats.md §3.13). It lives in `shared` because a
//! migration step cannot reach `features` (FSD: dependencies point downward).
//! See spec §11.2.

/// Maximum chat-title length (in characters). Anything past it is trimmed.
pub const MAX_TITLE_LEN: usize = 100;

/// Normalizes user-entered title input: trims the outer whitespace,
/// collapses internal line breaks/tabs into a space, and limits the length.
/// Returns `None` if the string is empty after normalization (the rename
/// is rejected — the old title is kept).
pub fn sanitize_title(input: &str) -> Option<String> {
    let collapsed: String = input
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    let trimmed = collapsed.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Collapse repeated spaces for a tidy title.
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

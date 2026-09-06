//! Chat-title normalization — pure string logic, shared by the list's `F2`
//! editor, `/rename`, the model-written title cleaner and (from the sub-agent
//! track on) a storage migration step that names a synthesized transcript
//! (docs/research/subagent-chats.md §3.13). It lives in `shared` because a
//! migration step cannot reach `features` (FSD: dependencies point downward).
//! See spec §11.2.

/// Normalizes user-entered title input: trims the outer whitespace,
/// collapses internal line breaks/tabs into a space, and collapses repeated
/// spaces. Returns `None` if the string is empty after normalization (the
/// rename is rejected — the old title is kept).
///
/// **Length is not bounded here** (the user's decision of 2026-09-06). A cap
/// used to cut the title at 100 characters, and a cut made in storage arrives
/// at a screen looking whole: a run named after the first line of its
/// instruction ended mid-word on a maximized window with columns to spare and
/// read as its own full name. A title is now cut only where it does not fit —
/// by the surface drawing it, in columns, with the "…" every such cut carries
/// (`wrap::truncate_to_width`, spec §11.2). Every surface that draws a title
/// in a fixed row must therefore do that cut itself; the ones that wrap
/// (the search screen, a feed note) need nothing.
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
    Some(title)
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
    fn a_long_title_is_kept_whole() {
        // No ceiling here: what does not fit is cut by the screen drawing it,
        // in columns and with a marker. A cut made here would arrive looking
        // like the whole name (spec §11.2).
        let long = "я".repeat(400);
        assert_eq!(sanitize_title(&long).as_deref(), Some(long.as_str()));
    }
}

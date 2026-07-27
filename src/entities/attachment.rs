//! A file attached to a chat (`/file attach`, docs/file-attachments.md).
//!
//! The extracted text is a **snapshot** taken at attach time and stored in the
//! chat file (fork F3): the conversation stays coherent if the file later
//! changes or disappears, building the request does no I/O on the hot path, and
//! the chat stays self-contained for backup/export. This mirrors what RAG
//! already does with `rag_sources`.
//!
//! Sizes are carried in **estimated tokens** (`shared::tokens`, fork F6):
//! characters mislead across scripts (Cyrillic ≈2 chars/token vs ≈4 for Latin),
//! and tokens are the unit the user sees in the status bar.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::shared::tokens::estimate_text;

/// How an attachment reaches the model. Decided at attach time from the budget
/// (`config.attachments`) and stored, so building a request only renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachMode {
    /// Fits the budget: the full text goes into the pinned block of every request.
    #[default]
    Inline,
    /// Too large to inline: the block carries metadata and an excerpt; the rest
    /// is read on demand (the `attachment_read` tool, stage 2 of the track).
    ByReference,
}

/// A file attached to a chat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: Uuid,
    /// Display name (the file name) — how the user and the model refer to it.
    pub name: String,
    /// Canonical path at attach time (for `/file remove <path>` and a future
    /// `/file refresh`).
    pub source: String,
    pub added_at: DateTime<Utc>,
    /// The extracted text snapshot (see the module doc).
    pub text: String,
    /// Size of the original file in bytes (shown to the user).
    pub bytes: usize,
    /// Estimated token count of [`Self::text`].
    pub est_tokens: usize,
    pub mode: AttachMode,
}

impl Attachment {
    /// Builds an attachment from already-extracted text. `mode` is decided by
    /// the caller (the orchestrator, which knows the budget and what is already
    /// attached).
    pub fn new(
        name: impl Into<String>,
        source: impl Into<String>,
        text: String,
        bytes: usize,
        mode: AttachMode,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            source: source.into(),
            added_at: Utc::now(),
            est_tokens: estimate_text(&text) as usize,
            text,
            bytes,
            mode,
        }
    }

    /// A card for the UI (no text — the feed/status bar must not carry hundreds
    /// of KB through the event channel).
    pub fn info(&self) -> AttachmentInfo {
        AttachmentInfo {
            name: self.name.clone(),
            bytes: self.bytes,
            est_tokens: self.est_tokens,
            mode: self.mode,
        }
    }

    /// Does the user's `/file remove <target>` refer to this attachment? Matches
    /// the display name or the full source path, case-insensitively (Windows
    /// paths differ only in case all the time).
    pub fn matches(&self, target: &str) -> bool {
        let t = target.trim().trim_matches(|c| c == '"' || c == '\'');
        self.name.eq_ignore_ascii_case(t) || self.source.eq_ignore_ascii_case(t)
    }

    /// The leading `max_tokens` (estimated) of the text — the excerpt shown in
    /// the pinned block for a by-reference attachment. Cut on a character
    /// boundary, then rolled back to the last whitespace so the excerpt doesn't
    /// end mid-word. Returns the whole text when it already fits.
    pub fn excerpt(&self, max_tokens: usize) -> &str {
        excerpt(&self.text, max_tokens)
    }
}

/// An attachment card for the UI (`/file list`, the status-bar chip).
#[derive(Debug, Clone, PartialEq)]
pub struct AttachmentInfo {
    pub name: String,
    pub bytes: usize,
    pub est_tokens: usize,
    pub mode: AttachMode,
}

/// The leading `max_tokens` (estimated) of `text` — see [`Attachment::excerpt`].
/// A free function so it can be tested without building an attachment.
pub fn excerpt(text: &str, max_tokens: usize) -> &str {
    // The estimate is "UTF-8 bytes / 4" (`shared::tokens`), so the budget in
    // bytes is the inverse.
    let max_bytes = max_tokens.saturating_mul(4);
    if text.len() <= max_bytes {
        return text;
    }
    // Floor to a character boundary, then to the last whitespace inside the
    // budget (if there is one) — a tidy cut rather than a broken word.
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let head = &text[..end];
    match head.rfind(char::is_whitespace) {
        // Guard against a degenerate cut: don't throw away most of the budget
        // just to land on whitespace.
        Some(i) if i * 2 > end => &head[..i],
        _ => head,
    }
}

/// A compact human-readable file size (`840 B`, `12.3 KB`, `1.4 MB`) — shown to
/// the user in notes/`/file list` and to the model in the pinned block, so it
/// lives here rather than in either consumer.
pub fn format_bytes(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    let b = bytes as f64;
    if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Total estimated tokens of the attachments rendered **inline** (the standing
/// cost paid on every request in the chat). By-reference ones contribute only
/// their excerpt, which the caller accounts for separately.
pub fn inline_tokens(attachments: &[Attachment]) -> usize {
    attachments
        .iter()
        .filter(|a| a.mode == AttachMode::Inline)
        .map(|a| a.est_tokens)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(name: &str, text: &str, mode: AttachMode) -> Attachment {
        Attachment::new(
            name,
            format!("/tmp/{name}"),
            text.to_string(),
            text.len(),
            mode,
        )
    }

    #[test]
    fn est_tokens_follows_the_shared_heuristic() {
        let a = att("a.txt", "abcdefghijklmnop", AttachMode::Inline); // 16 bytes → 4
        assert_eq!(a.est_tokens, 4);
        // Cyrillic is denser per character (2 bytes/char).
        let b = att("b.txt", "текс", AttachMode::Inline);
        assert_eq!(b.est_tokens, 2);
    }

    #[test]
    fn matches_by_name_or_path_case_insensitively() {
        let a = att("Notes.md", "x", AttachMode::Inline);
        assert!(a.matches("notes.md"));
        assert!(a.matches("NOTES.MD"));
        assert!(a.matches("/tmp/Notes.md"));
        assert!(a.matches("  \"notes.md\"  "));
        assert!(!a.matches("other.md"));
    }

    #[test]
    fn excerpt_cuts_on_word_boundary_and_keeps_short_text_whole() {
        let short = "small";
        assert_eq!(excerpt(short, 10), short);
        // 10 words of 4 bytes + spaces; budget 2 tokens = 8 bytes → cut at the
        // last whitespace inside it.
        let long = "aaaa bbbb cccc dddd";
        let cut = excerpt(long, 2);
        assert!(long.starts_with(cut), "the excerpt is a prefix: {cut:?}");
        assert!(
            !cut.ends_with(' '),
            "trailing whitespace is trimmed off the cut"
        );
        assert!(cut.len() <= 8, "the excerpt fits the byte budget: {cut:?}");
    }

    #[test]
    fn excerpt_never_splits_a_character() {
        // Cyrillic: 2 bytes per character; an odd byte budget must not land
        // inside a character (that would panic on slicing).
        let text = "абвгдеёжзийклмн";
        let cut = excerpt(text, 1); // 4 bytes → 2 characters
        assert!(text.starts_with(cut));
        assert!(cut.chars().count() <= 2);
    }

    #[test]
    fn inline_tokens_ignores_by_reference() {
        let list = vec![
            att("a.txt", "abcdefgh", AttachMode::Inline), // 2
            att("b.txt", "abcdefgh", AttachMode::ByReference),
            att("c.txt", "abcd", AttachMode::Inline), // 1
        ];
        assert_eq!(inline_tokens(&list), 3);
    }

    #[test]
    fn format_bytes_switches_units() {
        assert_eq!(format_bytes(840), "840 B");
        assert_eq!(format_bytes(12_595), "12.3 KB");
        assert_eq!(format_bytes(1_468_006), "1.4 MB");
    }

    #[test]
    fn serde_roundtrip() {
        let a = att("n.md", "содержимое", AttachMode::ByReference);
        let json = serde_json::to_string(&a).unwrap();
        let back: Attachment = serde_json::from_str(&json).unwrap();
        assert_eq!(a, back);
        // The mode is a readable string in the file, not a number.
        assert!(json.contains("by_reference"), "{json}");
    }
}

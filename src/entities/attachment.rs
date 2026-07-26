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
    /// of KB through the event channel). `prompt_tokens` is computed here because
    /// only the caller (the orchestrator) knows the budget settings.
    pub fn info(&self, excerpt_tokens: usize) -> AttachmentInfo {
        AttachmentInfo {
            name: self.name.clone(),
            bytes: self.bytes,
            est_tokens: self.est_tokens,
            prompt_tokens: self.prompt_tokens(excerpt_tokens),
            mode: self.mode,
        }
    }

    /// What this attachment actually costs in **every** request: the whole text
    /// when inline, only the excerpt when by reference. Reporting a by-reference
    /// file as free would be a lie — its excerpt is re-sent every turn.
    pub fn prompt_tokens(&self, excerpt_tokens: usize) -> usize {
        match self.mode {
            AttachMode::Inline => self.est_tokens,
            AttachMode::ByReference => estimate_text(self.excerpt(excerpt_tokens)) as usize,
        }
    }

    /// How many pages of `page_tokens` the text splits into (at least 1) — the
    /// range `attachment_read` accepts.
    pub fn page_count(&self, page_tokens: usize) -> usize {
        paginate(&self.text, page_tokens).len()
    }

    /// Page `n` (1-based) of the text, or `None` when out of range.
    pub fn page(&self, page_tokens: usize, n: usize) -> Option<&str> {
        let pages = paginate(&self.text, page_tokens);
        n.checked_sub(1).and_then(|i| pages.get(i)).copied()
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
    /// Estimated tokens of the whole file.
    pub est_tokens: usize,
    /// What it costs per request (see [`Attachment::prompt_tokens`]).
    pub prompt_tokens: usize,
    pub mode: AttachMode,
}

/// Splits text into pages of `page_tokens` (estimated) for `attachment_read`.
/// Cuts on a **line** boundary where possible (a page that starts mid-sentence
/// is hard to read), falling back to a whitespace and then a character boundary.
/// Never splits a character. Empty text yields one empty page, so page counts
/// are always ≥1 and `page 1` always exists.
pub fn paginate(text: &str, page_tokens: usize) -> Vec<&str> {
    let budget = byte_budget(page_tokens);
    let mut pages = Vec::new();
    let mut rest = text;
    while rest.len() > budget {
        let cut = cut_point(rest, budget);
        pages.push(&rest[..cut]);
        rest = &rest[cut..];
    }
    pages.push(rest);
    pages
}

/// The leading `max_tokens` (estimated) of `text` — see [`Attachment::excerpt`].
/// A free function so it can be tested without building an attachment. Shares
/// [`cut_point`] with [`paginate`], so an excerpt and a first page break at the
/// same place.
pub fn excerpt(text: &str, max_tokens: usize) -> &str {
    &text[..cut_point(text, byte_budget(max_tokens))]
}

/// The estimate is "UTF-8 bytes / 4" (`shared::tokens`), so a token budget
/// converts to bytes by multiplying. At least one token, so a cut always advances.
fn byte_budget(tokens: usize) -> usize {
    tokens.max(1).saturating_mul(4)
}

/// Where to cut `text` so the head fits `budget` bytes: on a **line** boundary
/// where possible (a fragment that starts mid-sentence is hard to read), else on
/// a whitespace, else on a character boundary. Never splits a character. The
/// separator ends the **current** fragment (the cut is *after* it) — cutting
/// before would push it to the head of the next one and shave a word off this
/// one. A boundary inside the first quarter is ignored: it would waste most of
/// the budget. Returns `text.len()` when the whole text fits.
fn cut_point(text: &str, budget: usize) -> usize {
    if text.len() <= budget {
        return text.len();
    }
    // Floor to a character boundary first — everything below indexes bytes.
    let mut end = budget;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let head = &text[..end];
    let floor = end / 4;
    let after = |i: usize, c: char| i + c.len_utf8();
    head.rfind('\n')
        .map(|i| after(i, '\n'))
        .filter(|&c| c > floor)
        .or_else(|| {
            head.char_indices()
                .rev()
                .find(|(_, c)| c.is_whitespace())
                .map(|(i, c)| after(i, c))
                .filter(|&c| c > floor)
        })
        .unwrap_or(end)
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

/// Total estimated tokens of the attachments rendered **inline** — the figure
/// the **budget** is measured against (`max_total_tokens` governs how much full
/// text a chat may carry; by-reference excerpts are bounded and small, so they
/// don't consume that budget). For what the user is shown, use
/// [`prompt_tokens`].
pub fn inline_tokens(attachments: &[Attachment]) -> usize {
    attachments
        .iter()
        .filter(|a| a.mode == AttachMode::Inline)
        .map(|a| a.est_tokens)
        .sum()
}

/// What the whole attachment set actually costs in **every** request — inline
/// texts in full plus by-reference excerpts. This is the number to **show**:
/// reporting a by-reference file as free would be a lie.
pub fn prompt_tokens(attachments: &[Attachment], excerpt_tokens: usize) -> usize {
    attachments
        .iter()
        .map(|a| a.prompt_tokens(excerpt_tokens))
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
        assert!(cut.len() <= 8, "the excerpt fits the byte budget: {cut:?}");
        // The cut lands on a word boundary and **keeps** the separator: the
        // shared `cut_point` ends a fragment with it, so pagination doesn't push
        // it onto the next page. Harmless for an excerpt — a newline follows it
        // in the prompt anyway.
        assert!(
            cut.ends_with(char::is_whitespace),
            "cut on a word boundary: {cut:?}"
        );
        // Page 1 breaks in exactly the same place — both go through `cut_point`.
        assert_eq!(cut, paginate(long, 2)[0]);
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
    fn pagination_covers_the_text_exactly_and_prefers_line_breaks() {
        let text = "строка один\nстрока два\nстрока три\nстрока четыре\nстрока пять\n";
        let pages = paginate(text, 5); // ≈20 bytes per page
        assert!(pages.len() > 1, "the text must split: {pages:?}");
        // Lossless: concatenating the pages reproduces the source byte for byte.
        assert_eq!(pages.concat(), text);
        // Most cuts land right after a line break.
        assert!(
            pages[..pages.len() - 1]
                .iter()
                .all(|p| p.ends_with('\n') || p.ends_with(' ')),
            "pages should end on a line/word boundary: {pages:?}"
        );
    }

    #[test]
    fn short_and_empty_text_is_a_single_page() {
        assert_eq!(paginate("short", 100), vec!["short"]);
        assert_eq!(paginate("", 100), vec![""]);
        let a = att("a.txt", "", AttachMode::ByReference);
        assert_eq!(a.page_count(100), 1, "page 1 always exists");
        assert_eq!(a.page(100, 1), Some(""));
        assert_eq!(a.page(100, 0), None, "pages are 1-based");
        assert_eq!(a.page(100, 2), None);
    }

    #[test]
    fn pagination_never_splits_a_character() {
        // Cyrillic (2 bytes/char) with an odd byte budget — a naive slice would panic.
        let text = "абвгдеёжзийклмнопрстуфхцч".repeat(4);
        let pages = paginate(&text, 3); // 12 bytes — not a multiple of the char size
        assert_eq!(pages.concat(), text);
        assert!(pages.iter().all(|p| p.chars().count() * 2 == p.len()));
    }

    #[test]
    fn prompt_cost_counts_the_excerpt_for_by_reference_not_the_whole_file() {
        let big = "слово ".repeat(2000); // ~3000 tokens
        let inline = att("a.txt", &big, AttachMode::Inline);
        let by_ref = att("b.txt", &big, AttachMode::ByReference);
        assert_eq!(inline.prompt_tokens(50), inline.est_tokens);
        let cost = by_ref.prompt_tokens(50);
        assert!(cost <= 50, "a by-reference file costs its excerpt: {cost}");
        assert!(cost > 0, "and it is NOT free: {cost}");
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

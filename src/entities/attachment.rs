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
    /// The chat file holding the **original bytes**, when `/file attach` kept them beside
    /// the extracted text — a pdf, a docx, an html page (fork F8a,
    /// docs/sandbox-file-exchange.md §12 T9). `None` when the text *is* the file. The two
    /// halves are one item wherever files are listed, named or removed. Additive — old
    /// chats read without migration (ADR 0006 F12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<Uuid>,
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
            file_id: None,
        }
    }

    /// The same attachment with the chat file that holds its original bytes (F8a).
    pub fn with_file(mut self, file_id: Uuid) -> Self {
        self.file_id = Some(file_id);
        self
    }

    /// A card for the UI (no text — the feed/status bar must not carry hundreds
    /// of KB through the event channel). `prompt_tokens` is computed here because
    /// only the caller (the orchestrator) knows the budget settings.
    pub fn info(&self, excerpt_tokens: usize) -> AttachmentInfo {
        AttachmentInfo {
            name: self.name.clone(),
            source: self.source.clone(),
            bytes: self.bytes,
            est_tokens: self.est_tokens,
            prompt_tokens: self.prompt_tokens(excerpt_tokens),
            mode: self.mode,
            has_original: self.file_id.is_some(),
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

/// One indexed fragment of a by-reference attachment — the chat-scoped semantic
/// index (stage 3, docs/file-attachments.md §4.5). Mirrors
/// [`RagDocument`](crate::entities::rag::RagDocument), but scoped by **chat**
/// instead of profile: an attachment belongs to exactly one conversation.
#[derive(Debug, Clone, PartialEq)]
pub struct AttachmentChunk {
    pub id: Uuid,
    pub chat_id: Uuid,
    pub attachment_id: Uuid,
    /// Display name of the source attachment. Denormalized on purpose: the DB
    /// knows nothing about chats, and search output has to name the file.
    pub name: String,
    pub text: String,
    pub embedding: Vec<f32>,
    pub created_at: DateTime<Utc>,
}

impl AttachmentChunk {
    pub fn new(
        chat_id: Uuid,
        attachment_id: Uuid,
        name: impl Into<String>,
        text: impl Into<String>,
        embedding: Vec<f32>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            chat_id,
            attachment_id,
            name: name.into(),
            text: text.into(),
            embedding,
            created_at: Utc::now(),
        }
    }
}

/// A hit from the chat's attachment index (`attachment_search`).
#[derive(Debug, Clone, PartialEq)]
pub struct AttachmentHit {
    pub attachment_id: Uuid,
    pub name: String,
    pub text: String,
    pub distance: f32,
}

/// An attachment card for the UI (`/file list`, the status-bar chip).
#[derive(Debug, Clone, PartialEq)]
pub struct AttachmentInfo {
    pub name: String,
    /// The canonical path, or a tool's source — shown on a `/file list` line whose name
    /// another attachment shares, since that is what tells the two apart.
    pub source: String,
    pub bytes: usize,
    /// Estimated tokens of the whole file.
    pub est_tokens: usize,
    /// What it costs per request (see [`Attachment::prompt_tokens`]).
    pub prompt_tokens: usize,
    pub mode: AttachMode,
    /// Whether the chat also keeps this file's **original bytes** ([`Attachment::file_id`],
    /// fork F8a): the listing says so, since that is the half `python_exec` reads and the
    /// half a removal deletes from disk.
    pub has_original: bool,
}

/// What a handle typed after `remove` — `#N`, a name or a path — reaches in a listed set
/// (`/file remove`, `/image remove`; docs/research/remove-by-shared-name.md).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// One item: its position.
    One(usize),
    /// A name several items share: their positions, in listing order. A removal refuses
    /// it rather than guess which one was meant (fork F1a).
    Shared(Vec<usize>),
    /// Nothing.
    Nothing,
}

/// Resolves `#N` (1-based, as the listings number items), which is never ambiguous, or
/// else every item `matches` accepts for the name or path typed.
pub fn resolve_handle<T>(
    items: &[T],
    target: &str,
    matches: impl Fn(&T, &str) -> bool,
) -> Resolved {
    let target = target.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    if let Some(digits) = target.strip_prefix('#')
        && let Ok(n) = digits.trim().parse::<usize>()
    {
        return match n.checked_sub(1).filter(|&i| i < items.len()) {
            Some(i) => Resolved::One(i),
            None => Resolved::Nothing,
        };
    }
    let hits: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| matches(*item, target))
        .map(|(i, _)| i)
        .collect();
    match hits.len() {
        0 => Resolved::Nothing,
        1 => Resolved::One(hits[0]),
        _ => Resolved::Shared(hits),
    }
}

/// Whether the item at `i` has a name another item shares — where a listing shows the
/// source and a removal note names it (fork F2a). Names compare as `matches` compares them.
pub fn name_is_shared<T>(items: &[T], i: usize, name: impl Fn(&T) -> &str) -> bool {
    let Some(own) = items.get(i).map(&name) else {
        return false;
    };
    items
        .iter()
        .enumerate()
        .any(|(j, item)| j != i && name(item).eq_ignore_ascii_case(own))
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

/// How a new attachment of `est` estimated tokens should reach the model, given
/// what the chat already spends inline (`used`) and the budget.
///
/// A pure function rather than a rule written twice: the orchestrator decides
/// this for `/file attach`, and a tool that produces its own attachment (a video
/// transcript, docs/history/youtube-transcript.md §3 F2) has to reach the same answer —
/// it tells the model what it did before the orchestrator persists it. Attaching
/// never *fails* on size; over the budget simply means by reference.
pub fn decide_mode(
    est: usize,
    used: usize,
    cfg: &crate::shared::config::AttachmentSettings,
) -> AttachMode {
    if est <= cfg.max_file_tokens && used + est <= cfg.max_total_tokens {
        AttachMode::Inline
    } else {
        AttachMode::ByReference
    }
}

/// Total estimated tokens of the attachments rendered **inline** — the figure
/// the **budget** is measured against (`max_total_tokens` governs how much full
/// text a chat may carry; by-reference excerpts are bounded and small, so they
/// don't consume that budget). For what the user is shown, use
/// [`prompt_tokens`].
///
/// `replacing` names the source about to be **replaced**, whose cost therefore
/// does not count: attaching the same file twice drops the old copy, and
/// counting it against the new one would push a re-attach by reference for no
/// reason. Pass `""` when nothing is being replaced.
pub fn inline_tokens_excluding(attachments: &[Attachment], source: &str) -> usize {
    attachments
        .iter()
        .filter(|a| a.source != source && a.mode == AttachMode::Inline)
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

    /// docs/research/remove-by-shared-name.md: `#N` and a path each reach one item, a name
    /// two items share reaches both, and nothing is picked for the caller.
    #[test]
    fn a_handle_resolves_to_one_item_to_every_holder_of_a_shared_name_or_to_nothing() {
        let file = |dir: &str, name: &str| {
            Attachment::new(
                name,
                format!("/tmp/{dir}/{name}"),
                "x".into(),
                1,
                AttachMode::Inline,
            )
        };
        let items = vec![
            file("a", "notes.md"),
            file("b", "Notes.md"),
            file("a", "todo.txt"),
        ];
        let resolve = |target: &str| resolve_handle(&items, target, Attachment::matches);
        assert_eq!(resolve("#2"), Resolved::One(1), "#N is never ambiguous");
        assert_eq!(
            resolve("\"#2\""),
            Resolved::One(1),
            "quoted, it is still #N"
        );
        assert_eq!(
            resolve("/tmp/b/Notes.md"),
            Resolved::One(1),
            "nor is a path"
        );
        assert_eq!(resolve("todo.txt"), Resolved::One(2));
        assert_eq!(resolve("NOTES.MD"), Resolved::Shared(vec![0, 1]));
        assert_eq!(resolve(" \"notes.md\" "), Resolved::Shared(vec![0, 1]));
        for nothing in ["#0", "#4", "#x", "other.md", ""] {
            assert_eq!(resolve(nothing), Resolved::Nothing, "{nothing:?}");
        }

        let shared: Vec<bool> = (0..items.len())
            .map(|i| name_is_shared(&items, i, |a| a.name.as_str()))
            .collect();
        assert_eq!(shared, [true, true, false]);
        assert!(!name_is_shared(&items, 9, |a| a.name.as_str()));
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
    fn inline_tokens_ignores_by_reference_and_the_source_being_replaced() {
        let list = vec![
            att("a.txt", "abcdefgh", AttachMode::Inline), // 2
            att("b.txt", "abcdefgh", AttachMode::ByReference),
            att("c.txt", "abcd", AttachMode::Inline), // 1
        ];
        assert_eq!(inline_tokens_excluding(&list, ""), 3);
        // Re-attaching a.txt frees its budget before the new copy is measured.
        assert_eq!(inline_tokens_excluding(&list, "/tmp/a.txt"), 1);
    }

    #[test]
    fn mode_is_by_reference_past_either_budget() {
        let cfg = crate::shared::config::AttachmentSettings {
            max_file_tokens: 100,
            max_total_tokens: 150,
            ..Default::default()
        };
        assert_eq!(decide_mode(100, 0, &cfg), AttachMode::Inline);
        // Over the per-file budget…
        assert_eq!(decide_mode(101, 0, &cfg), AttachMode::ByReference);
        // …or over what is left of the chat's total.
        assert_eq!(decide_mode(60, 100, &cfg), AttachMode::ByReference);
        assert_eq!(decide_mode(50, 100, &cfg), AttachMode::Inline);
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

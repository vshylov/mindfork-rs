//! An own multiline input editor (see ADR 0001: `tui-textarea` is incompatible
//! with ratatui 0.30, and an own widget gives control over `Shift+Enter`,
//! scrolling, and — later — spellcheck-error highlighting). See spec §11.5.
//!
//! Stores lines as `Vec<Vec<char>>`: the cursor index is a character index, no
//! worrying about UTF-8 boundaries. The calling layer decides the policy
//! "`Enter` sends / `Shift+Enter` breaks the line"; the widget only edits.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::shared::keys;
use crate::shared::theme::Palette;
use crate::shared::ui::render_scrollbar;
use crate::shared::wrap;

/// Width of the `❯ ` prompt column (in columns) before the input text.
const PROMPT_W: u16 = 2;

/// One visual row: `(logical line, start, end)` in the line's character indices
/// (accounting for width-based wrapping). See [`InputBox::visual_rows`].
type VisualRow = (usize, usize, usize);

/// Cache of visual rows: `(width, content revision, rows)`. See
/// [`InputBox::rows_cache`].
type RowCache = Option<(usize, u64, Vec<VisualRow>)>;

/// Ceiling on the undo-stack depth (units). The oldest ones get evicted.
const UNDO_CAP: usize = 200;

/// Masked-mode character (a secret input). Width — 1 column (WGL4-safe, needs no
/// separate substitution in old-terminal compatibility mode).
const MASK_CHAR: char = '•';

/// A content snapshot for undo/redo: lines + cursor position. See
/// [`InputBox::record_undo`], docs/history/input-selection-undo-mouse.md §C.
#[derive(Clone)]
struct Snapshot {
    lines: Vec<Vec<char>>,
    row: usize,
    col: usize,
}

/// Edit class for undo coalescing: consecutive edits of the same class (except
/// `Structural`) merge into one undo unit; a class change, navigation, or a
/// structural edit starts a new one. See [`InputBox::record_undo`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    /// Typing characters (coalesces; a break on whitespace — word-granular undo).
    Insert,
    /// Deletion (`Backspace`/`Delete`/word) — coalesces.
    Delete,
    /// A structural edit (a line break, insertion, replacing a range/selection,
    /// `Ctrl+K` clearing) — always its own undo unit.
    Structural,
}

/// A multiline input field with a cursor.
pub struct InputBox {
    /// Logical lines (characters). Always non-empty (at least one line).
    lines: Vec<Vec<char>>,
    /// Cursor row.
    row: usize,
    /// Cursor column (a character index into the row; may equal the row's length).
    col: usize,
    /// First visible row (vertical scroll).
    scroll: usize,
    /// Single-line mode: the value doesn't word-wrap, it **scrolls
    /// horizontally**; `↑/↓` and line breaks are disabled; `Home/End` — to the
    /// start/end of the logical line. For editable settings fields where the
    /// value is logically one line (a URL, a path, a number). Off by default —
    /// chat input stays multiline. See spec §11.6.
    single_line: bool,
    /// Masked mode (a secret input: an API key): every character renders as
    /// [`MASK_CHAR`], and [`Self::selected_text`] doesn't hand the content out.
    /// Enabling it switches the field to single-line mode (a secret is one line)
    /// and guarantees the mask can't be bypassed via the multiline render path.
    /// Spellcheck doesn't apply to such a field (asterisks aren't words).
    /// See [`Self::set_mask`], spec §11.6.
    mask: bool,
    /// Horizontal scroll in columns (single-line mode only): the first visible
    /// column. Keeps the cursor in the visible area, mirroring vertical `scroll`.
    hscroll: usize,
    /// Width of the inner area from the last render (in columns). Needed by
    /// `↑/↓` navigation to walk **visual** rows of a wrapped line, not logical
    /// lines (wrapping is only computed at render time). `0` — no render yet:
    /// then `↑/↓` fall back to a logical transition. See [`Self::move_up`].
    last_width: usize,
    /// The "goal" visual column of an `↑/↓` run (in columns). Remembered on the
    /// first vertical move and held while the cursor isn't moved otherwise — so a
    /// run of `↑/↓` through short rows keeps the original column (as in large
    /// editors). Any horizontal move/edit resets it to `None`. See [`Self::move_up`].
    goal_col: Option<usize>,
    /// Spelling-error word ranges by logical line (line index → sorted
    /// non-overlapping `[start, end)` in characters). Filled by the screen from
    /// the spellchecker; the widget only underlines. See spec §11.5.
    misspelled: Vec<Vec<(usize, usize)>>,
    /// Undo stack: content snapshots **before** the edit (coalesced by edit class,
    /// see [`Self::record_undo`]). `Ctrl+Z` restores the top one. Ceiling [`UNDO_CAP`].
    undo: Vec<Snapshot>,
    /// Redo stack: snapshots popped from `undo` on an undo; cleared by any new
    /// edit. `Ctrl+Y` restores the top one.
    redo: Vec<Snapshot>,
    /// The last edit's class — for coalescing undo units. Reset by navigation/
    /// selection/undo (then the next edit starts a new unit). See
    /// [`Self::record_undo`].
    last_edit_kind: Option<EditKind>,
    /// Content-revision counter: incremented on any edit to `lines`
    /// ([`Self::touch`]). Together with the width — the visual-row cache key: if
    /// the revision and width haven't changed, wrapping isn't recomputed.
    revision: u64,
    /// Cache of visual rows `(width, revision, rows)`. Wrapping (`visual_rows`,
    /// O(n) over characters) is needed 2-3 times per frame (height via
    /// [`Self::content_rows`], [`Self::render`] itself, `↑/↓` navigation), but
    /// only changes on an edit or a width change. Invalidated by
    /// `(width, revision)`. See [`Self::rows_cached`].
    rows_cache: RowCache,
    /// Selection anchor `(row, column)` in character indices. `Some` — an active
    /// selection `[anchor, cursor]` exists (normalized on use: the start = the
    /// smaller position in lexicographic `(row, column)` order). `None` — no
    /// selection; the cursor is the existing `(row, col)`. Moving with `Shift`
    /// sets the anchor and grows the selection, a plain move clears it; any
    /// content edit clears it (via [`Self::touch`]). See [`Self::selection_span`],
    /// docs/history/input-selection-undo-mouse.md.
    anchor: Option<(usize, usize)>,
    /// The text's inner area from the last render (after the border and the `❯`
    /// column). Needed by mouse-click mapping (screen coordinates → a text
    /// position): a left click/drag places the cursor / grows the selection
    /// (with mouse capture `Ctrl+W`). `None` before the first render. See
    /// [`Self::place_cursor_at`], track stage D.
    last_area: Option<Rect>,
}

impl Default for InputBox {
    fn default() -> Self {
        Self::new()
    }
}

/// Outcome of the input box handling a key press ([`InputBox::on_key`]):
/// distinguishes **editing content** from a mere **cursor/scroll move**. The
/// caller uses it to decide whether to mark the input "dirty" (saving the
/// draft and rechecking spellcheck). Previously `on_key` returned a `bool`
/// ("handled or not"), and any cursor move needlessly raised the spellcheck
/// debounce and sent `SetDraft` to disk. See spec §11.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOutcome {
    /// Content changed.
    Edited,
    /// Cursor/scroll moved, content unchanged.
    Moved,
    /// The key wasn't handled (the caller decides what to do with it).
    Ignored,
}

impl KeyOutcome {
    /// The key was handled by the widget (the caller doesn't process it
    /// further). Currently only tests need this — production call sites branch
    /// on [`Self::edited`]; kept as its counterpart in the widget's public API.
    #[allow(dead_code)]
    pub fn handled(self) -> bool {
        !matches!(self, KeyOutcome::Ignored)
    }

    /// The edit changed content (needs a "dirty input" mark / a spellcheck
    /// recheck); a cursor move and an unhandled key don't.
    pub fn edited(self) -> bool {
        matches!(self, KeyOutcome::Edited)
    }
}

/// Input-box render parameters — a bundle instead of six positional arguments
/// (a precedent — `StatusModel`, SOLID stage 4a). `title` — the border's title;
/// `focused` places the cursor and (on an empty field) hides the placeholder;
/// `command` colors the whole text `warning` and mutes spellcheck underlines
/// (the input is recognized as a `/rag …` command); `placeholder` — gray text on
/// an empty, **un**focused field. The placeholder used to be hardcoded ("type a
/// message…") straight into the generic widget — semantically foreign for
/// settings/rename/search fields (they were only saved by always being
/// `focused`, so the placeholder never rendered). See audit item 12, spec §11.5.
pub struct RenderOpts<'a> {
    pub title: &'a str,
    pub focused: bool,
    pub command: bool,
    pub placeholder: &'a str,
}

impl<'a> RenderOpts<'a> {
    /// A focused field with no placeholder or command highlighting — the typical
    /// case for modal fields (chat rename, the settings/"self-model" editor,
    /// search): they're always `focused`, don't recognize commands, and a
    /// placeholder is semantically foreign to them.
    pub fn focused(title: &'a str) -> Self {
        Self {
            title,
            focused: true,
            command: false,
            placeholder: "",
        }
    }
}

impl InputBox {
    pub fn new() -> Self {
        Self {
            lines: vec![Vec::new()],
            row: 0,
            col: 0,
            scroll: 0,
            single_line: false,
            mask: false,
            hscroll: 0,
            last_width: 0,
            goal_col: None,
            misspelled: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit_kind: None,
            revision: 0,
            rows_cache: None,
            anchor: None,
            last_area: None,
        }
    }

    /// Enables single-line mode (horizontal scroll instead of wrapping; `↑/↓` and
    /// a line break are disabled). Usually called **before** [`Self::set_text`],
    /// but the "one logical line" invariant holds even when enabling it on
    /// already-multiline content: the lines collapse into one via a space, the
    /// cursor is clamped. Otherwise [`Self::render_single_line`] would take
    /// `lines[0]`, and `col` could point past its length — a panic on the slice.
    /// See field [`Self::single_line`].
    pub fn set_single_line(&mut self, on: bool) {
        self.single_line = on;
        if on && self.lines.len() > 1 {
            let mut merged: Vec<char> = Vec::new();
            for (idx, line) in self.lines.iter().enumerate() {
                if idx > 0 {
                    merged.push(' ');
                }
                merged.extend(line.iter().copied());
            }
            self.col = self.col.min(merged.len());
            self.lines = vec![merged];
            self.row = 0;
            self.scroll = 0;
            self.hscroll = 0;
            self.goal_col = None;
            self.touch();
        }
    }

    /// Enables masked mode (a secret input: an API key in settings): characters
    /// render as `•`, [`Self::selected_text`] is empty (a secret can't leak via
    /// copying). Also enables single-line mode — otherwise the multiline render
    /// path would show the content in the open. Editing/navigation/paste work as
    /// usual. See field [`Self::mask`], spec §11.6.
    pub fn set_mask(&mut self, on: bool) {
        self.mask = on;
        if on {
            self.set_single_line(true);
        }
    }

    /// Is the input masked (secret mode). Consumers — tests (checking the key
    /// field opened masked); in production paths the mask is only ever set.
    #[allow(dead_code)]
    pub fn is_masked(&self) -> bool {
        self.mask
    }

    /// The field's text (lines joined by `\n`).
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Is the field empty (one empty line).
    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    /// The value's first non-whitespace character (over logical lines), if any.
    /// A cheap "looks like a command" check with no allocation of the whole
    /// text: the calling layer first looks for `Some('/')` and only then parses
    /// the full [`Self::text`]. Stops at the first non-whitespace character. See
    /// `ChatScreen::input_is_command`.
    pub fn first_non_whitespace(&self) -> Option<char> {
        self.lines
            .iter()
            .flat_map(|l| l.iter())
            .copied()
            .find(|c| !c.is_whitespace())
    }

    /// Does the field have a character matching the predicate. Streaming, no
    /// `text()` build — called on every edit (see
    /// `ChatScreen::mark_input_changed`). The caller passes the predicate, so
    /// the widget doesn't know about upper layers (FSD).
    pub fn any_char(&self, pred: impl Fn(char) -> bool) -> bool {
        self.lines.iter().flat_map(|l| l.iter()).copied().any(pred)
    }

    /// Number of logical lines. Field height is now computed by
    /// [`Self::visual_line_count`] (accounts for wrapping); the method is kept
    /// as a natural companion and for tests.
    #[allow(dead_code)]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Number of visual rows at width `width` (accounting for wrapping). The
    /// production field-height path now uses [`Self::content_rows`] (it
    /// subtracts the border and prompt column itself); the method is kept for
    /// tests as a thin wrapper over [`Self::visual_rows`].
    #[cfg(test)]
    pub fn visual_line_count(&self, width: usize) -> usize {
        self.visual_rows(width).len()
    }

    /// Number of visual content rows when rendering into an area of **outer**
    /// width `area_width` (including the border). Subtracts the border (2) and
    /// the prompt column ([`PROMPT_W`]) — the exact same text width
    /// [`Self::render`] uses. The layer above computes the field's height via
    /// this method so it matches the real wrapping: otherwise computing height
    /// from "width minus border" would overstate the available width by
    /// [`PROMPT_W`], and the field wouldn't grow by one-two characters past the
    /// wrap boundary (the cursor would pin to the edge, see spec §11.5). In
    /// single-line mode wrapping is off — always one row.
    pub fn content_rows(&mut self, area_width: u16) -> usize {
        if self.single_line {
            return 1;
        }
        let text_w = area_width.saturating_sub(2).saturating_sub(PROMPT_W).max(1) as usize;
        self.rows_cached(text_w).len()
    }

    /// Clears the field **programmatically** (sending a message, a reset).
    /// Unlike [`Self::clear_undoable`] — **clears the undo history** (after
    /// sending/loading someone else's text, `Ctrl+Z` shouldn't resurrect the
    /// previous context).
    pub fn clear(&mut self) {
        self.lines = vec![Vec::new()];
        self.row = 0;
        self.col = 0;
        self.scroll = 0;
        self.hscroll = 0;
        self.goal_col = None;
        self.misspelled.clear();
        self.undo.clear();
        self.redo.clear();
        self.last_edit_kind = None;
        self.touch();
    }

    /// Hotkey "delete all input text" (`Ctrl+K`, spec §11.5). Records a snapshot
    /// into the undo history and clears the content, **without** touching the
    /// history — so `Ctrl+Z` brings the text back (a shared undo model; the
    /// previous toggle semantics were removed, see
    /// docs/history/input-selection-undo-mouse.md §C). An empty field — a no-op.
    pub fn clear_undoable(&mut self) {
        if self.is_empty() {
            return;
        }
        self.record_undo(EditKind::Structural); // a snapshot of non-empty content
        self.lines = vec![Vec::new()];
        self.row = 0;
        self.col = 0;
        self.scroll = 0;
        self.hscroll = 0;
        self.goal_col = None;
        self.misspelled.clear();
        self.touch(); // revision + clear selection (does NOT clear history)
    }

    /// Text by logical lines (for line-by-line spellcheck).
    pub fn line_strings(&self) -> Vec<String> {
        self.lines.iter().map(|l| l.iter().collect()).collect()
    }

    /// Cursor position `(row, column)` in character indices.
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    /// Sets the spelling-error ranges (by line). See [`Self::misspelled`].
    pub fn set_misspelled(&mut self, ranges: Vec<Vec<(usize, usize)>>) {
        self.misspelled = ranges;
    }

    /// Syncs line `row`'s error ranges with an edit to its content: `removed`
    /// characters were deleted and `inserted` were inserted at position `at`.
    /// Ranges wholly to the left of the edit aren't touched; wholly to the right
    /// — shift by the delta; ones intersecting the changed span — are reset (the
    /// word changed — let the recheck re-evaluate it). This keeps underlines in
    /// place between debounced rechecks, not "drifting" onto neighboring words
    /// (otherwise the error style would highlight different text). See spec
    /// §11.5.
    fn edit_misspelled(&mut self, row: usize, at: usize, removed: usize, inserted: usize) {
        let Some(ranges) = self.misspelled.get_mut(row) else {
            return;
        };
        let end = at + removed;
        let delta = inserted as isize - removed as isize;
        ranges.retain_mut(|(s, e)| {
            if *e <= at {
                true // wholly to the left — unchanged
            } else if *s >= end {
                *s = (*s as isize + delta).max(0) as usize; // wholly to the right — shift
                *e = (*e as isize + delta).max(0) as usize;
                *e > *s
            } else {
                false // intersects the edit — reset
            }
        });
    }

    /// Are there no flagged spelling errors (for the layer above's tests).
    #[cfg(test)]
    pub fn misspelled_is_empty(&self) -> bool {
        self.misspelled.iter().all(|r| r.is_empty())
    }

    /// Spelling-error ranges of line `row` (for edit-sync tests).
    #[cfg(test)]
    fn misspelled_ranges_for_test(&self, row: usize) -> Vec<(usize, usize)> {
        self.misspelled.get(row).cloned().unwrap_or_default()
    }

    /// Current horizontal scroll (in columns) — for the character-boundary
    /// alignment test in single-line mode.
    #[cfg(test)]
    fn hscroll_for_test(&self) -> usize {
        self.hscroll
    }

    /// Replaces the character range `[start, end)` in line `row` with
    /// `replacement` and places the cursor after the inserted text (for
    /// applying a suggestion).
    pub fn replace_range(&mut self, row: usize, start: usize, end: usize, replacement: &str) {
        if row >= self.lines.len() {
            return;
        }
        self.record_undo(EditKind::Structural); // before the mutation (a replace is its own unit)
        let line = &mut self.lines[row];
        let end = end.min(line.len());
        let start = start.min(end);
        let repl: Vec<char> = replacement.chars().collect();
        let repl_len = repl.len();
        line.splice(start..end, repl);
        self.edit_misspelled(row, start, end - start, repl_len);
        self.row = row;
        self.col = start + repl_len;
        self.goal_col = None;
        self.touch();
    }

    /// Fills the field with text, places the cursor at the end (for in-place
    /// editing, M3+).
    pub fn set_text(&mut self, text: &str) {
        // In single-line mode we keep the "one logical line" invariant: line
        // breaks collapse into a space.
        let owned;
        let text = if self.single_line && text.contains('\n') {
            owned = text.replace('\n', " ");
            owned.as_str()
        } else {
            text
        };
        self.lines = if text.is_empty() {
            vec![Vec::new()]
        } else {
            text.split('\n').map(|l| l.chars().collect()).collect()
        };
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].len();
        self.scroll = 0;
        self.hscroll = 0;
        self.goal_col = None;
        // A programmatic text replacement (loading someone else's chat draft,
        // restore_input) — clears the undo history: `Ctrl+Z` shouldn't resurrect
        // someone else's context.
        self.undo.clear();
        self.redo.clear();
        self.last_edit_kind = None;
        // The old error ranges referred to the previous text — reset (otherwise
        // underlines would be drawn on the new content until the next recheck).
        self.misspelled.clear();
        self.touch();
    }

    // ---------- editing ----------

    pub fn insert_char(&mut self, c: char) {
        self.record_undo(EditKind::Insert);
        self.remove_selection(); // typing over a selection replaces it (undo already recorded)
        self.lines[self.row].insert(self.col, c);
        self.edit_misspelled(self.row, self.col, 0, 1);
        self.col += 1;
        self.goal_col = None;
        self.touch();
        // A space ends the undo unit (word-granular): the next run of typing is a new unit.
        if c.is_whitespace() {
            self.last_edit_kind = None;
        }
    }

    pub fn insert_newline(&mut self) {
        if self.single_line {
            return; // a line break is forbidden in single-line mode
        }
        self.record_undo(EditKind::Structural);
        self.remove_selection(); // a line break over a selection replaces it
        let tail = self.lines[self.row].split_off(self.col);
        self.lines.insert(self.row + 1, tail);
        // Sync underlines: reset the current line (its tail moved to the new
        // one), insert an empty range set for the new line.
        if self.row < self.misspelled.len() {
            self.misspelled[self.row].clear();
            self.misspelled.insert(self.row + 1, Vec::new());
        }
        self.row += 1;
        self.col = 0;
        self.goal_col = None;
        self.touch();
    }

    /// Inserts arbitrary text at the cursor position (a clipboard paste). Line
    /// breaks (`\n`) split the current logical line into new ones; `\r` is
    /// normalized (`\r\n`/`\r` → `\n`), `\t` expands into spaces. The cursor
    /// lands at the end of what was inserted. One pass, no per-character loop —
    /// so a large paste doesn't lag (see bracketed paste, spec §11.5).
    pub fn insert_str(&mut self, text: &str) {
        self.record_undo(EditKind::Structural);
        self.remove_selection(); // pasting over a selection replaces it
        // The current line's tail after the cursor — glue it to the last inserted line.
        let tail: Vec<char> = self.lines[self.row].split_off(self.col);
        // In single-line mode turn line breaks into spaces (one line).
        let normalized = normalize_paste(text);
        let normalized = if self.single_line {
            normalized.replace('\n', " ")
        } else {
            normalized
        };
        let mut first = true;
        for segment in normalized.split('\n') {
            if first {
                first = false;
            } else {
                // A new line break: start the next logical line.
                self.row += 1;
                self.lines.insert(self.row, Vec::new());
            }
            self.lines[self.row].extend(segment.chars());
        }
        self.col = self.lines[self.row].len();
        self.lines[self.row].extend(tail);
        self.goal_col = None;
        // A paste (clipboard) changes lines arbitrarily; simpler to reset all
        // underlines — a recheck (always triggered by `mark_input_changed` after
        // a paste) rebuilds them. See spec §11.5.
        self.misspelled.clear();
        self.touch();
    }

    pub fn backspace(&mut self) {
        // Nothing to delete (an empty prefix, no selection) — don't record an undo unit.
        if !self.has_selection() && self.col == 0 && self.row == 0 {
            return;
        }
        self.record_undo(EditKind::Delete);
        if self.remove_selection() {
            return; // with a selection, Backspace deletes it whole
        }
        self.goal_col = None;
        if self.col > 0 {
            // Delete the whole cluster (`❤️`/`👍🏽` — several scalars), not one
            // scalar — otherwise an orphaned selector/modifier remains. See spec §11.5.
            let start = wrap::prev_boundary(&self.lines[self.row], self.col);
            self.lines[self.row].drain(start..self.col);
            self.edit_misspelled(self.row, start, self.col - start, 0);
            self.col = start;
            self.touch();
        } else if self.row > 0 {
            // join with the previous line
            let current = self.lines.remove(self.row);
            self.join_misspelled_into_prev(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].len();
            self.lines[self.row].extend(current);
            self.touch();
        }
    }

    pub fn delete(&mut self) {
        // Nothing to delete (cursor at the very end, no selection) — don't record an undo unit.
        if !self.has_selection()
            && self.col >= self.lines[self.row].len()
            && self.row + 1 >= self.lines.len()
        {
            return;
        }
        self.record_undo(EditKind::Delete);
        if self.remove_selection() {
            return; // with a selection, Delete deletes it whole
        }
        self.goal_col = None;
        if self.col < self.lines[self.row].len() {
            // Delete the whole cluster (mirrors `backspace`), not one scalar.
            let end = wrap::next_boundary(&self.lines[self.row], self.col);
            self.lines[self.row].drain(self.col..end);
            self.edit_misspelled(self.row, self.col, end - self.col, 0);
            self.touch();
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.join_misspelled_into_prev(self.row + 1);
            self.lines[self.row].extend(next);
            self.touch();
        }
    }

    /// Syncs underlines when lines are joined: line `removed` moves into the
    /// previous one. Both lines' ranges refer to the old coordinates, so it's
    /// simpler to reset the remaining line's ranges (its content changes) and
    /// drop the record of the one that moved — a recheck rebuilds them.
    /// `removed` — the index of the line removed from [`Self::lines`]; the
    /// previous line is `removed - 1`.
    fn join_misspelled_into_prev(&mut self, removed: usize) {
        if removed < self.misspelled.len() {
            self.misspelled.remove(removed);
        }
        if removed > 0
            && let Some(r) = self.misspelled.get_mut(removed - 1)
        {
            r.clear();
        }
    }

    // ---------- selection ----------

    /// Is there a non-empty selection (the anchor is set and doesn't coincide
    /// with the cursor). Needed by consumers for copy/cut
    /// (docs/history/input-selection-undo-mouse.md §B).
    pub fn has_selection(&self) -> bool {
        matches!(self.anchor, Some(a) if a != (self.row, self.col))
    }

    /// The normalized selection range `(start, end)` in lexicographic
    /// `(row, column)` order. `None` if there's no selection (the anchor is
    /// unset or coincides with the cursor).
    fn selection_span(&self) -> Option<((usize, usize), (usize, usize))> {
        let a = self.anchor?;
        let c = (self.row, self.col);
        if a == c {
            return None;
        }
        Some(if a <= c { (a, c) } else { (c, a) })
    }

    /// The selection's text (lines joined by `\n`), or `None`. For copy/cut
    /// (consumers, docs/history/input-selection-undo-mouse.md §B).
    ///
    /// In masked mode (a secret input) always `None`: the content mustn't leak
    /// via copying to the clipboard. Deleting a selection still works — it goes
    /// through `remove_selection`, not through this method.
    pub fn selected_text(&self) -> Option<String> {
        if self.mask {
            return None;
        }
        let ((sr, sc), (er, ec)) = self.selection_span()?;
        let mut out = String::new();
        if sr == er {
            out.extend(self.lines[sr][sc..ec].iter().copied());
        } else {
            out.extend(self.lines[sr][sc..].iter().copied());
            out.push('\n');
            for line in &self.lines[sr + 1..er] {
                out.extend(line.iter().copied());
                out.push('\n');
            }
            out.extend(self.lines[er][..ec].iter().copied());
        }
        Some(out)
    }

    /// Sets the anchor to the current cursor if it isn't already set (before
    /// `Shift`-navigation — the start of a selection). An already-set anchor
    /// doesn't move (the selection grows from it).
    fn set_anchor_if_none(&mut self) {
        if self.anchor.is_none() {
            self.anchor = Some((self.row, self.col));
        }
    }

    /// Clears the selection (a plain move with no `Shift`; consumers — after
    /// copying via `Ctrl+C`).
    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    /// Selects all text (`Ctrl+A`): anchor — the start, cursor — the end.
    /// Resets undo coalescing (an edit after a selection is a new unit).
    fn select_all(&mut self) {
        self.anchor = Some((0, 0));
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].len();
        self.goal_col = None;
        self.last_edit_kind = None;
    }

    /// Deletes the selection as a **standalone** action (`Ctrl+X` cut): records
    /// a snapshot into the undo history, then deletes. Returns whether there
    /// was anything to delete. Internal mutators (`insert_char`/`backspace`/…)
    /// delete the selection via [`Self::remove_selection`] (with no recording —
    /// they already recorded their own snapshot).
    pub fn delete_selection(&mut self) -> bool {
        if !self.has_selection() {
            return false;
        }
        self.record_undo(EditKind::Structural);
        self.remove_selection()
    }

    /// Deletes the selected text, places the cursor at its start, clears the
    /// selection. Returns whether there was anything to delete. A multiline
    /// selection joins lines; spelling underlines are synced (as on
    /// deletion/joining, item 5). **Doesn't record** an undo snapshot — the
    /// caller does that (see [`Self::delete_selection`]).
    fn remove_selection(&mut self) -> bool {
        let Some(((sr, sc), (er, ec))) = self.selection_span() else {
            return false;
        };
        // Syncing `misspelled`: one line — shift/reset ranges; several — drop
        // the records of the middle/last lines and reset the first one's record.
        if sr == er {
            self.edit_misspelled(sr, sc, ec - sc, 0);
        } else {
            for _ in sr..er {
                if sr + 1 < self.misspelled.len() {
                    self.misspelled.remove(sr + 1);
                }
            }
            if let Some(m) = self.misspelled.get_mut(sr) {
                m.clear();
            }
        }
        // Join: `lines[sr][..sc]` + `lines[er][ec..]`, drop the middle lines.
        let tail: Vec<char> = self.lines[er][ec..].to_vec();
        self.lines[sr].truncate(sc);
        self.lines[sr].extend(tail);
        self.lines.drain((sr + 1)..=er);
        self.row = sr;
        self.col = sc;
        self.goal_col = None;
        self.touch(); // clears anchor + invalidates the cache
        true
    }

    // ---------- mouse ----------

    /// Places the cursor at the mouse click's screen coordinates `(mx, my)`
    /// (with mouse capture `Ctrl+W`). Returns whether the click landed inside
    /// the text's inner area from the last render ([`Self::last_area`]). The
    /// cursor snaps to a grapheme-cluster boundary (a click on a wide/emoji
    /// glyph doesn't place it in the middle, item 1); a click below the last row
    /// → the end of the text, past the row's end → the end of the row. Before
    /// the first render (`last_area == None`) or a click outside the area — a
    /// no-op (`false`). Doesn't touch the selection — that's managed by
    /// [`Self::mouse_press`]/[`Self::mouse_drag`]. Resets undo coalescing (like
    /// navigation). See track stage D.
    fn place_cursor_at(&mut self, mx: u16, my: u16) -> bool {
        let Some(area) = self.last_area else {
            return false;
        };
        if mx < area.x || mx >= area.x + area.width || my < area.y || my >= area.y + area.height {
            return false;
        }
        self.goal_col = None;
        self.last_edit_kind = None; // a click breaks undo coalescing (like navigation)
        let vcol = (mx - area.x) as usize;
        if self.single_line {
            // Single-line: the column from the left edge + horizontal scroll, snap to boundary.
            let line = &self.lines[0];
            let col = col_at_width(line, self.hscroll + vcol).min(line.len());
            self.col = wrap::snap_boundary(line, col);
            return true;
        }
        let vrow = (my - area.y) as usize + self.scroll;
        let vrows = self.rows_cached(self.last_width).to_vec();
        if vrow >= vrows.len() {
            // Below the last row → the end of the text.
            self.row = self.lines.len() - 1;
            self.col = self.lines[self.row].len();
            return true;
        }
        let (li, start, end) = vrows[vrow];
        // Inside the row: the logical column by cumulative width; past the row's
        // end `col_for_visual` gives the row's end (rolling back on a soft wrap) + a snap.
        self.col = col_for_visual(&self.lines[li], start, end, vcol, is_soft(&vrows, vrow));
        self.row = li;
        true
    }

    /// Places the cursor on a left mouse button press and **starts** a
    /// selection from that point (empty — the cursor moved, no visible
    /// selection yet; dragging grows it). Returns whether the click landed in
    /// the text area. See [`Self::place_cursor_at`].
    pub fn mouse_press(&mut self, mx: u16, my: u16) -> bool {
        if self.place_cursor_at(mx, my) {
            self.anchor = Some((self.row, self.col));
            true
        } else {
            false
        }
    }

    /// Moves the cursor by a mouse drag, **keeping** the anchor — the selection
    /// grows from the press point to the current one. Returns whether the drag
    /// landed in the text area.
    pub fn mouse_drag(&mut self, mx: u16, my: u16) -> bool {
        self.place_cursor_at(mx, my)
    }

    /// The text's inner area from the last render (for mouse-handling tests —
    /// computing a click's screen coordinates against the field).
    #[cfg(test)]
    pub(crate) fn last_area_for_test(&self) -> Option<Rect> {
        self.last_area
    }

    // ---------- undo / redo ----------

    /// A snapshot of the current content (lines + cursor) for the undo history.
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            lines: self.lines.clone(),
            row: self.row,
            col: self.col,
        }
    }

    /// Records a snapshot **before** the edit for undo (called at the start of
    /// a mutator). Coalescing: consecutive edits of the same class (except
    /// `Structural`) merge into one unit — a snapshot is only pushed on a class
    /// change / after navigation/selection (where `last_edit_kind` was reset) /
    /// for `Structural`. Any edit clears the redo stack. Ceiling [`UNDO_CAP`] —
    /// the oldest get evicted. See plan §C.
    fn record_undo(&mut self, kind: EditKind) {
        let coalesce = self.last_edit_kind == Some(kind) && kind != EditKind::Structural;
        if !coalesce {
            self.undo.push(self.snapshot());
            if self.undo.len() > UNDO_CAP {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
        self.last_edit_kind = Some(kind);
    }

    /// Restores content from a snapshot (shared by undo/redo): lines+cursor,
    /// clearing the selection/highlighting, invalidating the cache. Doesn't
    /// touch the undo history.
    fn restore(&mut self, snap: Snapshot) {
        self.lines = snap.lines;
        self.row = snap.row;
        self.col = snap.col;
        self.scroll = 0;
        self.hscroll = 0;
        self.goal_col = None;
        self.misspelled.clear();
        self.last_edit_kind = None;
        self.touch(); // revision + clearing the anchor
    }

    /// Undoes the last edit unit (`Ctrl+Z`). Returns whether an undo happened.
    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.snapshot());
        self.restore(prev);
        true
    }

    /// Redoes an undone edit (`Ctrl+Y`). Returns whether a redo happened.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.snapshot());
        self.restore(next);
        true
    }

    /// The selection range on visual row `[start, end)` of logical line `li` in
    /// **row-local** coordinates (for background highlighting at render time),
    /// or `None`.
    fn row_selection(&self, li: usize, start: usize, end: usize) -> Option<(usize, usize)> {
        let ((sr, sc), (er, ec)) = self.selection_span()?;
        if li < sr || li > er {
            return None;
        }
        let s = if li == sr { sc.max(start) } else { start };
        let e = if li == er { ec.min(end) } else { end };
        (s < e).then(|| (s - start, e - start))
    }

    // ---------- cursor movement ----------

    fn move_left(&mut self) {
        self.goal_col = None;
        if self.col > 0 {
            // By grapheme cluster, not by scalar (see `backspace`/spec §11.5).
            self.col = wrap::prev_boundary(&self.lines[self.row], self.col);
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].len();
        }
    }

    fn move_right(&mut self) {
        self.goal_col = None;
        if self.col < self.lines[self.row].len() {
            self.col = wrap::next_boundary(&self.lines[self.row], self.col);
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    /// Left by one word (`Ctrl+Left`): skips whitespace to the left, then the
    /// word's characters — the cursor lands at the word's start. At the start
    /// of a logical line, moves to the end of the previous one (one press = one
    /// boundary, as in large editors).
    fn move_word_left(&mut self) {
        self.goal_col = None;
        if self.col == 0 {
            if self.row > 0 {
                self.row -= 1;
                self.col = self.lines[self.row].len();
            }
            return;
        }
        self.col = self.word_left_col();
    }

    /// Right by one word (`Ctrl+Right`): skips whitespace to the right, then the
    /// word's characters — the cursor lands past the word's end. At the end of
    /// a logical line, moves to the start of the next one.
    fn move_word_right(&mut self) {
        self.goal_col = None;
        if self.col >= self.lines[self.row].len() {
            if self.row + 1 < self.lines.len() {
                self.row += 1;
                self.col = 0;
            }
            return;
        }
        self.col = self.word_right_col();
    }

    /// Word boundary to the left of the cursor **within the current line** (for
    /// word-wise movement and deletion): skips whitespace, then the word's
    /// characters. See [`Self::move_word_left`].
    ///
    /// A "word" here is by the **whitespace/non-whitespace** class (punctuation
    /// is part of the word) — this **deliberately diverges** from spellcheck
    /// segmentation (`features/spellcheck/segment.rs`, where punctuation is a
    /// separate class so it doesn't get dragged into the checked word). Word-wise
    /// navigation/deletion live by editor rules, spelling by its own; no need to
    /// reconcile them (large editors also treat punctuation as a separate class
    /// — a known simplification, not a bug). See InputBox audit item 14.
    fn word_left_col(&self) -> usize {
        let line = &self.lines[self.row];
        let mut i = self.col;
        while i > 0 && line[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !line[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    /// Word boundary to the right of the cursor **within the current line**
    /// (mirrors [`Self::word_left_col`]).
    fn word_right_col(&self) -> usize {
        let line = &self.lines[self.row];
        let mut i = self.col;
        while i < line.len() && line[i].is_whitespace() {
            i += 1;
        }
        while i < line.len() && !line[i].is_whitespace() {
            i += 1;
        }
        i
    }

    /// Deletes the word to the left of the cursor (`Ctrl+Backspace`). At the
    /// start of a line, joins with the line above (like a plain `Backspace`).
    fn delete_word_left(&mut self) {
        self.record_undo(EditKind::Delete);
        if self.remove_selection() {
            return; // with a selection, Ctrl+Backspace deletes it whole
        }
        self.goal_col = None;
        if self.col == 0 {
            self.backspace(); // record_undo(Delete) coalesces — no extra snapshot
            return;
        }
        let start = self.word_left_col();
        self.lines[self.row].drain(start..self.col);
        self.edit_misspelled(self.row, start, self.col - start, 0);
        self.col = start;
        self.touch();
    }

    /// Deletes the word to the right of the cursor (`Ctrl+Delete`). At the end
    /// of a line, joins with the line below (like a plain `Delete`).
    fn delete_word_right(&mut self) {
        self.record_undo(EditKind::Delete);
        if self.remove_selection() {
            return; // with a selection, Ctrl+Delete deletes it whole
        }
        self.goal_col = None;
        if self.col >= self.lines[self.row].len() {
            self.delete(); // record_undo(Delete) coalesces — no extra snapshot
            return;
        }
        let end = self.word_right_col();
        self.lines[self.row].drain(self.col..end);
        self.edit_misspelled(self.row, self.col, end - self.col, 0);
        self.touch();
    }

    /// To the very start of the text (`Ctrl+Home`).
    fn move_doc_start(&mut self) {
        self.goal_col = None;
        self.row = 0;
        self.col = 0;
    }

    /// To the very end of the text (`Ctrl+End`).
    fn move_doc_end(&mut self) {
        self.goal_col = None;
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].len();
    }

    /// Up by **visual** row: if the logical line is wrapped, `↑` goes to the
    /// previous visual row of the same line, keeping the column. Uses the width
    /// from the last render; before the first render (`last_width == 0`) — a
    /// logical transition.
    fn move_up(&mut self) {
        if self.single_line {
            return; // a single-line field — `↑` doesn't move the cursor
        }
        if self.last_width == 0 {
            self.goal_col = None;
            self.move_up_logical();
            return;
        }
        let vrows = self.rows_cached(self.last_width).to_vec();
        let (vrow, vcol) = self.cursor_visual(&vrows);
        // The first step of a run remembers the column; afterward we hold it (goal-column).
        let goal = *self.goal_col.get_or_insert(vcol);
        if vrow == 0 {
            return; // already the top visual row (goal is kept for the reverse ↓)
        }
        let (li, start, end) = vrows[vrow - 1];
        let col = col_for_visual(&self.lines[li], start, end, goal, is_soft(&vrows, vrow - 1));
        self.row = li;
        self.col = col;
    }

    /// Down by **visual** row (mirrors [`Self::move_up`]).
    fn move_down(&mut self) {
        if self.single_line {
            return; // a single-line field — `↓` doesn't move the cursor
        }
        if self.last_width == 0 {
            self.goal_col = None;
            self.move_down_logical();
            return;
        }
        let vrows = self.rows_cached(self.last_width).to_vec();
        let (vrow, vcol) = self.cursor_visual(&vrows);
        let goal = *self.goal_col.get_or_insert(vcol);
        if vrow + 1 >= vrows.len() {
            return; // already the bottom visual row
        }
        let (li, start, end) = vrows[vrow + 1];
        let col = col_for_visual(&self.lines[li], start, end, goal, is_soft(&vrows, vrow + 1));
        self.row = li;
        self.col = col;
    }

    /// `Home` — to the start of the current **visual** row (not the whole
    /// logical line). Before the first render — to the start of the logical line.
    fn move_home(&mut self) {
        self.goal_col = None;
        if self.single_line {
            self.col = 0; // a single-line field — to the start of the value
            return;
        }
        if self.last_width == 0 {
            self.col = 0;
            return;
        }
        let vrows = self.rows_cached(self.last_width).to_vec();
        let (vrow, _) = self.cursor_visual(&vrows);
        self.col = vrows[vrow].1;
    }

    /// `End` — to the end of the current **visual** row. On a soft wrap, lands
    /// on the row's last position (doesn't slide into the start of the next
    /// one, see `is_soft`). Before the first render — to the end of the logical
    /// line.
    fn move_end(&mut self) {
        self.goal_col = None;
        if self.single_line {
            self.col = self.lines[self.row].len(); // a single-line field — to the end of the value
            return;
        }
        if self.last_width == 0 {
            self.col = self.lines[self.row].len();
            return;
        }
        let vrows = self.rows_cached(self.last_width).to_vec();
        let (vrow, _) = self.cursor_visual(&vrows);
        let (li, start, end) = vrows[vrow];
        self.col = col_for_visual(
            &self.lines[li],
            start,
            end,
            usize::MAX,
            is_soft(&vrows, vrow),
        );
    }

    /// A logical up/down transition (a fallback before the first render, when
    /// the width and hence the wrapping aren't known yet).
    fn move_up_logical(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(self.lines[self.row].len());
        }
    }

    fn move_down_logical(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(self.lines[self.row].len());
        }
    }

    /// Handles an editing key press. Returns [`KeyOutcome`]: `Edited` (content
    /// changed), `Moved` (cursor/selection shifted) or `Ignored` (the key wasn't
    /// handled — the caller decides what to do with it). `Enter` is NOT handled
    /// (the calling layer sets the send/break-line policy). The `Edited`/`Moved`
    /// distinction is needed by the caller so bare navigation doesn't mark the
    /// input "dirty".
    ///
    /// **Selection** (stage A, docs/history/input-selection-undo-mouse.md):
    /// `Shift`+navigation grows the selection (sets the anchor), plain
    /// navigation — clears it; `Ctrl+A` selects everything; an edit with an
    /// active selection first deletes it (typing/`Backspace`/`Delete` over a
    /// selection replace/delete it whole). Copy/cut are a side effect of the
    /// calling layer (plan §B), the widget hands out [`Self::selected_text`].
    pub fn on_key(&mut self, key: KeyEvent) -> KeyOutcome {
        if key.kind != KeyEventKind::Press {
            return KeyOutcome::Ignored;
        }
        // Ctrl upgrades navigation/deletion to word/whole-text level
        // (`Ctrl+←/→` — by word, `Ctrl+Backspace/Delete` — delete a word,
        // `Ctrl+Home/End` — to the start/end of the text). See spec §11.5.
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        // Editor Ctrl shortcuts: select all / undo / redo / clear
        // (layout-independent via `physical_char`). Undo/redo return `Edited`
        // only if something actually changed (otherwise `Moved` — a no-op).
        if ctrl && let Some(physical) = keys::hotkey_char(&key) {
            match physical {
                'a' => {
                    self.select_all();
                    return KeyOutcome::Moved;
                }
                'z' => {
                    return if self.undo() {
                        KeyOutcome::Edited
                    } else {
                        KeyOutcome::Moved
                    };
                }
                'y' => {
                    return if self.redo() {
                        KeyOutcome::Edited
                    } else {
                        KeyOutcome::Moved
                    };
                }
                'k' => {
                    self.clear_undoable();
                    return KeyOutcome::Edited;
                }
                _ => {}
            }
        }

        // Navigation (incl. `Ctrl`+word / `Ctrl`+start/end): with `Shift` we grow
        // the selection (set the anchor before moving), without — clear it. Any
        // navigation/selection ends undo coalescing (a following edit is a new unit).
        if let Some(mv) = navigation(key.code, ctrl) {
            if shift {
                self.set_anchor_if_none();
            } else {
                self.clear_selection();
            }
            self.last_edit_kind = None;
            mv(self);
            return KeyOutcome::Moved;
        }

        // Editing: replacing/deleting a selection is done by the mutators
        // themselves (at the start — `delete_selection`), so paste/`Shift+Enter`/
        // emoji respect it too.
        match key.code {
            KeyCode::Backspace if ctrl => {
                self.delete_word_left();
                KeyOutcome::Edited
            }
            KeyCode::Delete if ctrl => {
                self.delete_word_right();
                KeyOutcome::Edited
            }
            // Plain character input: don't type Ctrl+character (that's a shortcut
            // for the layer above), otherwise a control character would land in the field.
            KeyCode::Char(c) if !ctrl => {
                self.insert_char(c);
                KeyOutcome::Edited
            }
            KeyCode::Backspace => {
                self.backspace();
                KeyOutcome::Edited
            }
            KeyCode::Delete => {
                self.delete();
                KeyOutcome::Edited
            }
            _ => KeyOutcome::Ignored,
        }
    }

    /// Draws the field in `area` with a border and title. Parameters — in
    /// [`RenderOpts`] (title, focus, command highlighting, placeholder); the
    /// palette — separately (as with `StatusModel`). Places the cursor when
    /// `focused`. When `command`, the whole text is highlighted `warning`
    /// colored (it's a command like `/rag …`), and spelling underlines aren't
    /// drawn. See spec §11.5.
    pub fn render(&mut self, frame: &mut Frame, area: Rect, opts: RenderOpts, palette: &Palette) {
        let RenderOpts {
            title,
            focused,
            command,
            placeholder,
        } = opts;
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(palette.glyphs().border)
            .border_style(palette.border_style(focused))
            .title(Span::styled(format!(" {title} "), palette.muted_style()));
        let full_inner = block.inner(area);
        frame.render_widget(&block, area);

        // The `❯` prompt column on the left; text is drawn to its right.
        let prompt_style = if focused {
            Style::new().fg(palette.assistant)
        } else {
            palette.muted_style()
        };
        if full_inner.width > PROMPT_W {
            let prompt_area = Rect {
                height: 1,
                ..full_inner
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    palette.glyphs().prompt,
                    prompt_style,
                ))),
                prompt_area,
            );
        }
        // Inner text area — without the prompt column.
        let inner = Rect {
            x: full_inner.x + PROMPT_W,
            width: full_inner.width.saturating_sub(PROMPT_W),
            ..full_inner
        };
        // Remember it for mouse-click mapping (screen → a text position).
        self.last_area = Some(inner);

        // The mask always goes through the single-line path — it's the only one
        // where it applies (a safety net in case the field was masked while `single_line == false`).
        if self.single_line || self.mask {
            self.render_single_line(frame, inner, focused, command, placeholder, palette);
            return;
        }

        let view_w = inner.width.max(1) as usize;
        let visible_rows = inner.height.max(1) as usize;
        // Remember the width for `↑/↓` navigation over visual rows (see `move_up`).
        self.last_width = view_w;

        // Visual rows accounting for wrapping; the cursor position uses the same
        // wrapping (a single source of truth, otherwise the cursor would diverge
        // from the text). Take a copy of the cache: we go on to mutate `self`
        // (scroll/cursor), so the borrow can't be held, and a memcpy of the
        // ready result is cheaper than a repeated O(n) wrap.
        let vrows = self.rows_cached(view_w).to_vec();
        let (cursor_row, cursor_col) = self.cursor_visual(&vrows);
        self.adjust_scroll(cursor_row, vrows.len(), visible_rows);

        // In command mode the whole text is colored `warning` and errors aren't
        // underlined; the selection (background) is shown in any mode.
        let base_fg = command.then_some(palette.warning);
        let lines: Vec<Line> = vrows
            .iter()
            .skip(self.scroll)
            .take(visible_rows)
            .map(|&(li, start, end)| {
                let sub = &self.lines[li][start..end];
                let mis = if command {
                    None
                } else {
                    self.misspelled
                        .get(li)
                        .map(|rs| clip_ranges(rs, start, end))
                };
                let sel = self.row_selection(li, start, end);
                styled_line(sub, mis.as_deref(), sel, base_fg, palette)
            })
            .collect();
        let show_placeholder = self.is_empty() && !focused;
        let text = if show_placeholder {
            Text::from(Line::from(placeholder).dim())
        } else {
            Text::from(lines)
        };
        frame.render_widget(Paragraph::new(text), inner);

        // A scrollbar on the right border — when there are more visual rows than
        // fit (the field grew to its height ceiling and scrolls). Track color —
        // same as the field's border (which depends on focus).
        render_scrollbar(
            frame,
            area.inner(Margin::new(0, 1)),
            vrows.len(),
            visible_rows,
            self.scroll,
            focused,
            palette,
        );

        if focused {
            let cursor_y = inner.y + (cursor_row.saturating_sub(self.scroll)) as u16;
            let cursor_x = inner.x + cursor_col as u16;
            // don't go past the inner area's bounds
            let x = cursor_x.min(inner.x + inner.width.saturating_sub(1));
            let y = cursor_y.min(inner.y + inner.height.saturating_sub(1));
            frame.set_cursor_position((x, y));
        }
    }

    /// Draws the value in single-line mode: no wrapping, with horizontal
    /// scroll — the cursor is always visible, a long value "slides" left rather
    /// than wrapping onto an invisible row. `inner` — the inner area (already
    /// without the border).
    fn render_single_line(
        &mut self,
        frame: &mut Frame,
        inner: Rect,
        focused: bool,
        command: bool,
        placeholder: &str,
        palette: &Palette,
    ) {
        let view_w = inner.width.max(1) as usize;
        self.last_width = view_w;
        // The mask substitutes characters **before** any width calculations:
        // `•` has width 1, so the cursor and horizontal scroll are computed
        // against what's actually visible (otherwise a wide glyph in a secret —
        // an emoji from the clipboard — would offset the cursor). The character
        // count matches the original, so `col`/selection remain valid.
        let masked: Vec<char>;
        let line: &[char] = if self.mask {
            masked = vec![MASK_CHAR; self.lines[0].len()];
            &masked
        } else {
            &self.lines[0]
        };
        let cursor_vw = wrap::display_width(&line[..self.col]);

        // Horizontal scroll keeps the cursor in the visible area.
        if cursor_vw < self.hscroll {
            self.hscroll = cursor_vw;
        } else if cursor_vw >= self.hscroll + view_w {
            self.hscroll = cursor_vw + 1 - view_w;
        }
        // Align the scroll's left edge to a character boundary. Without this, a
        // wide glyph (CJK/emoji) on the left could make the slice start "in the
        // middle" of a character, and `hscroll` (in columns) wouldn't match the
        // real width of the hidden prefix — the cursor would be drawn a column
        // to the right of its actual position. `col_at_width` rounds the start
        // up to a boundary, and we bring `hscroll` to that prefix's width
        // (guaranteed `≤ cursor_vw` → the cursor stays visible, see spec §11.6).
        let start = col_at_width(line, self.hscroll);
        self.hscroll = wrap::display_width(&line[..start]);
        let mut end = start;
        let mut w = 0;
        while end < line.len() {
            let cw = wrap::width_at(line, end);
            if w + cw > view_w {
                break;
            }
            w += cw;
            end += 1;
        }
        let sub = &line[start..end];

        let show_placeholder = self.is_empty() && !focused;
        let text = if show_placeholder {
            Text::from(Line::from(placeholder).dim())
        } else {
            let base_fg = command.then_some(palette.warning);
            let mis = if command {
                None
            } else {
                self.misspelled
                    .first()
                    .map(|rs| clip_ranges(rs, start, end))
            };
            let sel = self.row_selection(0, start, end);
            Text::from(styled_line(sub, mis.as_deref(), sel, base_fg, palette))
        };
        frame.render_widget(Paragraph::new(text), inner);

        if focused {
            let cursor_x = inner.x + (cursor_vw - self.hscroll) as u16;
            let x = cursor_x.min(inner.x + inner.width.saturating_sub(1));
            frame.set_cursor_position((x, inner.y));
        }
    }

    /// Marks the content as changed — invalidates the visual-row cache (the
    /// next [`Self::rows_cached`] will see a revision mismatch) and **clears
    /// the selection** (after an edit the anchor would point to stale
    /// coordinates). Called by all `lines` mutators; navigation does NOT call
    /// it (it manages `anchor` itself). [`Self::delete_selection`] clears
    /// `anchor` before `touch` — a double reset is harmless.
    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.anchor = None;
    }

    /// Visual rows with a cache by `(width, revision)` (see
    /// [`Self::rows_cache`]). Recomputes wrapping only on a width or content
    /// change; otherwise hands out a borrow into the cache. Consumers that go
    /// on to mutate `self` take a copy (`.to_vec()` — a cheap memcpy of the
    /// result vs. an O(n) wrap).
    fn rows_cached(&mut self, width: usize) -> &[VisualRow] {
        let fresh =
            matches!(&self.rows_cache, Some((w, r, _)) if *w == width && *r == self.revision);
        if !fresh {
            let rows = self.visual_rows(width);
            self.rows_cache = Some((width, self.revision, rows));
        }
        // The cache was just filled/checked — unwrap is safe.
        &self.rows_cache.as_ref().unwrap().2
    }

    /// Visual rows: for each — `(logical line, start, end)` in that line's
    /// character indices (accounting for wrapping at width `width`). A plain
    /// recompute; the cached path is [`Self::rows_cached`].
    fn visual_rows(&self, width: usize) -> Vec<VisualRow> {
        let mut rows = Vec::new();
        for (li, chars) in self.lines.iter().enumerate() {
            for (start, end) in wrap::wrap_ranges(chars, width) {
                rows.push((li, start, end));
            }
        }
        rows
    }

    /// Cursor position in visual coordinates `(row index, column)`. On a soft
    /// wrap (cursor at the end of a row, but not at the end of the logical
    /// line) the cursor moves to the start of the next row.
    fn cursor_visual(&self, vrows: &[VisualRow]) -> (usize, usize) {
        let mut last: Option<(usize, usize)> = None; // (row index, start)
        for (idx, &(li, start, end)) in vrows.iter().enumerate() {
            if li != self.row {
                continue;
            }
            last = Some((idx, start));
            if self.col < end {
                let col = wrap::display_width(&self.lines[li][start..self.col]);
                return (idx, col);
            }
        }
        // the cursor is at the very end of the logical line — its last row
        match last {
            Some((idx, start)) => (
                idx,
                wrap::display_width(&self.lines[self.row][start..self.col]),
            ),
            None => (0, 0),
        }
    }

    /// Keeps the cursor in the visible area (vertical scroll over visual rows).
    fn adjust_scroll(&mut self, cursor_row: usize, total: usize, visible_rows: usize) {
        if cursor_row < self.scroll {
            self.scroll = cursor_row;
        } else if visible_rows > 0 && cursor_row >= self.scroll + visible_rows {
            self.scroll = cursor_row + 1 - visible_rows;
        }
        // don't leave empty space at the bottom if there are now fewer rows (deletion/rewrap)
        let max_scroll = total.saturating_sub(visible_rows);
        if self.scroll > max_scroll {
            self.scroll = max_scroll;
        }
    }
}

/// The cursor-movement method for a navigation key (or `None` if the key isn't
/// navigation). Factored into a table so selection logic (`Shift` → anchor,
/// plain → clear) applies uniformly to every direction with no duplicated
/// branches. `Ctrl` upgrades `←/→` to word level, `Home/End` — to text
/// boundaries; `Ctrl+↑/↓` isn't defined (falls into `None` → `Ignored`, as before).
fn navigation(code: KeyCode, ctrl: bool) -> Option<fn(&mut InputBox)> {
    Some(match (code, ctrl) {
        (KeyCode::Left, true) => InputBox::move_word_left,
        (KeyCode::Right, true) => InputBox::move_word_right,
        (KeyCode::Home, true) => InputBox::move_doc_start,
        (KeyCode::End, true) => InputBox::move_doc_end,
        (KeyCode::Left, false) => InputBox::move_left,
        (KeyCode::Right, false) => InputBox::move_right,
        (KeyCode::Up, false) => InputBox::move_up,
        (KeyCode::Down, false) => InputBox::move_down,
        (KeyCode::Home, false) => InputBox::move_home,
        (KeyCode::End, false) => InputBox::move_end,
        _ => return None,
    })
}

/// Normalizes clipboard text before pasting: `\r\n`/`\r` → `\n` (a single
/// line-break form), `\t` → spaces. Other control characters are left as is
/// (the terminal/render will filter them).
fn normalize_paste(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\t', "    ")
}

/// Character index at which the line's cumulative width reaches `target`
/// columns (for the single-line field's horizontal scroll). `target` values
/// come from prefix widths — character boundaries match, no splitting a wide
/// character.
fn col_at_width(line: &[char], target: usize) -> usize {
    let mut w = 0;
    let mut i = 0;
    while i < line.len() && w < target {
        w += wrap::width_at(line, i);
        i += 1;
    }
    i
}

/// Visual row `idx` is a soft wrap (not the last row of its logical line),
/// i.e. the next row belongs to the same line. Then cursor position `== end`
/// is drawn at the start of the next row — navigation accounts for this.
fn is_soft(vrows: &[VisualRow], idx: usize) -> bool {
    idx + 1 < vrows.len() && vrows[idx + 1].0 == vrows[idx].0
}

/// Logical column on row `[start, end)` closest to the target visual column
/// `target_vw` (in columns) — for an `↑/↓` transition that preserves the
/// column. On a soft wrap we don't hand out `end` (otherwise the cursor would
/// "slide" into the start of the next row) — we roll back one character,
/// staying on this row. The result snaps to a grapheme-cluster boundary so the
/// cursor doesn't land between an emoji base and its variation selector
/// (`❤️` = `❤`+U+FE0F): otherwise a following `insert`/`backspace` would split
/// the cluster (an orphaned selector). See spec §11.5.
fn col_for_visual(line: &[char], start: usize, end: usize, target_vw: usize, soft: bool) -> usize {
    let mut w = 0;
    let mut col = start;
    while col < end {
        let cw = wrap::width_at(line, col);
        if w + cw > target_vw {
            break;
        }
        w += cw;
        col += 1;
    }
    if soft && col == end && end > start {
        col -= 1;
    }
    // Snap down to a cluster boundary. Always `≥ start` (start is a row
    // boundary), so the cursor doesn't leave this visual row.
    wrap::snap_boundary(line, col)
}

/// Intersects a logical line's error ranges `[s, e)` with a visual row
/// `[start, end)` and shifts into the row's coordinates (for underlining in
/// [`styled_line`]).
fn clip_ranges(ranges: &[(usize, usize)], start: usize, end: usize) -> Vec<(usize, usize)> {
    ranges
        .iter()
        .filter_map(|&(s, e)| {
            let s = s.clamp(start, end);
            let e = e.clamp(start, end);
            (e > s).then_some((s - start, e - start))
        })
        .collect()
}

/// Builds a line, composing three per-character styles: spelling-error
/// underlines (`misspelled`, `UNDERLINED` + the error color), a selection
/// background (`selection`, `keycap_bg`), and a base command color (`base_fg`,
/// the whole text). Ranges are row-local `[start, end)` in characters;
/// selection and errors can overlap (they stack: underlined AND on a
/// background). A fast path — when there's nothing to style.
fn styled_line(
    chars: &[char],
    misspelled: Option<&[(usize, usize)]>,
    selection: Option<(usize, usize)>,
    base_fg: Option<Color>,
    palette: &Palette,
) -> Line<'static> {
    let has_mis = misspelled.is_some_and(|r| !r.is_empty());
    let has_sel = selection.is_some_and(|(s, e)| e > s);
    if !has_mis && !has_sel && base_fg.is_none() {
        return Line::from(chars.iter().collect::<String>());
    }
    let n = chars.len();
    let base = base_fg.map(|c| Style::new().fg(c)).unwrap_or_default();
    let mut styles = vec![base; n];
    if let Some(ranges) = misspelled {
        let bad = Style::new().underlined().fg(palette.error);
        for &(s, e) in ranges {
            for st in styles.iter_mut().take(e.min(n)).skip(s.min(n)) {
                *st = st.patch(bad);
            }
        }
    }
    if let Some((s, e)) = selection {
        for st in styles.iter_mut().take(e.min(n)).skip(s.min(n)) {
            *st = st.bg(palette.keycap_bg);
        }
    }
    // Merge adjacent characters with the same style into spans.
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut i = 0;
    while i < n {
        let st = styles[i];
        let mut buf = String::new();
        while i < n && styles[i] == st {
            buf.push(chars[i]);
            i += 1;
        }
        spans.push(Span::styled(buf, st));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    fn ctrl_shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL | KeyModifiers::SHIFT)
    }

    fn type_str(ib: &mut InputBox, s: &str) {
        for c in s.chars() {
            ib.insert_char(c);
        }
    }

    #[test]
    fn new_is_empty() {
        let ib = InputBox::new();
        assert!(ib.is_empty());
        assert_eq!(ib.text(), "");
    }

    #[test]
    fn typing_and_text() {
        let mut ib = InputBox::new();
        type_str(&mut ib, "привет");
        assert!(!ib.is_empty());
        assert_eq!(ib.text(), "привет");
    }

    #[test]
    fn newline_splits_at_cursor() {
        let mut ib = InputBox::new();
        type_str(&mut ib, "abcd");
        ib.move_left();
        ib.move_left(); // cursor between b and c
        ib.insert_newline();
        assert_eq!(ib.text(), "ab\ncd");
        assert_eq!(ib.line_count(), 2);
    }

    #[test]
    fn backspace_joins_lines() {
        let mut ib = InputBox::new();
        type_str(&mut ib, "ab");
        ib.insert_newline();
        type_str(&mut ib, "cd");
        // cursor at the start of the second line? no — at the end of "cd". Go to the start of the line.
        ib.col = 0;
        ib.backspace();
        assert_eq!(ib.text(), "abcd");
        assert_eq!(ib.line_count(), 1);
    }

    #[test]
    fn delete_at_eol_joins_next() {
        let mut ib = InputBox::new();
        ib.set_text("ab\ncd");
        ib.row = 0;
        ib.col = 2; // end of the first line
        ib.delete();
        assert_eq!(ib.text(), "abcd");
    }

    #[test]
    fn unicode_cursor_is_char_based() {
        let mut ib = InputBox::new();
        type_str(&mut ib, "ёжик");
        ib.backspace(); // delete 'к'
        assert_eq!(ib.text(), "ёжи");
        ib.move_left();
        ib.insert_char('!'); // cursor was at position 2 → between the 3rd and 4th characters
        assert_eq!(ib.text(), "ёж!и");
    }

    #[test]
    fn insert_str_multiline_at_cursor() {
        let mut ib = InputBox::new();
        ib.set_text("aXd");
        ib.row = 0;
        ib.col = 1; // cursor between 'a' and 'X'
        ib.insert_str("b\nc");
        // 'a' + insert("b\nc") + tail("Xd")
        assert_eq!(ib.text(), "ab\ncXd");
        assert_eq!(ib.line_count(), 2);
        // cursor at the end of the inserted text, before the tail "Xd"
        assert_eq!(ib.cursor(), (1, 1));
    }

    #[test]
    fn insert_str_normalizes_newlines_and_tabs() {
        let mut ib = InputBox::new();
        ib.insert_str("a\r\nb\rc\td");
        assert_eq!(ib.text(), "a\nb\nc    d");
        assert_eq!(ib.line_count(), 3);
    }

    #[test]
    fn insert_str_single_line_keeps_one_row() {
        let mut ib = InputBox::new();
        type_str(&mut ib, "ab");
        ib.insert_str("XY"); // cursor at the end
        assert_eq!(ib.text(), "abXY");
        assert_eq!(ib.line_count(), 1);
        assert_eq!(ib.cursor(), (0, 4));
    }

    #[test]
    fn insert_str_unicode() {
        let mut ib = InputBox::new();
        ib.insert_str("привет\nмир");
        assert_eq!(ib.text(), "привет\nмир");
        assert_eq!(ib.cursor(), (1, 3));
    }

    #[test]
    fn clear_resets() {
        let mut ib = InputBox::new();
        ib.set_text("hello\nworld");
        ib.clear();
        assert!(ib.is_empty());
        assert_eq!(ib.line_count(), 1);
    }

    // ---------- stage C: undo/redo ----------

    #[test]
    fn undo_typing_run_is_one_unit_then_redo() {
        // A run of typing with no spaces — one undo unit; Ctrl+Z → empty, Ctrl+Y → back.
        let mut ib = InputBox::new();
        type_str(&mut ib, "hello");
        assert!(ib.undo());
        assert!(ib.is_empty());
        assert!(ib.redo());
        assert_eq!(ib.text(), "hello");
        // redo exhausted
        assert!(!ib.redo());
    }

    #[test]
    fn undo_breaks_on_whitespace_word_granular() {
        // A space ends the unit: "ab cd" undoes word by word ("ab " ← "").
        let mut ib = InputBox::new();
        type_str(&mut ib, "ab cd");
        assert!(ib.undo());
        assert_eq!(ib.text(), "ab ");
        assert!(ib.undo());
        assert!(ib.is_empty());
    }

    #[test]
    fn navigation_breaks_undo_coalescing() {
        // Typing, an arrow, more typing → two undo units (a break on navigation).
        let mut ib = InputBox::new();
        type_str(&mut ib, "abc");
        ib.on_key(k(KeyCode::Left)); // navigation resets coalescing
        ib.insert_char('X'); // cursor was before 'c' → "abXc"
        assert_eq!(ib.text(), "abXc");
        assert!(ib.undo());
        assert_eq!(ib.text(), "abc"); // only 'X' was undone
    }

    #[test]
    fn insert_str_is_separate_undo_unit() {
        let mut ib = InputBox::new();
        type_str(&mut ib, "ab");
        ib.insert_str("XY"); // a paste — its own (Structural) unit
        assert_eq!(ib.text(), "abXY");
        assert!(ib.undo());
        assert_eq!(ib.text(), "ab"); // only the paste was undone
    }

    #[test]
    fn edit_clears_redo() {
        let mut ib = InputBox::new();
        type_str(&mut ib, "abc");
        ib.undo(); // "" , redo has "abc"
        ib.insert_char('z'); // a new edit clears redo
        assert!(!ib.redo());
        assert_eq!(ib.text(), "z");
    }

    #[test]
    fn set_text_clears_undo_history() {
        // A programmatic replacement (loading someone else's draft) — Ctrl+Z doesn't resurrect it.
        let mut ib = InputBox::new();
        type_str(&mut ib, "user text");
        ib.set_text("другой чат");
        assert!(!ib.undo());
        assert_eq!(ib.text(), "другой чат");
    }

    #[test]
    fn ctrl_k_clears_and_ctrl_z_restores() {
        let mut ib = InputBox::new();
        type_str(&mut ib, "привет\nмир");
        assert_eq!(ib.on_key(ctrl(KeyCode::Char('k'))), KeyOutcome::Edited);
        assert!(ib.is_empty());
        // Ctrl+Z brings back what was deleted (a shared undo model, not a toggle).
        assert_eq!(ib.on_key(ctrl(KeyCode::Char('z'))), KeyOutcome::Edited);
        assert_eq!(ib.text(), "привет\nмир");
    }

    #[test]
    fn undo_redo_noop_returns_moved() {
        // Empty stacks — Ctrl+Z/Ctrl+Y change nothing (Moved, not Edited).
        let mut ib = InputBox::new();
        assert_eq!(ib.on_key(ctrl(KeyCode::Char('z'))), KeyOutcome::Moved);
        assert_eq!(ib.on_key(ctrl(KeyCode::Char('y'))), KeyOutcome::Moved);
    }

    #[test]
    fn undo_cap_evicts_oldest() {
        // More than UNDO_CAP units — the oldest get evicted (no panic, the stack is bounded).
        let mut ib = InputBox::new();
        for _ in 0..(UNDO_CAP + 20) {
            // Every insert is Structural → its own unit.
            ib.insert_str("x");
        }
        assert_eq!(ib.undo.len(), UNDO_CAP);
    }

    #[test]
    fn undo_restores_selection_replacement() {
        // Typing over a selection — one unit: Ctrl+Z brings back the original text.
        let mut ib = InputBox::new();
        ib.set_text("hello");
        ib.on_key(ctrl(KeyCode::Char('a'))); // select all
        ib.insert_char('Z'); // replace the selection
        assert_eq!(ib.text(), "Z");
        assert!(ib.undo());
        assert_eq!(ib.text(), "hello");
    }

    #[test]
    fn on_key_handles_editing_but_not_enter() {
        let mut ib = InputBox::new();
        assert!(ib.on_key(k(KeyCode::Char('x'))).handled());
        assert!(ib.on_key(k(KeyCode::Backspace)).handled());
        assert!(!ib.on_key(k(KeyCode::Enter)).handled());
        assert!(ib.is_empty());
    }

    #[test]
    fn line_strings_and_cursor() {
        let mut ib = InputBox::new();
        ib.set_text("abc\nde");
        assert_eq!(ib.line_strings(), vec!["abc".to_string(), "de".to_string()]);
        assert_eq!(ib.cursor(), (1, 2)); // cursor at the end of the last line
    }

    #[test]
    fn replace_range_swaps_word_and_moves_cursor() {
        let mut ib = InputBox::new();
        ib.set_text("helo world");
        ib.replace_range(0, 0, 4, "hello");
        assert_eq!(ib.text(), "hello world");
        assert_eq!(ib.cursor(), (0, 5));
    }

    #[test]
    fn replace_range_unicode() {
        let mut ib = InputBox::new();
        ib.set_text("превед мир");
        ib.replace_range(0, 0, 6, "привет");
        assert_eq!(ib.text(), "привет мир");
    }

    #[test]
    fn render_with_misspelled_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut ib = InputBox::new();
        ib.set_text("helo world\nпревед");
        ib.set_misspelled(vec![vec![(0, 4)], vec![(0, 6)]]);
        let mut term = Terminal::new(TestBackend::new(20, 4)).unwrap();
        term.draw(|f| {
            ib.render(
                f,
                f.area(),
                RenderOpts::focused("ввод"),
                &Palette::default(),
            )
        })
        .unwrap();
    }

    #[test]
    fn render_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut ib = InputBox::new();
        ib.set_text("строка 1\nстрока 2\nстрока 3");
        let mut term = Terminal::new(TestBackend::new(20, 4)).unwrap();
        term.draw(|f| {
            ib.render(
                f,
                f.area(),
                RenderOpts::focused("ввод"),
                &Palette::default(),
            )
        })
        .unwrap();
    }

    #[test]
    fn render_command_mode_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut ib = InputBox::new();
        ib.set_text("/rag add d:\\dir -r");
        // even with "errors" present, underlines aren't drawn in command mode
        ib.set_misspelled(vec![vec![(0, 4)]]);
        let mut term = Terminal::new(TestBackend::new(24, 3)).unwrap();
        term.draw(|f| {
            ib.render(
                f,
                f.area(),
                RenderOpts {
                    command: true,
                    ..RenderOpts::focused("ввод")
                },
                &Palette::default(),
            )
        })
        .unwrap();
    }

    #[test]
    fn content_rows_matches_render_text_width() {
        // `content_rows(area_width)` must count wrapping against the SAME text
        // width as `render` (minus the border 2 and the prompt column
        // PROMPT_W), otherwise field height diverges from actual wrapping (the
        // field doesn't grow by 1-2 characters past the boundary). Outer width
        // 14 → text width = 14 − 2 − PROMPT_W = 10.
        let area_width: u16 = 14;
        let text_w = (area_width - 2 - PROMPT_W) as usize; // 10
        let mut ib = InputBox::new();
        // A word exactly one character longer than the text width → render wraps to 2 rows.
        let word = "a".repeat(text_w + 1);
        type_str(&mut ib, &word);
        // Render into an outer area of this width — last_width becomes the real width.
        render_at(&mut ib, area_width - 2 - PROMPT_W);
        let rendered_rows = ib.visual_rows(ib.last_width).len();
        assert_eq!(ib.content_rows(area_width), rendered_rows);
        assert!(
            ib.content_rows(area_width) > 1,
            "the field should grow to two rows on the character past the wrap boundary"
        );
    }

    #[test]
    fn content_rows_single_line_is_one() {
        let mut ib = InputBox::new();
        ib.set_single_line(true);
        ib.set_text("очень длинное значение не помещающееся в узкое поле");
        assert_eq!(ib.content_rows(12), 1);
    }

    #[test]
    fn long_line_counts_as_multiple_visual_rows() {
        let mut ib = InputBox::new();
        // one logical line longer than the width → several visual rows
        type_str(&mut ib, "один два три четыре");
        assert_eq!(ib.line_count(), 1);
        assert!(ib.visual_line_count(8) > 1);
    }

    #[test]
    fn cursor_moves_and_deletes_by_grapheme_cluster() {
        let mut ib = InputBox::new();
        ib.insert_str("a❤\u{FE0F}👍🏽");
        // a(1) + ❤️(2 scalars) + 👍🏽(2 scalars) = 5 characters, cursor at the end
        assert_eq!(ib.cursor(), (0, 5));
        // ← once passes the whole 👍🏽 cluster (2 scalars back)
        ib.move_left();
        assert_eq!(ib.cursor(), (0, 3));
        // one more ← passes the whole ❤️ (also 2 scalars), no stopping in the middle
        ib.move_left();
        assert_eq!(ib.cursor(), (0, 1));
        ib.move_left();
        assert_eq!(ib.cursor(), (0, 0));
        // Backspace from the end deletes the whole cluster (no orphaned scalar left)
        ib.move_doc_end();
        ib.backspace(); // deletes 👍🏽 whole
        assert_eq!(ib.text(), "a❤\u{FE0F}");
        ib.backspace(); // deletes ❤️ whole
        assert_eq!(ib.text(), "a");
    }

    #[test]
    fn delete_forward_removes_whole_cluster() {
        let mut ib = InputBox::new();
        ib.insert_str("❤\u{FE0F}👍🏽b");
        ib.move_doc_start();
        ib.delete(); // deletes ❤️ whole, no U+FE0F left over
        assert_eq!(ib.text(), "👍🏽b");
        ib.delete(); // deletes 👍🏽 whole
        assert_eq!(ib.text(), "b");
        ib.delete();
        assert_eq!(ib.text(), "");
        assert!(ib.is_empty());
    }

    #[test]
    fn cursor_visual_accounts_for_emoji_cluster_width() {
        // The terminal draws ❤️ (❤ + U+FE0F) at width 2 → the cursor after the
        // cluster is at column 2, not 1 (otherwise it would "land" in the
        // middle of the emoji, see spec §11.5).
        let mut ib = InputBox::new();
        ib.insert_str("❤\u{FE0F}");
        let vrows = ib.visual_rows(40);
        let (row, col) = ib.cursor_visual(&vrows);
        assert_eq!((row, col), (0, 2));
        // The next character continues from column 2 — text after the emoji isn't shifted.
        ib.insert_char('a');
        let vrows = ib.visual_rows(40);
        assert_eq!(ib.cursor_visual(&vrows), (0, 3));
    }

    #[test]
    fn cursor_maps_onto_wrapped_row() {
        let mut ib = InputBox::new();
        type_str(&mut ib, "один два три"); // cursor at the end (col=12)
        let vrows = ib.visual_rows(8);
        // wraps into two rows; cursor on the second row, column 3 (the last word)
        let (row, col) = ib.cursor_visual(&vrows);
        assert_eq!((row, col), (1, 3));
    }

    #[test]
    fn cursor_at_soft_break_moves_to_next_row_start() {
        let mut ib = InputBox::new();
        ib.set_text("один два три");
        // cursor right after the first two words plus a trailing space (index 9) — the start of the last word
        ib.row = 0;
        ib.col = 9;
        let vrows = ib.visual_rows(8);
        let (row, col) = ib.cursor_visual(&vrows);
        assert_eq!((row, col), (1, 0));
    }

    /// Renders the field into inner width `inner_w` (the border adds 2
    /// columns, the `❯` prompt column — another `PROMPT_W`) to set `last_width`
    /// for `↑/↓` navigation over visual rows.
    fn render_at(ib: &mut InputBox, inner_w: u16) {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut term = Terminal::new(TestBackend::new(inner_w + 2 + PROMPT_W, 8)).unwrap();
        term.draw(|f| {
            ib.render(
                f,
                f.area(),
                RenderOpts::focused("ввод"),
                &Palette::default(),
            )
        })
        .unwrap();
    }

    /// Mask: only `•` is on screen, the actual secret isn't in the buffer;
    /// selection isn't handed out (copying a secret is forbidden), and
    /// editing/deletion still work.
    #[test]
    fn mask_hides_content_on_screen_and_from_clipboard() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut ib = InputBox::new();
        ib.set_mask(true);
        assert!(ib.is_masked());
        assert!(
            ib.single_line,
            "the mask switches the field to single-line mode"
        );
        ib.set_text("sk-secret");

        let mut term = Terminal::new(TestBackend::new(30, 3)).unwrap();
        term.draw(|f| {
            ib.render(
                f,
                f.area(),
                RenderOpts::focused("ключ"),
                &Palette::default(),
            )
        })
        .unwrap();
        let screen: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            !screen.contains("sk-secret"),
            "the secret is visible on screen: {screen}"
        );
        assert_eq!(
            screen.matches(MASK_CHAR).count(),
            "sk-secret".chars().count(),
            "every secret character must be masked"
        );

        // Selecting the whole field doesn't hand out content (the consumer's Ctrl+C gets None).
        ib.select_all();
        assert!(ib.has_selection());
        assert_eq!(ib.selected_text(), None);
        // But the selection can still be deleted — editing isn't broken.
        ib.backspace();
        assert_eq!(ib.text(), "");
    }

    /// A masked field computes width from `•` (1 column), so a wide glyph in
    /// the secret (an emoji from the clipboard) doesn't offset the cursor
    /// relative to the visible text.
    #[test]
    fn mask_keeps_cursor_aligned_with_wide_glyphs() {
        let mut ib = InputBox::new();
        ib.set_mask(true);
        ib.set_text("aXb");
        ib.insert_str("😀"); // width 2 in the original, 1 under the mask
        render_at(&mut ib, 20);
        assert_eq!(ib.text().chars().count(), 4);
        assert_eq!(ib.hscroll, 0, "a short value shouldn't scroll");
    }

    #[test]
    fn col_for_visual_clamps_off_soft_break() {
        let line: Vec<char> = "abcd".chars().collect();
        // On a soft wrap a target column past the row's end rolls back one
        // character (otherwise the cursor would slide into the start of the next row).
        assert_eq!(col_for_visual(&line, 0, 4, 10, true), 3);
        // At the hard end of a logical line there's no clamping.
        assert_eq!(col_for_visual(&line, 0, 4, 10, false), 4);
        // A column inside the row — a plain search by width.
        assert_eq!(col_for_visual(&line, 0, 4, 2, true), 2);
    }

    #[test]
    fn arrow_up_moves_within_wrapped_line() {
        let mut ib = InputBox::new();
        // one logical line, wraps into two rows: the first two words | the last word
        ib.set_text("один два три"); // cursor at the end (row=0, col=12)
        render_at(&mut ib, 8);
        // ↑ moves to the previous visual row of the same line (doesn't go above it)
        assert!(ib.on_key(k(KeyCode::Up)).handled());
        assert_eq!(ib.cursor(), (0, 3)); // column 3 preserved, inside the first word
        // one more ↑ on the top visual row — no movement
        assert!(ib.on_key(k(KeyCode::Up)).handled());
        assert_eq!(ib.cursor(), (0, 3));
    }

    #[test]
    fn arrow_down_moves_within_wrapped_line() {
        let mut ib = InputBox::new();
        ib.set_text("один два три");
        render_at(&mut ib, 8);
        ib.row = 0;
        ib.col = 3; // top visual row, column 3
        assert!(ib.on_key(k(KeyCode::Down)).handled());
        // to the bottom row (the last word) keeping the column → end of the line (3 characters)
        assert_eq!(ib.cursor(), (0, 12));
        // one more ↓ on the bottom visual row — no movement
        assert!(ib.on_key(k(KeyCode::Down)).handled());
        assert_eq!(ib.cursor(), (0, 12));
    }

    #[test]
    fn arrow_up_down_cross_logical_lines_when_not_wrapped() {
        let mut ib = InputBox::new();
        ib.set_text("abc\ndef"); // two short logical lines, no wrapping
        render_at(&mut ib, 20);
        ib.row = 1;
        ib.col = 2;
        assert!(ib.on_key(k(KeyCode::Up)).handled());
        assert_eq!(ib.cursor(), (0, 2)); // moved to the previous logical line
        assert!(ib.on_key(k(KeyCode::Down)).handled());
        assert_eq!(ib.cursor(), (1, 2));
    }

    #[test]
    fn goal_column_preserved_through_short_row() {
        // A run of ↓ through a short line keeps the original column (goal-column).
        let mut ib = InputBox::new();
        ib.set_text("abcdef\nx\nabcdef");
        render_at(&mut ib, 20); // wide — no wrapping, by logical lines
        ib.row = 0;
        ib.col = 5; // column 5 on the first line
        ib.goal_col = None; // a direct col assignment above doesn't reset goal
        assert!(ib.on_key(k(KeyCode::Down)).handled());
        assert_eq!(ib.cursor(), (1, 1)); // "x" is shorter — cursor pinned to the end
        assert!(ib.on_key(k(KeyCode::Down)).handled());
        assert_eq!(ib.cursor(), (2, 5)); // column 5 restored, didn't stay at 1
    }

    #[test]
    fn horizontal_move_resets_goal_column() {
        let mut ib = InputBox::new();
        ib.set_text("abcdef\nx\nabcdef");
        render_at(&mut ib, 20);
        ib.row = 0;
        ib.col = 5;
        ib.goal_col = None;
        assert!(ib.on_key(k(KeyCode::Down)).handled()); // (1,1), goal=5
        assert!(ib.on_key(k(KeyCode::Left)).handled()); // a horizontal move resets goal
        assert!(ib.on_key(k(KeyCode::Down)).handled());
        // with no goal, the column comes from the current one (0) → start of the third line
        assert_eq!(ib.cursor(), (2, 0));
    }

    #[test]
    fn home_end_act_on_visual_row() {
        let mut ib = InputBox::new();
        ib.set_text("один два три"); // width 8: "один два " | "три"
        render_at(&mut ib, 8);
        // cursor in the middle of the bottom visual row (the last word)
        ib.row = 0;
        ib.col = 10;
        assert!(ib.on_key(k(KeyCode::Home)).handled());
        assert_eq!(ib.cursor(), (0, 9)); // start of the bottom row, not the whole line
        assert!(ib.on_key(k(KeyCode::End)).handled());
        assert_eq!(ib.cursor(), (0, 12)); // end of the bottom row = end of the line
        // on the top row, End lands on the row's last position (soft wrap)
        ib.col = 2;
        assert!(ib.on_key(k(KeyCode::End)).handled());
        assert_eq!(ib.cursor(), (0, 8)); // end of the first two words, doesn't slide into the start of the last one
        assert!(ib.on_key(k(KeyCode::Home)).handled());
        assert_eq!(ib.cursor(), (0, 0)); // start of the top row
    }

    #[test]
    fn arrow_up_falls_back_to_logical_before_render() {
        // before the first render the width is unknown (last_width == 0) → a logical transition
        let mut ib = InputBox::new();
        ib.set_text("abc\ndef");
        ib.row = 1;
        ib.col = 2;
        assert!(ib.on_key(k(KeyCode::Up)).handled());
        assert_eq!(ib.cursor(), (0, 2));
    }

    #[test]
    fn single_line_disables_newline_and_collapses_paste() {
        let mut ib = InputBox::new();
        ib.set_single_line(true);
        ib.set_text("ab\ncd"); // a line break collapses into a space
        assert_eq!(ib.text(), "ab cd");
        assert_eq!(ib.line_count(), 1);
        ib.insert_newline(); // no-op
        assert_eq!(ib.line_count(), 1);
        ib.insert_str("x\ny"); // a paste also lands as one line
        assert_eq!(ib.line_count(), 1);
        assert!(ib.text().contains("x y"));
    }

    #[test]
    fn single_line_arrows_up_down_are_noop() {
        let mut ib = InputBox::new();
        ib.set_single_line(true);
        ib.set_text("hello");
        ib.col = 2;
        assert!(ib.on_key(k(KeyCode::Up)).handled());
        assert_eq!(ib.cursor(), (0, 2));
        assert!(ib.on_key(k(KeyCode::Down)).handled());
        assert_eq!(ib.cursor(), (0, 2));
    }

    #[test]
    fn single_line_home_end_span_whole_value() {
        let mut ib = InputBox::new();
        ib.set_single_line(true);
        ib.set_text("a long value");
        render_at(&mut ib, 4); // a narrow field — the value is longer than the width
        ib.col = 5;
        assert!(ib.on_key(k(KeyCode::Home)).handled());
        assert_eq!(ib.cursor(), (0, 0));
        assert!(ib.on_key(k(KeyCode::End)).handled());
        assert_eq!(ib.cursor(), (0, 12)); // end of the whole value, not the visual row
    }

    #[test]
    fn single_line_renders_long_value_without_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut ib = InputBox::new();
        ib.set_single_line(true);
        ib.set_text("/very/long/path/to/a/gguf/model/that/does/not/fit.gguf");
        let mut term = Terminal::new(TestBackend::new(20, 3)).unwrap();
        term.draw(|f| {
            ib.render(
                f,
                f.area(),
                RenderOpts::focused("ввод"),
                &Palette::default(),
            )
        })
        .unwrap();
    }

    #[test]
    fn col_at_width_lands_on_char_boundary() {
        let line: Vec<char> = "abcdef".chars().collect();
        assert_eq!(col_at_width(&line, 0), 0);
        assert_eq!(col_at_width(&line, 3), 3);
        assert_eq!(col_at_width(&line, 100), 6); // past the end — the whole line
    }

    #[test]
    fn ctrl_left_right_move_by_word() {
        let mut ib = InputBox::new();
        ib.set_text("один два три"); // cursor at the end (col=12)
        // Ctrl+← → start of the last word
        assert!(ib.on_key(ctrl(KeyCode::Left)).handled());
        assert_eq!(ib.cursor(), (0, 9));
        // again → start of the middle word
        assert!(ib.on_key(ctrl(KeyCode::Left)).handled());
        assert_eq!(ib.cursor(), (0, 5));
        // again → start of the first word
        assert!(ib.on_key(ctrl(KeyCode::Left)).handled());
        assert_eq!(ib.cursor(), (0, 0));
        // Ctrl+→ → past the end of the first word
        assert!(ib.on_key(ctrl(KeyCode::Right)).handled());
        assert_eq!(ib.cursor(), (0, 4));
        // again → past the end of the middle word
        assert!(ib.on_key(ctrl(KeyCode::Right)).handled());
        assert_eq!(ib.cursor(), (0, 8));
    }

    #[test]
    fn ctrl_left_right_cross_logical_lines() {
        let mut ib = InputBox::new();
        ib.set_text("ab\ncd");
        ib.row = 1;
        ib.col = 0; // start of the second line
        // Ctrl+← at the line boundary → end of the previous one
        assert!(ib.on_key(ctrl(KeyCode::Left)).handled());
        assert_eq!(ib.cursor(), (0, 2));
        // Ctrl+→ from the end of the first line → start of the next one
        assert!(ib.on_key(ctrl(KeyCode::Right)).handled());
        assert_eq!(ib.cursor(), (1, 0));
    }

    #[test]
    fn ctrl_backspace_deletes_word_left() {
        let mut ib = InputBox::new();
        ib.set_text("один два три"); // cursor at the end
        assert!(ib.on_key(ctrl(KeyCode::Backspace)).handled());
        assert_eq!(ib.text(), "один два ");
        assert_eq!(ib.cursor(), (0, 9));
        // at the start of a line, joins with the line above (like a plain Backspace)
        ib.set_text("ab\ncd");
        ib.row = 1;
        ib.col = 0;
        assert!(ib.on_key(ctrl(KeyCode::Backspace)).handled());
        assert_eq!(ib.text(), "abcd");
    }

    #[test]
    fn ctrl_delete_deletes_word_right() {
        let mut ib = InputBox::new();
        ib.set_text("один два три");
        ib.col = 0;
        assert!(ib.on_key(ctrl(KeyCode::Delete)).handled());
        assert_eq!(ib.text(), " два три"); // the word "один" was deleted, the space stayed
        assert_eq!(ib.cursor(), (0, 0));
    }

    #[test]
    fn ctrl_home_end_jump_to_document_bounds() {
        let mut ib = InputBox::new();
        ib.set_text("abc\ndef\nghi");
        ib.row = 1;
        ib.col = 1;
        assert!(ib.on_key(ctrl(KeyCode::Home)).handled());
        assert_eq!(ib.cursor(), (0, 0));
        assert!(ib.on_key(ctrl(KeyCode::End)).handled());
        assert_eq!(ib.cursor(), (2, 3));
    }

    #[test]
    fn ctrl_char_is_not_inserted() {
        // Ctrl+character — a shortcut for the layer above, doesn't type into
        // the field. We take `j` (neutral; `a` is now "select all", other
        // Ctrl-letters are unhandled).
        let mut ib = InputBox::new();
        assert!(!ib.on_key(ctrl(KeyCode::Char('j'))).handled());
        assert!(ib.is_empty());
    }

    #[test]
    fn render_wrapped_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut ib = InputBox::new();
        ib.set_text("очень длинная строка которая точно не влезает в узкое поле ввода");
        ib.set_misspelled(vec![vec![(0, 5)]]);
        let mut term = Terminal::new(TestBackend::new(12, 4)).unwrap();
        term.draw(|f| {
            ib.render(
                f,
                f.area(),
                RenderOpts::focused("ввод"),
                &Palette::default(),
            )
        })
        .unwrap();
    }

    // ---------- item 1: snapping the cursor to a cluster boundary ----------

    #[test]
    fn col_for_visual_snaps_off_emoji_cluster() {
        // "❤️abc" = ❤(U+2764) + U+FE0F + a + b + c. Target column 1 falls
        // between the base and the selector → snap to the cluster's start (0),
        // not into its middle (otherwise insert/backspace would split ❤️).
        // Column 2 — right after the cluster (a boundary).
        let line: Vec<char> = "❤\u{FE0F}abc".chars().collect();
        assert_eq!(col_for_visual(&line, 0, line.len(), 1, false), 0);
        assert_eq!(col_for_visual(&line, 0, line.len(), 2, false), 2);
        // A soft wrap: the row's end on a VS16 cluster doesn't leave the cursor
        // in the middle. Row "❤️" [0,2): target past the end, soft → roll back,
        // then snap to the boundary (0).
        assert_eq!(col_for_visual(&line, 0, 2, 10, true), 0);
    }

    #[test]
    fn arrow_up_lands_on_cluster_boundary() {
        // The top row starts with ❤️; ↓ then ↑ with goal column 1 (middle of ❤️)
        // doesn't place the cursor between the base and the selector.
        let mut ib = InputBox::new();
        ib.set_text("❤\u{FE0F}xy\nz");
        render_at(&mut ib, 20);
        ib.row = 1;
        ib.col = 1; // visual column 1 on the bottom line "z"
        ib.goal_col = None;
        assert!(ib.on_key(k(KeyCode::Up)).handled());
        // on the top line the target column 1 falls into ❤️ → the cursor snaps
        // to 0 or 2, but does NOT land between the scalars (index 1 = between ❤ and U+FE0F)
        assert_ne!(
            ib.cursor(),
            (0, 1),
            "the cursor landed in the middle of the ❤️ cluster"
        );
    }

    // ---------- item 3: a hard invariant of single-line mode ----------

    #[test]
    fn single_line_after_multiline_content_merges_without_panic() {
        let mut ib = InputBox::new();
        ib.set_text("first\nsecond\nthird"); // multiline, cursor at the end
        ib.set_single_line(true); // enabled AFTER set_text — the invariant must hold
        assert_eq!(ib.line_count(), 1);
        assert_eq!(ib.text(), "first second third");
        assert_eq!(ib.cursor().0, 0);
        // rendering single-line doesn't panic (col isn't past lines[0]'s bound)
        render_at(&mut ib, 8);
    }

    // ---------- item 4: aligning hscroll to a character boundary ----------

    #[test]
    fn single_line_hscroll_aligns_to_char_boundary() {
        // "世aBcd": a leading CJK glyph of width 2. Cursor after "世a" (visual
        // column 3), a narrow field (view_w=3) → scrolling. Without alignment,
        // hscroll would land "in the middle" of 世 (a value outside the set of
        // prefix widths) and the cursor would be drawn a column to the right.
        // See spec §11.6.
        let mut ib = InputBox::new();
        ib.set_single_line(true);
        ib.set_text("世aBcd");
        ib.col = 2; // after "世a"
        render_at(&mut ib, 3);
        let line: Vec<char> = "世aBcd".chars().collect();
        let boundary_widths: Vec<usize> = (0..=line.len())
            .map(|k| wrap::display_width(&line[..k]))
            .collect();
        assert!(
            boundary_widths.contains(&ib.hscroll_for_test()),
            "hscroll={} didn't match any prefix width (not on a character boundary)",
            ib.hscroll_for_test()
        );
    }

    // ---------- item 5: syncing spelling underlines with edits ----------

    #[test]
    fn misspelled_shifts_on_insert_before_word() {
        let mut ib = InputBox::new();
        ib.set_text("foo bar");
        ib.set_misspelled(vec![vec![(4, 7)]]); // "bar" is flagged
        ib.row = 0;
        ib.col = 0;
        ib.insert_char('X'); // "Xfoo bar" — "bar" shifted to [5,8)
        assert_eq!(ib.misspelled_ranges_for_test(0), vec![(5, 8)]);
    }

    #[test]
    fn misspelled_dropped_when_edited_inside_word() {
        let mut ib = InputBox::new();
        ib.set_text("foo bar");
        ib.set_misspelled(vec![vec![(4, 7)]]);
        ib.row = 0;
        ib.col = 5; // inside "bar"
        ib.insert_char('X'); // an edit inside the word — the range is reset
        assert!(ib.misspelled_ranges_for_test(0).is_empty());
    }

    #[test]
    fn misspelled_shifts_left_on_delete_after_word() {
        let mut ib = InputBox::new();
        ib.set_text("X foo");
        ib.set_misspelled(vec![vec![(2, 5)]]); // "foo" is flagged
        ib.row = 0;
        ib.col = 0;
        ib.delete(); // deleted 'X' at the start → "foo" is now [1,4)? no: " foo" [1,4)
        assert_eq!(ib.misspelled_ranges_for_test(0), vec![(1, 4)]);
    }

    #[test]
    fn misspelled_synced_on_newline_and_join() {
        let mut ib = InputBox::new();
        ib.set_text("foo bar");
        ib.set_misspelled(vec![vec![(0, 3), (4, 7)]]);
        ib.row = 0;
        ib.col = 3; // after "foo"
        ib.insert_newline(); // "foo" | " bar": the current one is reset, the new one is empty
        assert!(ib.misspelled_ranges_for_test(0).is_empty());
        assert!(ib.misspelled_ranges_for_test(1).is_empty());
        // joining back — consistency is preserved, no panic
        ib.row = 1;
        ib.col = 0;
        ib.backspace();
        assert_eq!(ib.line_count(), 1);
    }

    #[test]
    fn set_text_and_paste_clear_misspelled() {
        let mut ib = InputBox::new();
        ib.set_text("helo");
        ib.set_misspelled(vec![vec![(0, 4)]]);
        ib.set_text("совсем другой текст"); // the old ranges of the previous text are reset
        assert!(ib.misspelled_ranges_for_test(0).is_empty());
        // a paste also resets underlines (a recheck rebuilds them)
        ib.set_misspelled(vec![vec![(0, 6)]]);
        ib.insert_str("abc");
        assert!(ib.misspelled_is_empty());
    }

    // ---------- item 6: on_key distinguishes an edit from a move ----------

    #[test]
    fn on_key_distinguishes_edit_move_ignore() {
        let mut ib = InputBox::new();
        assert_eq!(ib.on_key(k(KeyCode::Char('a'))), KeyOutcome::Edited);
        assert_eq!(ib.on_key(k(KeyCode::Left)), KeyOutcome::Moved);
        assert_eq!(ib.on_key(k(KeyCode::Backspace)), KeyOutcome::Edited);
        assert_eq!(ib.on_key(k(KeyCode::Enter)), KeyOutcome::Ignored);
        assert_eq!(ib.on_key(ctrl(KeyCode::Left)), KeyOutcome::Moved);
        assert_eq!(ib.on_key(ctrl(KeyCode::Backspace)), KeyOutcome::Edited);
        // the edited()/handled() helpers
        assert!(KeyOutcome::Edited.edited());
        assert!(!KeyOutcome::Moved.edited());
        assert!(KeyOutcome::Moved.handled());
        assert!(!KeyOutcome::Ignored.handled());
    }

    // ---------- item 7: visual-row cache + a cheap guard ----------

    #[test]
    fn row_cache_invalidates_on_every_mutator() {
        // After an edit, the visual-row cache must match a fresh recompute —
        // otherwise a `touch()` was forgotten somewhere (the cache would return
        // stale wrapping, diverging from the real content: a wrong cursor/scroll).
        fn check(setup: &str, mutate: impl FnOnce(&mut InputBox)) {
            const W: usize = 6;
            let mut ib = InputBox::new();
            ib.set_text(setup);
            let _ = ib.rows_cached(W); // fill the cache BEFORE the edit
            mutate(&mut ib);
            let cached = ib.rows_cached(W).to_vec();
            let fresh = ib.visual_rows(W);
            assert_eq!(cached, fresh, "the visual-row cache wasn't invalidated");
        }
        check("abc", |ib| {
            ib.col = 3;
            ib.insert_char('d');
        });
        check("abc", |ib| ib.replace_range(0, 0, 3, "xy"));
        check("abc", |ib| {
            ib.col = 3;
            ib.backspace();
        });
        check("ab\ncd", |ib| {
            ib.row = 1;
            ib.col = 0;
            ib.backspace(); // joining lines
        });
        check("abc", |ib| {
            ib.col = 0;
            ib.delete();
        });
        check("ab\ncd", |ib| {
            ib.row = 0;
            ib.col = 2;
            ib.delete(); // joining lines
        });
        check("abc def", |ib| {
            ib.col = 7;
            ib.delete_word_left();
        });
        check("abc def", |ib| {
            ib.col = 0;
            ib.delete_word_right();
        });
        check("abc", |ib| {
            ib.col = 1;
            ib.insert_newline();
        });
        check("abc", |ib| ib.insert_str("X\nY"));
        check("abc", |ib| ib.set_text("zzzz"));
        check("abc", |ib| ib.clear());
    }

    #[test]
    fn navigation_preserves_revision_but_edit_bumps_it() {
        // Optimization invariant: a cursor move doesn't invalidate the cache
        // (the revision doesn't grow), an edit does (the cache gets recomputed).
        let mut ib = InputBox::new();
        ib.set_text("hello world");
        render_at(&mut ib, 20);
        let r0 = ib.revision;
        assert!(ib.on_key(k(KeyCode::Left)).handled());
        assert!(ib.on_key(k(KeyCode::Home)).handled());
        assert!(ib.on_key(ctrl(KeyCode::Right)).handled());
        assert_eq!(ib.revision, r0, "navigation shouldn't invalidate the cache");
        assert!(ib.on_key(k(KeyCode::Char('!'))).handled());
        assert!(ib.revision > r0, "an edit should invalidate the cache");
    }

    #[test]
    fn first_non_whitespace_finds_leading_glyph() {
        let mut ib = InputBox::new();
        assert_eq!(ib.first_non_whitespace(), None); // empty
        ib.set_text("  /rag add x");
        assert_eq!(ib.first_non_whitespace(), Some('/'));
        ib.set_text("привет");
        assert_eq!(ib.first_non_whitespace(), Some('п'));
        ib.set_text("\n\n  x"); // leading empty lines/whitespace
        assert_eq!(ib.first_non_whitespace(), Some('x'));
        ib.set_text("   "); // whitespace only
        assert_eq!(ib.first_non_whitespace(), None);
    }

    // ---------- stage A: selection ----------

    #[test]
    fn shift_arrow_extends_selection_plain_arrow_collapses() {
        let mut ib = InputBox::new();
        ib.set_text("hello");
        ib.col = 0;
        assert!(!ib.has_selection());
        // Shift+Right ×2 → "he" is selected
        ib.on_key(shift(KeyCode::Right));
        ib.on_key(shift(KeyCode::Right));
        assert!(ib.has_selection());
        assert_eq!(ib.selected_text().as_deref(), Some("he"));
        // a plain Right clears the selection
        ib.on_key(k(KeyCode::Right));
        assert!(!ib.has_selection());
        assert_eq!(ib.selected_text(), None);
    }

    #[test]
    fn ctrl_a_selects_all() {
        let mut ib = InputBox::new();
        ib.set_text("line1\nline2");
        assert_eq!(ib.on_key(ctrl(KeyCode::Char('a'))), KeyOutcome::Moved);
        assert!(ib.has_selection());
        assert_eq!(ib.selected_text().as_deref(), Some("line1\nline2"));
        assert_eq!(ib.cursor(), (1, 5));
    }

    #[test]
    fn ctrl_shift_right_selects_word() {
        let mut ib = InputBox::new();
        ib.set_text("one two");
        ib.col = 0;
        ib.on_key(ctrl_shift(KeyCode::Right)); // to the end of "one"
        assert_eq!(ib.selected_text().as_deref(), Some("one"));
    }

    #[test]
    fn typing_replaces_selection() {
        let mut ib = InputBox::new();
        ib.set_text("hello");
        ib.on_key(ctrl(KeyCode::Char('a'))); // select all
        assert_eq!(ib.on_key(k(KeyCode::Char('X'))), KeyOutcome::Edited);
        assert_eq!(ib.text(), "X");
        assert!(!ib.has_selection());
    }

    #[test]
    fn backspace_deletes_whole_selection() {
        let mut ib = InputBox::new();
        ib.set_text("abcdef");
        ib.col = 1;
        for _ in 0..3 {
            ib.on_key(shift(KeyCode::Right)); // "bcd" is selected
        }
        assert_eq!(ib.selected_text().as_deref(), Some("bcd"));
        ib.on_key(k(KeyCode::Backspace));
        assert_eq!(ib.text(), "aef");
        assert!(!ib.has_selection());
    }

    #[test]
    fn delete_multiline_selection_merges_and_syncs_misspelled() {
        let mut ib = InputBox::new();
        ib.set_text("abc\ndef\nghi");
        ib.set_misspelled(vec![vec![(0, 3)], vec![(0, 3)], vec![(0, 3)]]);
        // selection (0,1)..(2,2): "bc\ndef\ngh"
        ib.anchor = Some((0, 1));
        ib.row = 2;
        ib.col = 2;
        assert_eq!(ib.selected_text().as_deref(), Some("bc\ndef\ngh"));
        assert!(ib.delete_selection());
        assert_eq!(ib.text(), "ai"); // "a" + "i"
        assert_eq!(ib.cursor(), (0, 1));
        assert_eq!(ib.line_count(), 1);
        // underlines are synced: one line, the first is reset
        assert!(ib.misspelled_ranges_for_test(0).is_empty());
    }

    #[test]
    fn shift_nav_is_moved_edit_over_selection_is_edited() {
        let mut ib = InputBox::new();
        ib.set_text("hello");
        render_at(&mut ib, 20);
        ib.col = 0;
        let r0 = ib.revision;
        assert_eq!(ib.on_key(shift(KeyCode::Right)), KeyOutcome::Moved);
        assert_eq!(
            ib.revision, r0,
            "extending the selection doesn't bump the revision (the cache stays intact)"
        );
        assert!(ib.has_selection());
        assert_eq!(ib.on_key(k(KeyCode::Char('Z'))), KeyOutcome::Edited);
        assert!(
            ib.revision > r0,
            "an edit over a selection invalidates the cache"
        );
    }

    #[test]
    fn render_highlights_selection_background() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut ib = InputBox::new();
        ib.set_text("hello");
        ib.on_key(ctrl(KeyCode::Char('a'))); // select all
        let pal = Palette::default();
        let mut term = Terminal::new(TestBackend::new(20, 3)).unwrap();
        term.draw(|f| ib.render(f, f.area(), RenderOpts::focused("ввод"), &pal))
            .unwrap();
        let buf = term.backend().buffer();
        let area = buf.area;
        let has_sel_bg = (area.left()..area.right()).any(|x| {
            (area.top()..area.bottom()).any(|y| buf[(x, y)].style().bg == Some(pal.keycap_bg))
        });
        assert!(
            has_sel_bg,
            "the selection isn't drawn with a keycap_bg background"
        );
    }

    #[test]
    fn paste_replaces_selection() {
        // A paste (bypassing `on_key`) also replaces the selection — deletion is
        // pushed into the `insert_str` mutator itself, so paste/`Shift+Enter`/
        // emoji respect the selection.
        let mut ib = InputBox::new();
        ib.set_text("hello");
        ib.on_key(ctrl(KeyCode::Char('a')));
        assert!(ib.has_selection());
        ib.insert_str("XY");
        assert_eq!(ib.text(), "XY");
        assert!(!ib.has_selection());
    }

    // ---------- stage D: mouse (click → cursor, drag → selection) ----------

    // `render_at(ib, inner_w)` renders into inner width `inner_w`: the border
    // adds a border (1 on the left) + the `❯` prompt column (PROMPT_W), so the
    // text area starts at screen `x = 1 + PROMPT_W = 3`, `y = 1`. Screen
    // coordinates for a click on visual cell `(vrow, vcol)` — `(3 + vcol, 1 + vrow)`.
    const TX: u16 = 1 + PROMPT_W; // the text area's left edge under render_at
    const TY: u16 = 1; // the text area's top edge

    #[test]
    fn place_cursor_at_maps_click_to_position() {
        let mut ib = InputBox::new();
        ib.set_text("hello world"); // one logical line
        render_at(&mut ib, 20); // wide — no wrapping
        // a click in the middle of the line → the cursor goes there
        assert!(ib.place_cursor_at(TX + 3, TY));
        assert_eq!(ib.cursor(), (0, 3));
        // a click past the end of the text (within the area) → the row's end
        assert!(ib.place_cursor_at(TX + 15, TY));
        assert_eq!(ib.cursor(), (0, 11));
    }

    #[test]
    fn place_cursor_below_last_row_goes_to_text_end() {
        let mut ib = InputBox::new();
        ib.set_text("abc\ndef");
        render_at(&mut ib, 20);
        // a click below the last row (but within the area's height) → the end of the text
        assert!(ib.place_cursor_at(TX, TY + 5));
        assert_eq!(ib.cursor(), (1, 3));
    }

    #[test]
    fn place_cursor_snaps_to_cluster_boundary() {
        // A click in the middle of the VS16 cluster ❤️ (❤ + U+FE0F, width 2)
        // snaps to the boundary (0), not landing between the base and the
        // selector (otherwise insert/backspace would split it).
        let mut ib = InputBox::new();
        ib.set_text("❤\u{FE0F}abc");
        render_at(&mut ib, 20);
        assert!(ib.place_cursor_at(TX + 1, TY)); // visual column 1 = the middle of ❤️
        assert_ne!(
            ib.cursor(),
            (0, 1),
            "the cursor landed in the middle of the ❤️ cluster"
        );
        assert_eq!(ib.cursor(), (0, 0));
    }

    #[test]
    fn place_cursor_outside_area_is_noop() {
        let mut ib = InputBox::new();
        ib.set_text("hello");
        render_at(&mut ib, 20);
        ib.row = 0;
        ib.col = 2;
        // a click to the left of the text area (in the prompt column/border) — doesn't move the cursor
        assert!(!ib.place_cursor_at(0, TY));
        assert_eq!(ib.cursor(), (0, 2));
    }

    #[test]
    fn place_cursor_before_render_is_noop() {
        // Before the first render last_area == None → the click is ignored.
        let mut ib = InputBox::new();
        ib.set_text("hello");
        assert!(!ib.place_cursor_at(3, 1));
    }

    #[test]
    fn mouse_press_then_drag_builds_selection() {
        let mut ib = InputBox::new();
        ib.set_text("hello world");
        render_at(&mut ib, 20);
        // a press at column 0 — the cursor goes there, no selection yet (empty)
        assert!(ib.mouse_press(TX, TY));
        assert_eq!(ib.cursor(), (0, 0));
        assert!(!ib.has_selection());
        // a drag to column 5 grows the selection "hello"
        assert!(ib.mouse_drag(TX + 5, TY));
        assert_eq!(ib.cursor(), (0, 5));
        assert!(ib.has_selection());
        assert_eq!(ib.selected_text().as_deref(), Some("hello"));
    }

    #[test]
    fn mouse_press_without_drag_is_empty_selection() {
        let mut ib = InputBox::new();
        ib.set_text("hello");
        render_at(&mut ib, 20);
        assert!(ib.mouse_press(TX + 3, TY));
        assert_eq!(ib.cursor(), (0, 3));
        assert!(!ib.has_selection()); // a click with no drag — cursor moved, no selection
    }

    #[test]
    fn mouse_press_outside_keeps_cursor() {
        let mut ib = InputBox::new();
        ib.set_text("hello");
        render_at(&mut ib, 20);
        ib.row = 0;
        ib.col = 4;
        assert!(!ib.mouse_press(0, TY)); // outside the text area
        assert_eq!(ib.cursor(), (0, 4));
        assert!(!ib.has_selection());
    }

    #[test]
    fn mouse_drag_selects_across_wrapped_rows() {
        // A drag through a soft wrap selects by logical coordinates.
        let mut ib = InputBox::new();
        ib.set_text("один два три"); // width 8: "один два " | "три"
        render_at(&mut ib, 8);
        assert!(ib.mouse_press(TX, TY)); // start of the top row (0,0)
        assert_eq!(ib.cursor(), (0, 0));
        assert!(ib.mouse_drag(TX + 1, TY + 1)); // bottom row (the last word), column 1
        assert_eq!(ib.cursor(), (0, 10)); // the last word starts at index 9 → +1 = 10
        assert_eq!(ib.selected_text().as_deref(), Some("один два т"));
    }

    #[test]
    fn scrollbar_appears_only_when_input_scrolls() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        // The "█" thumb on the right border — only when there are more rows than the visible height.
        let right_col = |term: &Terminal<TestBackend>| -> Vec<String> {
            let buf = term.backend().buffer();
            let area = buf.area;
            (area.top()..area.bottom())
                .map(|y| buf[(area.right() - 1, y)].symbol().to_string())
                .collect()
        };
        let mut ib = InputBox::new();
        ib.set_text("a\nb"); // 2 rows in an inner height of 2 — fits
        let mut term = Terminal::new(TestBackend::new(20, 4)).unwrap();
        term.draw(|f| {
            ib.render(
                f,
                f.area(),
                RenderOpts::focused("ввод"),
                &Palette::default(),
            )
        })
        .unwrap();
        assert!(
            !right_col(&term).iter().any(|s| s == "█"),
            "text that fits — no thumb"
        );
        ib.set_text("1\n2\n3\n4\n5\n6"); // 6 rows, 2 visible — scrolling
        term.draw(|f| {
            ib.render(
                f,
                f.area(),
                RenderOpts::focused("ввод"),
                &Palette::default(),
            )
        })
        .unwrap();
        assert!(
            right_col(&term).iter().any(|s| s == "█"),
            "a scrollable field — with a thumb"
        );
    }

    #[test]
    fn placeholder_is_configurable_on_unfocused_empty_field() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        // The placeholder is drawn on an empty, NOT focused field and comes
        // from `RenderOpts` (previously "type a message…" was hardcoded into
        // the generic widget, item 12).
        let render_ph = |ph: &str| -> String {
            let mut ib = InputBox::new();
            let mut term = Terminal::new(TestBackend::new(30, 3)).unwrap();
            term.draw(|f| {
                ib.render(
                    f,
                    f.area(),
                    RenderOpts {
                        title: "поле",
                        focused: false,
                        command: false,
                        placeholder: ph,
                    },
                    &Palette::default(),
                )
            })
            .unwrap();
            let buf = term.backend().buffer();
            let area = buf.area;
            (area.top()..area.bottom())
                .map(|y| {
                    (area.left()..area.right())
                        .map(|x| buf[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("")
        };
        assert!(render_ph("введите сообщение…").contains("введите сообщение"));
        // Different text — also shows up (not hardcoded): confirms configurability.
        assert!(render_ph("свой плейсхолдер").contains("свой плейсхолдер"));
    }
}

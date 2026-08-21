//! The changes screen (FSD "page", `F4` / `/changes`): what the assistant did
//! to the attached project, as a unified diff, with per-file revert.
//! See spec §9.12, [docs/history/code-workspace.md](../../docs/history/code-workspace.md) §3.5.
//!
//! It is the other half of the promise the editing tools make. They apply a
//! change without asking (design fork F1: no per-edit popup), and what makes
//! that safe is that the user can see every change afterwards and put any of
//! them back. The bytes behind it are the change journal's pre-images — the
//! only copy that exists once a file is overwritten.
//!
//! Like the other screens it is a **pure projection**: the orchestrator builds
//! the whole change set off the runtime
//! ([`crate::features::workspace_diff`]) and hands it over; this file knows
//! nothing about `app`, channels or the file system (FSD), and answers with a
//! [`ChangesIntent`]. Two panes — the files on the left, the selected file's
//! diff on the right — because a diff is wide and a path list is narrow, and
//! the settings screen already established the shape.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::features::workspace_diff::{ChangeSet, DiffKind, FileChange, FileState};
use crate::shared::i18n::Locale;
use crate::shared::keys;
use crate::shared::theme::Palette;
use crate::shared::ui::{confirm_popup, render_scrollbar, screen_chrome};

/// How far `PageUp`/`PageDown` move the diff. Fixed rather than a screenful:
/// the pane's height is only known at render time, and a fixed step is what the
/// other screens do.
const PAGE_STEP: usize = 10;

/// Widest the file pane gets. A path is usually short and a diff never is, so
/// the split is deliberately lopsided.
const FILE_PANE_WIDTH: u16 = 34;

/// Which pane the arrow keys drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Files,
    Diff,
}

/// Intent from the changes screen (translated by `app`).
#[derive(Debug, Clone, PartialEq)]
pub enum ChangesIntent {
    /// Close and go back to the chat (`Esc`).
    Close,
    /// Quit the application (`Ctrl+Q`/`F10`).
    Quit,
    /// Put this file back to how it was before the assistant touched it.
    /// Confirmed inside the screen, so `app` acts on it without asking again.
    Revert(String),
}

/// The changes screen: a change-set snapshot plus where the user is in it.
pub struct ChangesScreen {
    set: ChangeSet,
    palette: Palette,
    /// Interface locale (updated on `Settings`). See docs/i18n-ui.md.
    loc: &'static Locale,
    selected: usize,
    /// First visible line of the diff pane.
    diff_scroll: usize,
    /// First visible row of the file pane.
    file_scroll: usize,
    focus: Focus,
    /// The path a revert is being confirmed for. A revert writes over the
    /// user's file, and it is the one action here that cannot be undone by this
    /// screen — the journal row goes with it.
    confirming: Option<String>,
}

impl ChangesScreen {
    pub fn new(set: ChangeSet, palette: Palette, loc: &'static Locale) -> Self {
        Self {
            set,
            palette,
            loc,
            selected: 0,
            diff_scroll: 0,
            file_scroll: 0,
            focus: Focus::Files,
            confirming: None,
        }
    }

    /// Replaces the snapshot — a refreshed change set after a revert.
    ///
    /// The selection is kept **by path** rather than by index: reverting drops a
    /// row, so an index would silently point at the next file down and a second
    /// `r` would revert something the user never selected.
    pub fn set_changes(&mut self, set: ChangeSet) {
        let previous = self.selected_path().map(str::to_string);
        self.set = set;
        self.selected = previous
            .and_then(|path| self.set.files.iter().position(|f| f.path == path))
            .unwrap_or_else(|| self.selected.min(self.set.files.len().saturating_sub(1)));
        self.diff_scroll = 0;
        self.confirming = None;
    }

    /// Updates the theme palette (the `AppEvent::Settings` event).
    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    /// Updates the interface locale (the `AppEvent::Settings` event).
    pub fn set_loc(&mut self, loc: &'static Locale) {
        self.loc = loc;
    }

    fn selected_file(&self) -> Option<&FileChange> {
        self.set.files.get(self.selected)
    }

    fn selected_path(&self) -> Option<&str> {
        self.selected_file().map(|f| f.path.as_str())
    }

    /// Handles a key press, returning an intent for `app` (`None` — handled
    /// internally: navigation, and arming or cancelling the confirmation).
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<ChangesIntent> {
        if key.kind != KeyEventKind::Press {
            return None;
        }
        // Ctrl+Q and F10 punch through everything, including the confirmation:
        // quitting must never be behind a question the user did not ask for.
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::F(10) || (ctrl && keys::hotkey_char(&key) == Some('q')) {
            return Some(ChangesIntent::Quit);
        }
        if let Some(path) = self.confirming.clone() {
            // Enter confirms, anything else cancels — the app's own confirmation
            // keys (`ui.confirm.footer`), so this popup answers to what the
            // reader has already been taught elsewhere.
            self.confirming = None;
            return (key.code == KeyCode::Enter).then_some(ChangesIntent::Revert(path));
        }
        if ctrl {
            return None;
        }
        let last = self.set.files.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => return Some(ChangesIntent::Close),
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Files => Focus::Diff,
                    Focus::Diff => Focus::Files,
                }
            }
            KeyCode::Up => self.step(-1, last),
            KeyCode::Down => self.step(1, last),
            // Paging always moves the diff, whichever pane has focus: a file
            // list is short and a diff is long, so this is what the key is for.
            KeyCode::PageUp => self.diff_scroll = self.diff_scroll.saturating_sub(PAGE_STEP),
            KeyCode::PageDown => self.diff_scroll = self.scroll_down(PAGE_STEP),
            KeyCode::Home => match self.focus {
                Focus::Files => self.select(0),
                Focus::Diff => self.diff_scroll = 0,
            },
            KeyCode::End => match self.focus {
                Focus::Files => self.select(last),
                Focus::Diff => self.diff_scroll = usize::MAX,
            },
            KeyCode::Char(_) if keys::hotkey_char(&key) == Some('r') => {
                // Only a file that has something to put back: arming a revert on
                // a row that cannot be reverted would ask a question whose
                // answer changes nothing.
                if let Some(file) = self.selected_file().filter(|f| f.state.is_revertable()) {
                    self.confirming = Some(file.path.clone());
                }
            }
            _ => {}
        }
        None
    }

    /// One arrow step, in whichever pane has focus.
    fn step(&mut self, delta: isize, last: usize) {
        match self.focus {
            Focus::Files => {
                let next = self.selected as isize + delta;
                self.select(next.clamp(0, last as isize) as usize);
            }
            Focus::Diff if delta < 0 => self.diff_scroll = self.diff_scroll.saturating_sub(1),
            Focus::Diff => self.diff_scroll = self.scroll_down(1),
        }
    }

    /// Selects a file, putting the diff pane back to its top — the lines
    /// belong to a different file now, and keeping the offset would open it
    /// somewhere arbitrary.
    fn select(&mut self, index: usize) {
        if index != self.selected {
            self.diff_scroll = 0;
        }
        self.selected = index;
    }

    /// The diff offset `step` rows further down, clamped so the pane never
    /// scrolls past the end into blankness.
    fn scroll_down(&self, step: usize) -> usize {
        let len = self.selected_file().map_or(0, |f| f.lines.len());
        self.diff_scroll
            .saturating_add(step)
            .min(len.saturating_sub(1))
    }

    /// Draws the screen full-screen.
    pub fn render(&mut self, frame: &mut Frame) {
        let palette = self.palette;
        let loc = self.loc;
        let hk: [(&str, &str, bool); 5] = [
            ("↑↓", loc.t("ui.changes.hk.select"), false),
            ("Tab", loc.t("ui.changes.hk.pane"), false),
            ("PgUp/Dn", loc.t("ui.changes.hk.scroll"), false),
            ("R", loc.t("ui.changes.hk.revert"), false),
            ("Esc", loc.t("ui.changes.hk.back"), false),
        ];
        let chrome = screen_chrome(
            frame,
            &palette,
            format!(
                "{} {}",
                palette.glyphs().title_marker,
                loc.t("ui.changes.title")
            ),
            Some(self.summary()),
            &hk,
        );
        let (inner, status_area, hotkeys) = (chrome.inner, chrome.status, chrome.hotkeys);

        if self.set.is_empty() {
            // Nothing changed is an answer, not a blank screen — and it names
            // what would put something here (docs/lessons.md §4).
            let key = if self.set.root.is_empty() {
                "ui.changes.no_project"
            } else {
                "ui.changes.empty"
            };
            frame.render_widget(
                Paragraph::new(Line::styled(loc.t(key), palette.muted_style())),
                inner,
            );
            frame.render_widget(Paragraph::new(hotkeys), status_area);
            return;
        }

        let file_w = FILE_PANE_WIDTH.min(inner.width.saturating_sub(12)).max(12);
        // A rule between the panes, not just a gap: without it a file's counts
        // and the first diff line sit shoulder to shoulder and read as one
        // column of nonsense (`+12 −4@@ -940,7 +940,9 @@`).
        let [files_area, rule_area, diff_area] = Layout::horizontal([
            Constraint::Length(file_w),
            Constraint::Length(1),
            Constraint::Min(10),
        ])
        .areas(inner);
        self.render_files(frame, files_area);
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled("│", palette.border_style(false));
                rule_area.height as usize
            ]),
            rule_area,
        );
        self.render_diff(frame, diff_area);
        frame.render_widget(Paragraph::new(hotkeys), status_area);

        if let Some(path) = &self.confirming {
            confirm_popup(
                frame,
                &palette,
                loc.t("ui.confirm.title"),
                &loc.tf("ui.changes.confirm_revert", &[("path", path)]),
                loc.t("ui.confirm.footer"),
            );
        }
    }

    /// The right-aligned title: how many files, and the totals.
    fn summary(&self) -> String {
        let (added, removed) = self
            .set
            .files
            .iter()
            .fold((0usize, 0usize), |(a, r), f| (a + f.added, r + f.removed));
        self.loc.tf(
            "ui.changes.summary",
            &[
                ("files", &self.set.files.len().to_string()),
                ("added", &added.to_string()),
                ("removed", &removed.to_string()),
            ],
        )
    }

    /// The left pane: one row per file, with its counts or its state.
    fn render_files(&mut self, frame: &mut Frame, area: Rect) {
        let p = &self.palette;
        let focused = self.focus == Focus::Files;
        let width = area.width as usize;
        let view = area.height as usize;
        self.file_scroll = keep_visible(self.file_scroll, self.selected, view);

        // One column width for every row's counts, so they line up: sizing each
        // row to its own text puts `+12 −4` and `+1 −0` at different columns and
        // the eye cannot scan down them.
        let counts: Vec<String> = self.set.files.iter().map(|f| self.counts_of(f)).collect();
        let counts_w = counts.iter().map(|c| c.chars().count()).max().unwrap_or(0);
        let room = width.saturating_sub(1 + counts_w + 1).max(4);

        let mut lines = Vec::new();
        for (i, file) in self.set.files.iter().enumerate().skip(self.file_scroll) {
            if lines.len() >= view {
                break;
            }
            let selected = i == self.selected;
            let marker = if selected && focused { "▌" } else { " " };
            // The name is what identifies a row, so it is what survives a narrow
            // pane: the directory is trimmed from the left, not the file name
            // from the right.
            let name = elide_left(&file.path, room);
            let pad = room.saturating_sub(name.chars().count());
            lines.push(Line::from(vec![
                Span::styled(marker.to_string(), p.accent_style()),
                Span::styled(
                    format!("{name}{:pad$} ", "", pad = pad),
                    if selected {
                        Style::new().fg(p.text).add_modifier(Modifier::BOLD)
                    } else {
                        Style::new().fg(p.text)
                    },
                ),
                Span::styled(
                    format!("{:>counts_w$}", counts[i], counts_w = counts_w),
                    self.counts_style(file),
                ),
            ]));
        }
        frame.render_widget(Paragraph::new(lines), area);
        render_scrollbar(
            frame,
            area,
            self.set.files.len(),
            view,
            self.file_scroll,
            focused,
            p,
        );
    }

    /// A file's right-hand column: `+a −b` when there is a diff, otherwise the
    /// word for what happened to it.
    fn counts_of(&self, file: &FileChange) -> String {
        match file.state {
            FileState::Created if file.lines.is_empty() => self.loc.t("ui.changes.new").to_string(),
            FileState::Modified | FileState::Created => {
                format!("+{} −{}", file.added, file.removed)
            }
            FileState::Gone => self.loc.t("ui.changes.gone").to_string(),
            FileState::NotShown => self.loc.t("ui.changes.not_shown").to_string(),
            FileState::Unchanged => self.loc.t("ui.changes.unchanged").to_string(),
        }
    }

    fn counts_style(&self, file: &FileChange) -> Style {
        match file.state {
            FileState::Modified | FileState::Created => Style::new().fg(self.palette.success),
            FileState::Gone => Style::new().fg(self.palette.warning),
            _ => self.palette.muted_style(),
        }
    }

    /// The right pane: the selected file's diff, or why there is none.
    fn render_diff(&mut self, frame: &mut Frame, area: Rect) {
        let p = &self.palette;
        let loc = self.loc;
        let focused = self.focus == Focus::Diff;
        let Some(file) = self.set.files.get(self.selected) else {
            return;
        };
        let view = area.height as usize;
        if file.lines.is_empty() {
            let key = match file.state {
                FileState::Created => "ui.changes.detail_new",
                FileState::Gone => "ui.changes.detail_gone",
                FileState::NotShown => "ui.changes.detail_not_shown",
                _ => "ui.changes.detail_unchanged",
            };
            frame.render_widget(
                Paragraph::new(Line::styled(loc.t(key), p.muted_style())),
                area,
            );
            return;
        }
        self.diff_scroll = self.diff_scroll.min(file.lines.len().saturating_sub(1));
        let lines: Vec<Line<'static>> = file
            .lines
            .iter()
            .skip(self.diff_scroll)
            .take(view)
            .map(|line| {
                let (marker, style) = match line.kind {
                    DiffKind::Hunk => ("", p.accent_style()),
                    DiffKind::Added => ("+", Style::new().fg(p.success)),
                    DiffKind::Removed => ("−", Style::new().fg(p.error)),
                    DiffKind::Context => (" ", p.muted_style()),
                };
                Line::from(Span::styled(format!("{marker}{}", line.text), style))
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), area);
        render_scrollbar(
            frame,
            area,
            file.lines.len(),
            view,
            self.diff_scroll,
            focused,
            p,
        );
    }
}

impl FileState {
    /// Whether reverting this file does anything. A file already gone has no
    /// bytes to put back where they were.
    fn is_revertable(self) -> bool {
        !matches!(self, FileState::Gone)
    }
}

/// Keeps `selected` inside a `view`-row window starting at `scroll`.
fn keep_visible(scroll: usize, selected: usize, view: usize) -> usize {
    if view == 0 {
        return 0;
    }
    if selected < scroll {
        selected
    } else if selected >= scroll + view {
        selected + 1 - view
    } else {
        scroll
    }
}

/// Trims a path from the **left** to fit `width`, marking the cut with `…`.
///
/// The file name is the part that identifies a row; `src/features/tools/code.rs`
/// cut from the right becomes `src/features/to…`, which names nothing.
fn elide_left(path: &str, width: usize) -> String {
    let count = path.chars().count();
    if count <= width {
        return path.to_string();
    }
    let keep = width.saturating_sub(1);
    let tail: String = path.chars().skip(count - keep).collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::workspace_diff::DiffLine;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn loc() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    fn line(kind: DiffKind, text: &str) -> DiffLine {
        DiffLine {
            kind,
            text: text.to_string(),
        }
    }

    /// A change set of `n` modified files, each with a one-line diff, plus
    /// whatever extra rows a test needs. A fixture rather than a test opening:
    /// the third test that starts the same way as the first two is a fixture
    /// (docs/lessons.md §2).
    fn screen(files: Vec<FileChange>) -> ChangesScreen {
        ChangesScreen::new(
            ChangeSet {
                root: "D:/proj".into(),
                files,
            },
            Palette::default(),
            loc(),
        )
    }

    fn modified(path: &str) -> FileChange {
        FileChange {
            path: path.into(),
            state: FileState::Modified,
            added: 1,
            removed: 1,
            lines: vec![
                line(DiffKind::Hunk, "@@ -1,3 +1,3 @@"),
                line(DiffKind::Context, "keep"),
                line(DiffKind::Removed, "was"),
                line(DiffKind::Added, "is"),
            ],
        }
    }

    fn press(s: &mut ChangesScreen, code: KeyCode) -> Option<ChangesIntent> {
        s.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// Rendering the screen and reading the buffer back as text.
    fn text(s: &mut ChangesScreen) -> String {
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
        let buf = term.backend().buffer();
        let mut out = String::new();
        for y in buf.area.top()..buf.area.bottom() {
            for x in buf.area.left()..buf.area.right() {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn the_selected_file_diff_is_drawn() {
        let mut s = screen(vec![modified("src/a.rs"), modified("src/b.rs")]);
        let shown = text(&mut s);
        assert!(shown.contains("src/a.rs"), "{shown}");
        assert!(shown.contains("src/b.rs"), "{shown}");
        assert!(shown.contains("@@ -1,3 +1,3 @@"), "{shown}");
        assert!(shown.contains("+is"), "{shown}");
        assert!(shown.contains("−was"), "{shown}");
    }

    /// Moving the selection shows the other file's diff — the property that
    /// makes the left pane a navigation control rather than a label.
    #[test]
    fn moving_the_selection_changes_the_diff() {
        let mut a = modified("src/a.rs");
        a.lines = vec![line(DiffKind::Added, "only in a")];
        let mut b = modified("src/b.rs");
        b.lines = vec![line(DiffKind::Added, "only in b")];
        let mut s = screen(vec![a, b]);
        assert!(text(&mut s).contains("only in a"));
        press(&mut s, KeyCode::Down);
        let shown = text(&mut s);
        assert!(shown.contains("only in b"), "{shown}");
        assert!(!shown.contains("only in a"), "{shown}");
    }

    /// The selection cannot walk off either end — an out-of-range index would
    /// blank the diff pane and `r` would then arm nothing.
    #[test]
    fn the_selection_is_clamped_at_both_ends() {
        let mut s = screen(vec![modified("a"), modified("b")]);
        for _ in 0..5 {
            press(&mut s, KeyCode::Up);
        }
        assert_eq!(s.selected, 0);
        for _ in 0..5 {
            press(&mut s, KeyCode::Down);
        }
        assert_eq!(s.selected, 1);
    }

    /// `r` asks first, and only `Enter` answers yes. A revert writes over the
    /// user's file and drops the journal row with it — this screen cannot undo
    /// it, so it is the one action here that has to be confirmed.
    #[test]
    fn revert_is_confirmed_before_it_is_reported() {
        let mut s = screen(vec![modified("src/a.rs")]);
        assert_eq!(
            press(&mut s, KeyCode::Char('r')),
            None,
            "r must not act yet"
        );
        assert_eq!(s.confirming.as_deref(), Some("src/a.rs"));
        let shown = text(&mut s);
        assert!(
            shown.contains("src/a.rs"),
            "the question must name the file: {shown}"
        );
        assert_eq!(
            press(&mut s, KeyCode::Enter),
            Some(ChangesIntent::Revert("src/a.rs".into()))
        );
        assert_eq!(s.confirming, None);
    }

    #[test]
    fn any_other_key_cancels_the_confirmation() {
        let mut s = screen(vec![modified("src/a.rs")]);
        press(&mut s, KeyCode::Char('r'));
        assert_eq!(press(&mut s, KeyCode::Esc), None, "Esc cancels, not closes");
        assert_eq!(s.confirming, None);
        // …and the screen is still open: a second Esc is what closes it.
        assert_eq!(press(&mut s, KeyCode::Esc), Some(ChangesIntent::Close));
    }

    /// A file that is no longer on disk has nothing to put back, so `r` must not
    /// ask a question whose answer changes nothing.
    #[test]
    fn a_missing_file_cannot_be_reverted() {
        let mut s = screen(vec![FileChange {
            path: "gone.rs".into(),
            state: FileState::Gone,
            added: 0,
            removed: 0,
            lines: Vec::new(),
        }]);
        press(&mut s, KeyCode::Char('r'));
        assert_eq!(s.confirming, None);
    }

    /// Quitting must never sit behind a question the user did not ask for.
    #[test]
    fn quit_punches_through_the_confirmation() {
        let mut s = screen(vec![modified("a")]);
        press(&mut s, KeyCode::Char('r'));
        assert_eq!(press(&mut s, KeyCode::F(10)), Some(ChangesIntent::Quit));
    }

    /// After a revert the snapshot is rebuilt, and the selection has to follow
    /// the **path**: an index would point at the next file down, and a second
    /// `r` would revert something the user never selected.
    #[test]
    fn the_selection_follows_the_file_not_the_index() {
        let mut s = screen(vec![modified("a"), modified("b"), modified("c")]);
        press(&mut s, KeyCode::Down);
        press(&mut s, KeyCode::Down);
        assert_eq!(s.selected_path(), Some("c"));
        // "a" was reverted elsewhere and is gone from the set.
        s.set_changes(ChangeSet {
            root: "D:/proj".into(),
            files: vec![modified("b"), modified("c")],
        });
        assert_eq!(s.selected_path(), Some("c"));
    }

    /// …and when the selected file is the one that went, the index is clamped
    /// rather than left dangling past the end.
    #[test]
    fn losing_the_selected_file_clamps_rather_than_dangles() {
        let mut s = screen(vec![modified("a"), modified("b")]);
        press(&mut s, KeyCode::Down);
        s.set_changes(ChangeSet {
            root: "D:/proj".into(),
            files: vec![modified("a")],
        });
        assert_eq!(s.selected_path(), Some("a"));
    }

    /// Every state has to say what it is: an empty diff pane for all of them
    /// would be indistinguishable from a bug (docs/lessons.md §4). Looped over
    /// the states rather than written five times.
    #[test]
    fn every_state_explains_itself_in_the_diff_pane() {
        for state in [
            FileState::Gone,
            FileState::NotShown,
            FileState::Unchanged,
            FileState::Created,
        ] {
            let mut s = screen(vec![FileChange {
                path: "f.rs".into(),
                state,
                added: 0,
                removed: 0,
                lines: Vec::new(),
            }]);
            let shown = text(&mut s);
            // The pane is not blank, and the row says something other than a
            // bare count.
            let body: String = shown.lines().skip(2).take(6).collect();
            assert!(
                body.trim().chars().filter(|c| !c.is_whitespace()).count() > 10,
                "{state:?} rendered nothing: {shown}"
            );
        }
    }

    /// Nothing to show is an answer, and it names what would put something here.
    #[test]
    fn an_empty_change_set_says_so() {
        let mut s = ChangesScreen::new(ChangeSet::default(), Palette::default(), loc());
        let shown = text(&mut s);
        assert!(
            shown.contains("/project"),
            "with no project it must name the command that attaches one: {shown}"
        );

        let mut with_project = ChangesScreen::new(
            ChangeSet {
                root: "D:/proj".into(),
                files: Vec::new(),
            },
            Palette::default(),
            loc(),
        );
        let shown = text(&mut with_project);
        assert!(shown.contains(loc().t("ui.changes.empty")), "{shown}");
    }

    /// The two panes have a rule between them.
    ///
    /// Found by looking at a render rather than by an assertion: with the panes
    /// merely adjacent, a file's counts and the first diff line sit shoulder to
    /// shoulder — `+12 −4@@ -940,7 +940,9 @@` — which reads as one column of
    /// nonsense. The symptom is what this pins.
    #[test]
    fn the_panes_are_separated() {
        let mut file = modified("src/a.rs");
        file.lines = vec![line(DiffKind::Hunk, "@@ -940,7 +940,9 @@")];
        let mut s = screen(vec![file]);
        let shown = text(&mut s);
        assert!(
            !shown.contains("−1@@"),
            "the file row runs straight into the diff: {shown}"
        );
        let row = shown
            .lines()
            .find(|l| l.contains("src/a.rs"))
            .expect("the file row");
        let bar = row.find('│').expect("the panel border");
        assert!(
            row[bar + '│'.len_utf8()..].contains('│'),
            "no rule between the panes: {row}"
        );
    }

    /// Every row's counts sit in one column. Sized per row, `+12 −4` and
    /// `+1 −0` start at different offsets and the eye cannot scan down them.
    #[test]
    fn the_counts_line_up_across_rows() {
        let mut wide = modified("src/a.rs");
        wide.added = 12;
        wide.removed = 4;
        let narrow = modified("src/b.rs");
        let mut s = screen(vec![wide, narrow]);
        let shown = text(&mut s);
        let end_of = |needle: &str| -> usize {
            let row = shown
                .lines()
                .find(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("no row with {needle}: {shown}"));
            let at = row.find(needle).unwrap();
            row[..at + needle.len()].chars().count()
        };
        assert_eq!(
            end_of("+12 −4"),
            end_of("+1 −1"),
            "the counts columns are ragged: {shown}"
        );
    }

    /// A long path keeps its **file name**: cut from the right,
    /// `src/features/tools/code.rs` becomes `src/features/to…`, which names
    /// nothing the user can act on.
    #[test]
    fn a_long_path_keeps_its_file_name() {
        assert_eq!(elide_left("src/a.rs", 20), "src/a.rs");
        let cut = elide_left("src/features/tools/code.rs", 14);
        assert!(cut.ends_with("code.rs"), "got: {cut}");
        assert!(cut.starts_with('…'), "got: {cut}");
        assert_eq!(cut.chars().count(), 14);
    }

    /// The diff pane scrolls, and cannot be scrolled into blankness past the
    /// end — the shape a fixed page step gets wrong when the diff is short.
    #[test]
    fn the_diff_pane_scrolls_within_its_bounds() {
        let mut file = modified("a");
        file.lines = (0..50)
            .map(|i| line(DiffKind::Context, &format!("line {i}")))
            .collect();
        let mut s = screen(vec![file]);
        press(&mut s, KeyCode::PageDown);
        assert_eq!(s.diff_scroll, PAGE_STEP);
        for _ in 0..20 {
            press(&mut s, KeyCode::PageDown);
        }
        assert_eq!(s.diff_scroll, 49, "must stop at the last line");
        press(&mut s, KeyCode::PageUp);
        assert_eq!(s.diff_scroll, 39);
    }

    /// Selecting another file opens its diff at the top: the offset belonged to
    /// a different file, and keeping it would land somewhere arbitrary.
    #[test]
    fn changing_file_resets_the_diff_offset() {
        let mut long = modified("a");
        long.lines = (0..50)
            .map(|i| line(DiffKind::Context, &format!("line {i}")))
            .collect();
        let mut s = screen(vec![long, modified("b")]);
        press(&mut s, KeyCode::PageDown);
        assert!(s.diff_scroll > 0);
        press(&mut s, KeyCode::Down);
        assert_eq!(s.diff_scroll, 0);
    }

    /// `Tab` hands the arrows to the diff pane — the reason it exists, since
    /// paging alone cannot walk a diff line by line.
    #[test]
    fn tab_moves_the_arrows_to_the_diff() {
        let mut file = modified("a");
        file.lines = (0..50)
            .map(|i| line(DiffKind::Context, &format!("line {i}")))
            .collect();
        let mut s = screen(vec![file, modified("b")]);
        press(&mut s, KeyCode::Tab);
        press(&mut s, KeyCode::Down);
        assert_eq!(s.selected, 0, "the file selection must not move");
        assert_eq!(s.diff_scroll, 1);
        press(&mut s, KeyCode::Tab);
        press(&mut s, KeyCode::Down);
        assert_eq!(s.selected, 1);
    }
}

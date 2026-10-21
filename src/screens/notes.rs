//! The notes screen (FSD "page", `/notes`): what the assistant wrote down about
//! the person and their world, readable and editable by the person — every
//! note of the active profile (not its `@self` observations, which are `F3`'s),
//! a search, a note opened with its links and the notes it superseded, its
//! text and tags edited, the note deleted. And **what this conversation was
//! shown**: the notes and the observations its prompt carries, picked as a
//! turn picks them. See spec §11.11, docs/research/companion-platform.md C3.
//!
//! A pure projection, like the other screens: the orchestrator builds the
//! snapshot ([`NotesView`]) and applies the edits ([`NoteEdit`]); this file
//! knows nothing about `app` or storage (FSD). The screen opens at once and
//! says it is waiting until the first snapshot lands (the tasks screen's
//! shape), since picking what the prompt carries embeds the opening.

use chrono::{DateTime, Utc};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use uuid::Uuid;

use crate::entities::note::{NoteCard, NoteEdit, NotesView};
use crate::entities::self_model::age_label;
use crate::features::tools::notes::SELF_NOTE_TAG;
use crate::shared::i18n::Locale;
use crate::shared::keys;
use crate::shared::theme::Palette;
use crate::shared::ui::{
    MinSize, SCREEN_MIN_SIZE, dim_background, keep_visible, render_scrollbar, screen_chrome,
};
use crate::shared::wrap::{truncate_to_width, wrap_line};
use crate::widgets::help_dialog::{HelpContext, HelpSection};
use crate::widgets::input_box::{InputBox, RenderOpts};

/// The notes screen's "Shortcuts" section (`F1`): one row per key
/// `handle_key` matches — a new arm gets a row here (AGENTS.md §3).
pub(crate) static HELP_SECTION: HelpSection = HelpSection {
    title: "ui.help.sec.notes",
    context: Some(HelpContext::Notes),
    rows: &[
        ("↑/↓", "ui.help.nt_select"),
        ("Enter", "ui.help.nt_open"),
        ("E", "ui.help.nt_edit"),
        ("T", "ui.help.nt_tags"),
        ("Del", "ui.help.nt_delete"),
        ("/", "ui.help.nt_search"),
        ("Esc", "ui.help.nt_back"),
    ],
    openers: &[],
};

/// How far `PageUp`/`PageDown` move: fixed, like the other screens.
const PAGE_STEP: usize = 10;

/// The glyph marking a note this conversation's prompt carries.
const SHOWN_MARK: &str = "◆";

/// Intent from the notes screen (translated by `app`).
#[derive(Debug, Clone, PartialEq)]
pub enum NotesIntent {
    /// Close and go back to the chat (`Esc`).
    Close,
    /// Quit the application (`Ctrl+Q`/`F10`).
    Quit,
    /// Apply an edit; the orchestrator answers with a fresh snapshot.
    Edit(NoteEdit),
}

/// Which view is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    List,
    /// One note, opened from the list.
    Note(Uuid),
}

/// What the open editor changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Text(Uuid),
    Tags(Uuid),
}

struct Editor {
    kind: EditKind,
    input: InputBox,
}

/// The notes screen: the latest snapshot plus where the person is in it.
pub struct NotesScreen {
    /// `None` until the orchestrator's first reply — the screen says it is
    /// waiting rather than showing an empty list, which would read as "no
    /// notes" (docs/lessons.md §4).
    view: Option<NotesView>,
    palette: Palette,
    loc: &'static Locale,
    mode: Mode,
    /// Index into [`Self::visible`] — the list's selection.
    selected: usize,
    /// First visible display line of the list.
    scroll: usize,
    /// First visible display line of an opened note.
    note_scroll: usize,
    /// The search being typed (`/`), narrowing the list as it is typed.
    search: Option<InputBox>,
    /// The search in force: what `search` held when it was closed with `Enter`,
    /// or what it holds while open.
    query: String,
    editor: Option<Editor>,
    /// A delete asked and not yet confirmed: `Del` again deletes, any other key
    /// keeps the note. A note is deleted outright, so the key asks twice.
    confirm_delete: Option<Uuid>,
}

impl NotesScreen {
    pub fn new(palette: Palette, loc: &'static Locale) -> Self {
        Self {
            view: None,
            palette,
            loc,
            mode: Mode::List,
            selected: 0,
            scroll: 0,
            note_scroll: 0,
            search: None,
            query: String::new(),
            editor: None,
            confirm_delete: None,
        }
    }

    /// Replaces the snapshot — the reply to the opening request or to an edit.
    /// The selection is kept by the note's id; an opened note that is gone
    /// (deleted) returns to the list.
    pub fn set_view(&mut self, view: NotesView) {
        let previous = self.selected_id();
        self.view = Some(view);
        if let Mode::Note(id) = self.mode
            && self.card(id).is_none()
        {
            self.mode = Mode::List;
        }
        let visible = self.visible();
        self.selected = previous
            .and_then(|id| visible.iter().position(|c| c.note.id == id))
            .unwrap_or_else(|| self.selected.min(visible.len().saturating_sub(1)));
    }

    /// Updates the theme palette (the `AppEvent::Settings` event).
    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    /// Updates the interface locale (the `AppEvent::Settings` event).
    pub fn set_loc(&mut self, loc: &'static Locale) {
        self.loc = loc;
    }

    /// Clipboard paste — into the open editor or the search (otherwise a no-op).
    pub fn handle_paste(&mut self, text: &str) {
        if let Some(editor) = self.editor.as_mut() {
            editor.input.insert_str(text);
        } else if let Some(search) = self.search.as_mut() {
            search.insert_str(text);
            self.query = search.text();
            self.selected = 0;
        }
    }

    fn card(&self, id: Uuid) -> Option<&NoteCard> {
        self.view.as_ref()?.notes.iter().find(|c| c.note.id == id)
    }

    /// The notes the list shows: all of them, or those matching the search —
    /// in their text or their tags, ignoring case.
    fn visible(&self) -> Vec<&NoteCard> {
        let Some(view) = &self.view else {
            return Vec::new();
        };
        let q = self.query.trim().to_lowercase();
        view.notes
            .iter()
            .filter(|c| {
                q.is_empty()
                    || c.note.content.to_lowercase().contains(&q)
                    || c.note.tags.iter().any(|t| t.to_lowercase().contains(&q))
            })
            .collect()
    }

    fn selected_id(&self) -> Option<Uuid> {
        self.visible().get(self.selected).map(|c| c.note.id)
    }

    /// The note the keys act on: the opened one, or the list's selection.
    fn target(&self) -> Option<Uuid> {
        match self.mode {
            Mode::Note(id) => Some(id),
            Mode::List => self.selected_id(),
        }
    }

    fn is_shown(&self, id: Uuid) -> bool {
        self.view.as_ref().is_some_and(|v| v.shown.contains(&id))
    }

    /// Handles a key press, returning an intent for `app` (`None` — handled
    /// here: navigation, the search, the editor).
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<NotesIntent> {
        if key.kind != KeyEventKind::Press {
            return None;
        }
        if self.editor.is_some() {
            return self.handle_editor_key(key);
        }
        if self.search.is_some() {
            self.handle_search_key(key);
            return None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let phys = keys::hotkey_char(&key);
        if key.code == KeyCode::F(10) || (ctrl && phys == Some('q')) {
            return Some(NotesIntent::Quit);
        }
        // The delete's question: `Del` again deletes, anything else keeps it.
        if let Some(id) = self.confirm_delete.take() {
            return (key.code == KeyCode::Delete)
                .then_some(NotesIntent::Edit(NoteEdit::Delete(id)));
        }
        if ctrl {
            return None;
        }
        match key.code {
            KeyCode::Esc => return self.back(),
            KeyCode::Delete => {
                self.confirm_delete = self.target();
                return None;
            }
            KeyCode::Enter if self.mode == Mode::List => {
                if let Some(id) = self.selected_id() {
                    self.mode = Mode::Note(id);
                    self.note_scroll = 0;
                }
                return None;
            }
            KeyCode::Char(_) if phys == Some('e') => {
                self.open_editor(EditKind::Text);
                return None;
            }
            KeyCode::Char(_) if phys == Some('t') => {
                self.open_editor(EditKind::Tags);
                return None;
            }
            // `/`, or `.` — the same physical key under a Russian layout.
            KeyCode::Char('/' | '.') if self.mode == Mode::List => {
                let mut input = InputBox::new();
                input.set_single_line(true);
                input.set_text(&self.query);
                self.search = Some(input);
                return None;
            }
            _ => {}
        }
        match self.mode {
            Mode::List => self.move_selection(key.code),
            Mode::Note(_) => self.scroll_note(key.code),
        }
        None
    }

    /// `Esc`: an opened note goes back to the list, a search in force is
    /// dropped, and only then the screen closes.
    fn back(&mut self) -> Option<NotesIntent> {
        if let Mode::Note(_) = self.mode {
            self.mode = Mode::List;
            return None;
        }
        if !self.query.is_empty() {
            self.query.clear();
            self.selected = 0;
            return None;
        }
        Some(NotesIntent::Close)
    }

    fn move_selection(&mut self, code: KeyCode) {
        let last = self.visible().len().saturating_sub(1);
        self.selected = match code {
            KeyCode::Up => self.selected.saturating_sub(1),
            KeyCode::Down => (self.selected + 1).min(last),
            KeyCode::PageUp => self.selected.saturating_sub(PAGE_STEP),
            KeyCode::PageDown => (self.selected + PAGE_STEP).min(last),
            KeyCode::Home => 0,
            KeyCode::End => last,
            _ => self.selected,
        };
    }

    /// An opened note scrolls; the render clamps the offset to the text.
    fn scroll_note(&mut self, code: KeyCode) {
        self.note_scroll = match code {
            KeyCode::Up => self.note_scroll.saturating_sub(1),
            KeyCode::Down => self.note_scroll + 1,
            KeyCode::PageUp => self.note_scroll.saturating_sub(PAGE_STEP),
            KeyCode::PageDown => self.note_scroll + PAGE_STEP,
            KeyCode::Home => 0,
            KeyCode::End => usize::MAX,
            _ => self.note_scroll,
        };
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        match key.code {
            // `Enter` keeps the search in force; `Esc` drops it.
            KeyCode::Enter => self.search = None,
            KeyCode::Esc => {
                self.search = None;
                self.query.clear();
            }
            _ => {
                search.on_key(key);
                self.query = search.text();
            }
        }
        self.selected = 0;
        self.scroll = 0;
    }

    fn open_editor(&mut self, kind: fn(Uuid) -> EditKind) {
        let Some(id) = self.target() else {
            return;
        };
        let Some(card) = self.card(id) else {
            return;
        };
        let kind = kind(id);
        let mut input = InputBox::new();
        match kind {
            EditKind::Text(_) => input.set_text(&card.note.content),
            EditKind::Tags(_) => {
                input.set_single_line(true);
                input.set_text(&shown_tags(&card.note.tags).join(", "));
            }
        }
        self.editor = Some(Editor { kind, input });
    }

    fn handle_editor_key(&mut self, key: KeyEvent) -> Option<NotesIntent> {
        let editor = self.editor.as_mut()?;
        match key.code {
            KeyCode::Esc => {
                self.editor = None;
                None
            }
            _ if matches!(editor.kind, EditKind::Text(_)) && keys::is_line_break(&key) => {
                editor.input.insert_newline();
                None
            }
            KeyCode::Enter => {
                let editor = self.editor.take()?;
                let text = editor.input.text();
                match editor.kind {
                    // An emptied text is no edit: deleting is `Del`'s job.
                    EditKind::Text(id) => (!text.trim().is_empty())
                        .then_some(NotesIntent::Edit(NoteEdit::SetText { id, text })),
                    EditKind::Tags(id) => Some(NotesIntent::Edit(NoteEdit::SetTags {
                        id,
                        tags: parse_tags(&text),
                    })),
                }
            }
            _ => {
                editor.input.on_key(key);
                None
            }
        }
    }

    /// The footer's hints — built from the same state `handle_key` dispatches
    /// on, so a key that would do nothing is not advertised (spec §11.2).
    fn hints(&self) -> Vec<(&'static str, &'static str, bool)> {
        let loc = self.loc;
        if self.search.is_some() {
            return vec![
                ("Enter", loc.t("ui.notes.hk.search_done"), false),
                ("Esc", loc.t("ui.notes.hk.search_clear"), false),
            ];
        }
        let mut hk = Vec::new();
        let target = self.target().is_some();
        match self.mode {
            Mode::List => {
                hk.push(("↑↓", loc.t("ui.notes.hk.select"), false));
                if target {
                    hk.push(("Enter", loc.t("ui.notes.hk.open"), false));
                }
            }
            Mode::Note(_) => hk.push(("↑↓", loc.t("ui.notes.hk.scroll"), false)),
        }
        if target {
            hk.push(("E", loc.t("ui.notes.hk.edit"), false));
            hk.push(("T", loc.t("ui.notes.hk.tags"), false));
            hk.push(("Del", loc.t("ui.notes.hk.delete"), true));
        }
        if self.mode == Mode::List {
            hk.push(("/", loc.t("ui.notes.hk.search"), false));
        }
        let esc = match self.mode {
            Mode::Note(_) => loc.t("ui.notes.hk.to_list"),
            Mode::List if !self.query.is_empty() => loc.t("ui.notes.hk.search_clear"),
            Mode::List => loc.t("ui.notes.hk.back"),
        };
        hk.extend([
            ("F1", loc.t("ui.notes.hk.help"), false),
            ("Esc", esc, false),
            ("Ctrl+Q", loc.t("ui.notes.hk.quit"), false),
        ]);
        hk
    }

    /// The smallest window the screen is drawn in (spec §11.1.1).
    pub fn min_size(&self) -> MinSize {
        SCREEN_MIN_SIZE
    }

    /// Draws the screen full-screen; the editor as a popup over it.
    pub fn render(&mut self, frame: &mut Frame) {
        let palette = self.palette;
        let loc = self.loc;
        let hk = if self.confirm_delete.is_some() {
            Vec::new()
        } else {
            self.hints()
        };
        let right = self
            .view
            .as_ref()
            .map(|v| loc.tf("ui.notes.count", &[("n", &v.notes.len().to_string())]));
        let chrome = screen_chrome(
            frame,
            &palette,
            format!(
                "{} {}",
                palette.glyphs().title_marker,
                loc.t("ui.notes.title")
            ),
            right,
            &hk,
            SCREEN_MIN_SIZE,
        );
        if self.confirm_delete.is_some() {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    loc.t("ui.notes.confirm_delete"),
                    Style::new().fg(palette.warning),
                )),
                chrome.status,
            );
        } else {
            frame.render_widget(Paragraph::new(chrome.hotkeys), chrome.status);
        }
        let inner = chrome.inner;
        let mut body = inner;
        if let Some(search) = self.search.as_mut() {
            // The search takes the panel's first rows while it is typed.
            let h = 3.min(inner.height);
            let area = Rect { height: h, ..inner };
            search.render(
                frame,
                area,
                RenderOpts::focused(loc.t("ui.notes.search_title")),
                &palette,
            );
            body = Rect {
                y: inner.y + h,
                height: inner.height - h,
                ..inner
            };
        }
        if self.view.is_none() {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    loc.t("ui.notes.loading"),
                    palette.muted_style(),
                )),
                body,
            );
            return;
        }
        let width = (body.width as usize).saturating_sub(1).max(1);
        let now = Utc::now();
        let (lines, focus) = match self.mode {
            Mode::List => self.list_lines(width, now),
            Mode::Note(id) => (self.note_lines(id, width, now), None),
        };
        let view_h = body.height as usize;
        let total = lines.len();
        let scroll = match (self.mode, focus) {
            (Mode::List, Some(line)) => {
                self.scroll = keep_visible(self.scroll, line, view_h);
                self.scroll
            }
            (Mode::List, None) => {
                self.scroll = self.scroll.min(total.saturating_sub(view_h));
                self.scroll
            }
            (Mode::Note(_), _) => {
                self.note_scroll = self.note_scroll.min(total.saturating_sub(view_h));
                self.note_scroll
            }
        };
        let shown: Vec<Line<'static>> = lines.into_iter().skip(scroll).take(view_h).collect();
        frame.render_widget(Paragraph::new(shown), body);
        render_scrollbar(frame, chrome.panel, total, view_h, scroll, true, &palette);
        self.render_editor(frame);
    }

    /// The list: what this conversation's prompt carries, then the notes, one
    /// line each. Returns the lines and the display line of the selection.
    fn list_lines(&self, width: usize, now: DateTime<Utc>) -> (Vec<Line<'static>>, Option<usize>) {
        let p = &self.palette;
        let loc = self.loc;
        let view = self.view.as_ref().expect("checked by render");
        let muted = p.muted_style();
        let mut out: Vec<Line<'static>> = Vec::new();
        // What this conversation was shown (C3 (b)).
        let about = if !view.notes_in_prompt {
            loc.t("ui.notes.prompt_off").to_string()
        } else if view.shown.is_empty() {
            loc.t("ui.notes.prompt_none").to_string()
        } else {
            loc.tf(
                "ui.notes.prompt_some",
                &[("n", &view.shown.len().to_string()), ("mark", SHOWN_MARK)],
            )
        };
        out.extend(wrap_line(&Line::styled(about, muted), width));
        if !view.observations.is_empty() {
            out.extend(wrap_line(
                &Line::styled(loc.t("ui.notes.prompt_observations").to_string(), muted),
                width,
            ));
            for o in &view.observations {
                let line = Line::styled(
                    format!(
                        "  ({}) {}",
                        age_label(o.created_at, now, loc),
                        one_line(&o.text)
                    ),
                    muted,
                );
                out.push(clip(&line, width));
            }
        }
        out.push(Line::default());
        let visible = self.visible();
        let header = if self.query.is_empty() {
            loc.tf(
                "ui.notes.header_all",
                &[("n", &view.notes.len().to_string())],
            )
        } else {
            loc.tf(
                "ui.notes.header_found",
                &[
                    ("q", self.query.trim()),
                    ("n", &visible.len().to_string()),
                    ("total", &view.notes.len().to_string()),
                ],
            )
        };
        out.push(Line::from(Span::styled(
            header,
            Style::new().fg(p.accent).add_modifier(Modifier::BOLD),
        )));
        if visible.is_empty() {
            let key = if view.notes.is_empty() {
                "ui.notes.empty"
            } else {
                "ui.notes.no_match"
            };
            out.extend(wrap_line(
                &Line::styled(loc.t(key).to_string(), muted),
                width,
            ));
            return (out, None);
        }
        let first_row = out.len();
        for (i, card) in visible.iter().enumerate() {
            let selected = i == self.selected;
            let mark = if self.is_shown(card.note.id) {
                SHOWN_MARK
            } else {
                " "
            };
            let tags = shown_tags(&card.note.tags);
            let mut spans = vec![
                Span::raw(if selected { "▌" } else { " " }),
                Span::styled(format!("{mark} "), Style::new().fg(p.accent)),
                Span::styled(
                    format!("({}) ", age_label(card.note.updated_at, now, loc)),
                    muted,
                ),
                Span::raw(one_line(&card.note.content)),
            ];
            if !tags.is_empty() {
                spans.push(Span::styled(format!("  [{}]", tags.join(", ")), muted));
            }
            let mut line = clip(&Line::from(spans), width);
            if selected {
                line = line.style(Style::new().bg(p.keycap_bg));
            }
            out.push(line);
        }
        (out, Some(first_row + self.selected))
    }

    /// An opened note: its dates and tags, its whole text, its links, and the
    /// notes it superseded.
    fn note_lines(&self, id: Uuid, width: usize, now: DateTime<Utc>) -> Vec<Line<'static>> {
        let p = &self.palette;
        let loc = self.loc;
        let muted = p.muted_style();
        let accent = Style::new().fg(p.accent).add_modifier(Modifier::BOLD);
        let Some(card) = self.card(id) else {
            return Vec::new();
        };
        let n = &card.note;
        let mut out: Vec<Line<'static>> = Vec::new();
        let push = |line: Line<'static>, out: &mut Vec<Line<'static>>| {
            out.extend(wrap_line(&line, width));
        };
        push(
            Line::styled(
                loc.tf(
                    "ui.notes.dates",
                    &[
                        ("created", &stamp(n.created_at)),
                        ("updated", &stamp(n.updated_at)),
                    ],
                ),
                muted,
            ),
            &mut out,
        );
        let tags = shown_tags(&n.tags);
        push(
            Line::styled(
                if tags.is_empty() {
                    loc.t("ui.notes.no_tags").to_string()
                } else {
                    loc.tf("ui.notes.tags", &[("tags", &tags.join(", "))])
                },
                muted,
            ),
            &mut out,
        );
        if self.is_shown(id) {
            push(
                Line::styled(
                    loc.tf("ui.notes.in_prompt", &[("mark", SHOWN_MARK)]),
                    Style::new().fg(p.accent),
                ),
                &mut out,
            );
        }
        out.push(Line::default());
        for para in n.content.lines() {
            push(Line::raw(para.to_string()), &mut out);
        }
        if !card.links.is_empty() {
            out.push(Line::default());
            push(
                Line::styled(
                    loc.tf("ui.notes.links", &[("n", &card.links.len().to_string())]),
                    accent,
                ),
                &mut out,
            );
            for l in &card.links {
                let arrow = if l.outgoing { "→" } else { "←" };
                let mark = if l.other.tags.iter().any(|t| t == SELF_NOTE_TAG) {
                    format!("{} ", loc.t("ui.notes.observation_mark"))
                } else {
                    String::new()
                };
                push(
                    Line::raw(format!(
                        "{arrow} {}: {mark}{}",
                        l.relation,
                        one_line(&l.other.content)
                    )),
                    &mut out,
                );
            }
        }
        if !card.supersedes.is_empty() {
            out.push(Line::default());
            push(
                Line::styled(
                    loc.tf(
                        "ui.notes.supersedes",
                        &[("n", &card.supersedes.len().to_string())],
                    ),
                    accent,
                ),
                &mut out,
            );
            for old in &card.supersedes {
                push(
                    Line::styled(
                        format!(
                            "({}) {}",
                            age_label(old.created_at, now, loc),
                            one_line(&old.content)
                        ),
                        muted,
                    ),
                    &mut out,
                );
            }
        }
        out
    }

    /// The editor drawn on top, with a real cursor.
    fn render_editor(&mut self, frame: &mut Frame) {
        let palette = self.palette;
        let loc = self.loc;
        let area = frame.area();
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let (popup, title) = match editor.kind {
            EditKind::Text(_) => (
                centered(80, 50, 5, area),
                loc.tf(
                    "ui.editor.multiline_footer",
                    &[("newline", keys::newline_chord())],
                ),
            ),
            EditKind::Tags(_) => (
                centered(80, 0, 3, area),
                loc.t("ui.notes.tags_title").to_string(),
            ),
        };
        dim_background(frame, &palette);
        frame.render_widget(Clear, popup);
        editor
            .input
            .render(frame, popup, RenderOpts::focused(&title), &palette);
    }
}

/// A moment as the opened note shows it: the local date and time, exact where
/// the list's ages ("5d") are coarse.
fn stamp(at: DateTime<Utc>) -> String {
    at.with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// Tags as the person sees them: never the reserved `@self`.
fn shown_tags(tags: &[String]) -> Vec<&str> {
    tags.iter()
        .map(String::as_str)
        .filter(|t| *t != SELF_NOTE_TAG)
        .collect()
}

/// Typed tags: comma-separated, trimmed, empty ones dropped.
fn parse_tags(text: &str) -> Vec<String> {
    text.split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// A note's text on one row: whitespace runs, line breaks included, collapsed.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A line cut to `width` columns, an ellipsis where it was cut.
fn clip(line: &Line<'static>, width: usize) -> Line<'static> {
    let mut left = width;
    let mut spans: Vec<Span<'static>> = Vec::new();
    let total: usize = line
        .spans
        .iter()
        .map(|s| crate::shared::wrap::str_width(&s.content))
        .sum();
    if total <= width {
        return line.clone();
    }
    let budget = width.saturating_sub(1);
    left = left.min(budget);
    for s in &line.spans {
        if left == 0 {
            break;
        }
        let (cut, used) = truncate_to_width(&s.content, left);
        left -= used;
        spans.push(Span::styled(cut, s.style));
    }
    spans.push(Span::raw("…"));
    Line::from(spans).style(line.style)
}

/// A centered popup: `pct_x` of the width; `pct_y` of the height, or exactly
/// `min_h` rows when `pct_y` is 0 (a one-line field and its border).
fn centered(pct_x: u16, pct_y: u16, min_h: u16, area: Rect) -> Rect {
    let w = (area.width * pct_x / 100).max(10).min(area.width);
    let h = if pct_y == 0 {
        min_h
    } else {
        (area.height * pct_y / 100).max(min_h)
    }
    .min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests;

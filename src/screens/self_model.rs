//! "Self-model" screen (FSD "page"): viewing and **manually editing** the
//! agent's representation of itself for the active profile — the description, goals, the
//! interlocutor model, and the insight narrative. Opened from chat via `F3`, closed via `Esc`.
//!
//! Data arrives as a snapshot from the orchestrator (it owns `Storage`) via the
//! `AppEvent::SelfModelView` event. The screen hands edits off as the `SelfModelIntent::Edit`
//! intent (→ `AppCommand::UpdateSelfModel`); the orchestrator applies, saves, and re-emits the
//! updated snapshot (the screen updates in place). The screen itself doesn't know about
//! `app`/`Storage` (FSD). See docs/history/self-model-mvp.md.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use uuid::Uuid;

use crate::entities::profile::CharacterNames;
use crate::entities::self_model::{GoalStatus, SelfModel, SelfModelEdit, order_narrative};
use crate::shared::config::NoteOrder;
use crate::shared::i18n::Locale;
use crate::shared::keys;
use crate::shared::theme::Palette;
use crate::shared::ui::{dim_background, screen_chrome};
use crate::shared::wrap::wrap_line;
use crate::widgets::help_dialog::{HelpContext, HelpSection};
use crate::widgets::input_box::{InputBox, RenderOpts};

/// The self-model screen's "Shortcuts" section (`F1`): one row per key
/// `handle_key` matches — a new arm gets a row here (AGENTS.md §3). The app
/// layer composes the dialog's tab from the screens' sections
/// (docs/history/help-hotkeys-context.md §6).
pub(crate) static HELP_SECTION: HelpSection = HelpSection {
    title: "ui.help.sec.self_model",
    context: Some(HelpContext::SelfModel),
    rows: &[
        ("↑/↓", "ui.help.sm_select"),
        ("Enter", "ui.help.sm_edit"),
        ("Space", "ui.help.sm_goal"),
        ("Del", "ui.help.sm_delete"),
        ("Ctrl+K Ctrl+K", "ui.help.sm_clear"),
        ("Esc", "ui.help.sm_close"),
    ],
    openers: &[],
};

/// Intent from the "self-model" screen (translated by `app`). A counterpart to
/// [`ChatListIntent`](crate::screens::chat_list::ChatListIntent).
#[derive(Debug, Clone, PartialEq)]
pub enum SelfModelIntent {
    /// Close the view, return to the chat (`Esc`).
    Close,
    /// Quit the application (`Ctrl+Q`/`F10`).
    Quit,
    /// Apply a manual edit (the orchestrator will save and re-emit the snapshot).
    Edit(SelfModelEdit),
}

/// What the selected row points to (for edit actions).
#[derive(Debug, Clone, Copy, PartialEq)]
enum RowAction {
    Summary,
    Goal(Uuid),
    AddGoal,
    Traits,
    Interests,
    Relationship,
    Insight(Uuid),
    /// A decorative row (an empty separator or a section header) — the cursor
    /// can't land on it; navigation skips it.
    Decoration,
}

/// What exactly is being edited in the open text editor.
#[derive(Debug, Clone, Copy, PartialEq)]
enum EditKind {
    Summary,
    AddGoal,
    GoalText(Uuid),
    Traits,
    Interests,
    Relationship,
}

/// The active field text editor (a popup). Always multiline
/// (`Shift+Enter` — a line break, `Enter` — commit): models write long text.
struct Editor {
    kind: EditKind,
    input: InputBox,
}

/// The "self-model" screen: a model snapshot + navigation/edit state.
pub struct SelfModelScreen {
    model: Option<SelfModel>,
    /// The **profile's** role names (they ride the `SelfModelView` event): the
    /// two halves of the screen are headed by the assistant's and the user's
    /// name, an unset one falling back to the localized label (spec §17.7).
    /// Deliberately the profile's own, not the active chat's — a subagent
    /// transcript re-labels its sides for the run, while the self-model
    /// belongs to the profile.
    names: CharacterNames,
    palette: Palette,
    /// Interface locale (updated on `Settings`). See docs/i18n-ui.md.
    loc: &'static Locale,
    selected: usize,
    /// The first visible visual row (for scrolling a long list). Persists
    /// across frames; recomputed in [`Self::render`] so the selected row
    /// stays visible (and if it doesn't fit entirely, its top is visible).
    scroll: usize,
    editor: Option<Editor>,
    /// Confirmation for clearing the whole model (`Ctrl+K` twice).
    confirm_clear: bool,
    /// Which end of the narrative the observations are listed from
    /// (`interface.self_model_note_order`, spec §17.7). Taken at open time: the
    /// screen is a full-screen overlay, so settings cannot be edited while it
    /// stands, and every `F3` builds it anew.
    note_order: NoteOrder,
}

impl SelfModelScreen {
    /// Opens the screen with a model snapshot and the profile's role names.
    pub fn new(
        model: Option<SelfModel>,
        names: CharacterNames,
        palette: Palette,
        loc: &'static Locale,
        note_order: NoteOrder,
    ) -> Self {
        let mut screen = Self {
            model,
            names,
            palette,
            loc,
            selected: 0,
            scroll: 0,
            editor: None,
            confirm_clear: false,
            note_order,
        };
        // Row 0 is the "Assistant" header — a decoration. Land the cursor on
        // the first selectable row instead of highlighting a header on open.
        screen.move_selection(0);
        screen
    }

    /// Updates the model snapshot (after an edit/reflection — the `SelfModelView` event),
    /// keeping the selection where possible and closing the clear confirmation.
    pub fn set_model(&mut self, model: Option<SelfModel>, names: CharacterNames) {
        self.model = model;
        self.names = names;
        self.confirm_clear = false;
        let max = self.rows().len().saturating_sub(1);
        self.selected = self.selected.min(max);
        // The snapshot may have shifted/removed rows — move the cursor off a decoration if it landed there.
        self.move_selection(0);
    }

    /// Updates the theme palette (the `AppEvent::Settings` event).
    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    /// Updates the interface locale (the `AppEvent::Settings` event).
    pub fn set_loc(&mut self, loc: &'static Locale) {
        self.loc = loc;
    }

    /// Builds navigable rows (a view + an action target). The base fields are always
    /// present (editing is possible even on an empty model); goals and insights — if present.
    ///
    /// The screen reads as two halves, each under a header naming whose side it
    /// is — the assistant's (self-description + goals) and the user's
    /// (traits/interests/relationship) — followed by the observations. Every
    /// section, and every observation, is separated by a blank row: the values
    /// are free prose that wraps over several rows, so without the gaps two
    /// neighbouring fields read as one paragraph (spec §17.7).
    fn rows(&self) -> Vec<(Line<'static>, RowAction)> {
        let p = &self.palette;
        let empty = SelfModel::new(Uuid::nil());
        let m = self.model.as_ref().unwrap_or(&empty);
        let label = |s: &str| Span::styled(s.to_string(), p.accent_style());
        let dim = |s: String| Span::styled(s, p.muted_style());
        let mut rows: Vec<(Line<'static>, RowAction)> = Vec::new();
        // Decorative separator rows: a blank line and a section header
        // (bold accent). The cursor never lands on them — they only visually divide sections.
        let spacer = || (Line::from(String::new()), RowAction::Decoration);
        let header = |s: &str| {
            (
                Line::from(Span::styled(
                    s.to_string(),
                    p.accent_style().add_modifier(Modifier::BOLD),
                )),
                RowAction::Decoration,
            )
        };

        let loc = self.loc;
        // Whose half this is: the profile's name for the side, or the localized
        // label when it is unset — the same rule the feed's role headers follow
        // (spec §5.1).
        let named = |custom: Option<&str>, key: &str| {
            custom
                .map(str::to_string)
                .unwrap_or_else(|| loc.t(key).to_string())
        };

        // The assistant's half: who the model is, and what it is working toward.
        rows.push(header(&named(
            self.names.assistant_name(),
            "ui.self_model.assistant",
        )));
        rows.push(spacer());
        // The self-description.
        let summary = if m.summary.trim().is_empty() {
            dim("—".into())
        } else {
            Span::raw(m.summary.trim().to_string())
        };
        rows.push((
            Line::from(vec![label(loc.t("ui.self_model.summary")), summary]),
            RowAction::Summary,
        ));
        rows.push(spacer());

        // Goals (with a status marker) + an "add" row. The date is in the local zone:
        // creation for an active one, the closing moment (`closed_at`) for a closed one.
        for g in &m.goals {
            // Active — `●` (WGL4-safe, no replacement needed); closed — from the
            // palette's glyph set (compat: `√`/`×`).
            let glyphs = p.glyphs();
            let (marker, style) = match g.status {
                GoalStatus::Active => ("● ".to_string(), p.success_style()),
                GoalStatus::Completed => (format!("{} ", glyphs.ok), p.muted_style()),
                GoalStatus::Abandoned => (format!("{} ", glyphs.failed), p.muted_style()),
            };
            let date = g
                .closed_at
                .unwrap_or(g.created_at)
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string();
            rows.push((
                Line::from(vec![
                    Span::styled(marker.to_string(), style),
                    Span::raw(g.description.trim().to_string()),
                    dim(format!("  [{date}]")),
                ]),
                RowAction::Goal(g.id),
            ));
        }
        rows.push((
            Line::from(vec![Span::styled(
                format!("＋ {}", loc.t("ui.self_model.add_goal")),
                p.muted_style(),
            )]),
            RowAction::AddGoal,
        ));

        // The user's half — separated from the assistant's by a blank line and a header.
        rows.push(spacer());
        rows.push(header(&named(self.names.user_name(), "ui.self_model.user")));
        rows.push(spacer());
        let u = &m.user_model;
        let join_or_dash = |v: &[String]| {
            if v.is_empty() {
                dim("—".into())
            } else {
                Span::raw(v.join(", "))
            }
        };
        rows.push((
            Line::from(vec![
                label(loc.t("ui.self_model.traits")),
                join_or_dash(&u.perceived_traits),
            ]),
            RowAction::Traits,
        ));
        rows.push(spacer());
        rows.push((
            Line::from(vec![
                label(loc.t("ui.self_model.interests")),
                join_or_dash(&u.current_interests),
            ]),
            RowAction::Interests,
        ));
        rows.push(spacer());
        let rel = if u.relationship_dynamic.trim().is_empty() {
            dim("—".into())
        } else {
            Span::raw(u.relationship_dynamic.trim().to_string())
        };
        rows.push((
            Line::from(vec![label(loc.t("ui.self_model.relationship")), rel]),
            RowAction::Relationship,
        ));

        // The narrative — deletable via `Del`. Also behind a separator and
        // a header, so observations don't blend into the user's half. Which end
        // it starts from is the reader's setting (`interface.self_model_note_order`,
        // spec §17.7), newest first by default.
        if !m.narrative.is_empty() {
            rows.push(spacer());
            rows.push(header(&loc.tf(
                "ui.self_model.observations",
                &[("n", &m.narrative.len().to_string())],
            )));
            rows.push(spacer());
        }
        for (i, seg) in order_narrative(&m.narrative, self.note_order)
            .into_iter()
            .enumerate()
        {
            if i > 0 {
                rows.push(spacer());
            }
            // The date is in the local zone: `created_at` is stored in UTC, and without
            // conversion an observation added after local midnight would show
            // yesterday's date (in zones ahead of UTC "yesterday" is still going on).
            let date = seg
                .created_at
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string();
            rows.push((
                Line::from(vec![
                    dim(format!("[{date}] ")),
                    Span::raw(seg.text.trim().to_string()),
                ]),
                RowAction::Insight(seg.id),
            ));
        }
        rows
    }

    /// The selected row's action (or `None` if the index is out of range).
    fn selected_action(&self) -> Option<RowAction> {
        self.rows().get(self.selected).map(|(_, a)| *a)
    }

    /// Indices of rows the cursor can land on (everything except decorations).
    fn selectable_indices(&self) -> Vec<usize> {
        self.rows()
            .iter()
            .enumerate()
            .filter(|(_, (_, a))| !matches!(a, RowAction::Decoration))
            .map(|(i, _)| i)
            .collect()
    }

    /// Shifts the selection by `delta` positions among selectable rows (decorations
    /// are skipped). `isize::MIN`/`MAX` — to the start/end. If the current row is a
    /// decoration (after a model change), start from the nearest selectable one.
    fn move_selection(&mut self, delta: isize) {
        let sel = self.selectable_indices();
        if sel.is_empty() {
            return;
        }
        let cur = sel
            .iter()
            .position(|&i| i == self.selected)
            .unwrap_or_else(|| sel.iter().filter(|&&i| i < self.selected).count());
        let new = (cur as isize)
            .saturating_add(delta)
            .clamp(0, sel.len() as isize - 1) as usize;
        self.selected = sel[new];
    }

    /// Handles a keypress, returning an intent for `app`.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<SelfModelIntent> {
        if self.editor.is_some() {
            return self.handle_editor_key(key);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let phys = keys::hotkey_char(&key).unwrap_or('\0');
        // Clear confirmation: a repeated Ctrl+K confirms; any other key cancels.
        if self.confirm_clear {
            self.confirm_clear = false;
            if ctrl && phys == 'k' {
                return Some(SelfModelIntent::Edit(SelfModelEdit::Clear));
            }
            return None;
        }
        // Quit moved to Ctrl+Q/F10 (Ctrl+C is freed up). See docs/history/input-selection-undo-mouse.md §B.
        if ctrl && phys == 'q' {
            return Some(SelfModelIntent::Quit);
        }
        if ctrl && phys == 'k' {
            self.confirm_clear = true;
            return None;
        }
        match key.code {
            KeyCode::F(10) => return Some(SelfModelIntent::Quit), // a second way to quit
            KeyCode::Esc => return Some(SelfModelIntent::Close),
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::Home => self.move_selection(isize::MIN),
            KeyCode::End => self.move_selection(isize::MAX),
            KeyCode::Enter => return self.begin_edit(),
            KeyCode::Char(' ') => {
                if let Some(RowAction::Goal(id)) = self.selected_action() {
                    return Some(SelfModelIntent::Edit(SelfModelEdit::CycleGoalStatus(id)));
                }
            }
            KeyCode::Delete => match self.selected_action() {
                Some(RowAction::Goal(id)) => {
                    return Some(SelfModelIntent::Edit(SelfModelEdit::DeleteGoal(id)));
                }
                Some(RowAction::Insight(id)) => {
                    return Some(SelfModelIntent::Edit(SelfModelEdit::DeleteInsight(id)));
                }
                _ => {}
            },
            _ => {}
        }
        None
    }

    /// Opens the editor for the selected row (or does nothing — for an insight).
    fn begin_edit(&mut self) -> Option<SelfModelIntent> {
        let action = self.selected_action()?;
        let m = self
            .model
            .clone()
            .unwrap_or_else(|| SelfModel::new(Uuid::nil()));
        let (kind, seed) = match action {
            RowAction::Summary => (EditKind::Summary, m.summary.clone()),
            RowAction::AddGoal => (EditKind::AddGoal, String::new()),
            RowAction::Goal(id) => {
                let seed = m
                    .goals
                    .iter()
                    .find(|g| g.id == id)
                    .map(|g| g.description.clone())
                    .unwrap_or_default();
                (EditKind::GoalText(id), seed)
            }
            RowAction::Traits => (EditKind::Traits, m.user_model.perceived_traits.join(", ")),
            RowAction::Interests => (
                EditKind::Interests,
                m.user_model.current_interests.join(", "),
            ),
            RowAction::Relationship => (
                EditKind::Relationship,
                m.user_model.relationship_dynamic.clone(),
            ),
            RowAction::Insight(_) => return None, // insights aren't edited, only deleted
            RowAction::Decoration => return None, // a separator/header isn't editable
        };
        // The field is multiline (`InputBox` already defaults to that) — text wraps.
        let mut input = InputBox::new();
        input.set_text(&seed);
        self.editor = Some(Editor { kind, input });
        None
    }

    fn handle_editor_key(&mut self, key: KeyEvent) -> Option<SelfModelIntent> {
        let editor = self.editor.as_mut()?;
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                self.editor = None;
                None
            }
            // Shift+Enter (or Alt+Enter — a fallback line break for terminals with no
            // kitty protocol, see item 11) — a line break; Enter — commit.
            (KeyCode::Enter, m) if m.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => {
                editor.input.insert_newline();
                None
            }
            (KeyCode::Enter, _) => {
                let editor = self.editor.take().unwrap();
                commit_edit(editor.kind, editor.input.text())
            }
            // Ctrl+K (clear the field, undo — `Ctrl+Z`) and the field's other Ctrl
            // combinations are handled by `InputBox` itself in `on_key` (layout-independent).
            _ => {
                editor.input.on_key(key);
                None
            }
        }
    }

    /// Clipboard paste — into the active field editor (otherwise a no-op).
    pub fn handle_paste(&mut self, text: &str) {
        if let Some(editor) = self.editor.as_mut() {
            editor.input.insert_str(text);
        }
    }

    /// The screen's hotkey line, for the row the cursor stands on.
    ///
    /// Three of these keys act on the **selected row** and do nothing on the
    /// others, and the footer used to name all three unconditionally: on an
    /// observation it read "Enter edit" while `begin_edit` returns `None` there
    /// ("insights aren't edited, only deleted"), and on the summary it offered
    /// `Del` and `Space`, neither of which that row answers. So the list is
    /// built from [`Self::selected_action`] — the same value `handle_key`
    /// dispatches on, read in the same frame — and a key that would be a no-op
    /// is not advertised (spec §11.2, docs/history/status-hints-unified.md §2.2). The
    /// full list stays one `F1` away, which is what makes hiding safe.
    ///
    /// `Ctrl+K` and `Esc` are unconditional: they are about the whole model,
    /// not the selection. Pairs "key — description — is it dangerous"; the
    /// descriptions are already localized.
    fn hints(&self) -> Vec<(&'static str, &'static str, bool)> {
        let loc = self.loc;
        let action = self.selected_action();
        let mut hk: Vec<(&'static str, &'static str, bool)> =
            vec![("↑↓", loc.t("ui.self_model.hk.select"), false)];
        // `Enter` opens the editor for the six editable rows; on the "add a
        // goal" row it does not edit anything, so it is worded as what it does.
        match action {
            Some(RowAction::AddGoal) => hk.push(("Enter", loc.t("ui.self_model.hk.add"), false)),
            Some(
                RowAction::Summary
                | RowAction::Goal(_)
                | RowAction::Traits
                | RowAction::Interests
                | RowAction::Relationship,
            ) => hk.push(("Enter", loc.t("ui.self_model.hk.edit"), false)),
            _ => {}
        }
        if matches!(action, Some(RowAction::Goal(_))) {
            hk.push(("Space", loc.t("ui.self_model.hk.goal_status"), false));
        }
        if matches!(action, Some(RowAction::Goal(_) | RowAction::Insight(_))) {
            hk.push(("Del", loc.t("ui.self_model.hk.delete"), true));
        }
        hk.extend([
            ("Ctrl+K", loc.t("ui.self_model.hk.clear"), false),
            ("F1", loc.t("ui.self_model.hk.help"), false),
            ("Esc", loc.t("ui.self_model.hk.close"), false),
            ("Ctrl+Q", loc.t("ui.self_model.hk.quit"), false),
        ]);
        hk
    }

    /// Draws the screen full-screen: the field list + a hotkey line (below the
    /// panel, outside the border); the editor — as a popup.
    pub fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let palette = self.palette;
        let loc = self.loc;

        // The hotkey line — below the panel (outside the border), right-aligned
        // and wrapping to as many rows as it needs, like every other screen's
        // footer (`screen_chrome`). The clear confirmation takes the strip
        // instead, and it is one line: passing no hints leaves `screen_chrome`
        // its minimum height, so the question never trails a blank row and the
        // panel keeps the space the hidden hints would have cost.
        let hints = if self.confirm_clear {
            Vec::new()
        } else {
            self.hints()
        };
        let chrome = screen_chrome(
            frame,
            &palette,
            format!(
                "{} {}",
                palette.glyphs().assistant_icon,
                loc.t("ui.self_model.title")
            ),
            None,
            &hints,
        );
        let (inner, status_area, hotkeys) = (chrome.inner, chrome.status, chrome.hotkeys);

        // Word wrap: long values (models write a lot of text) don't fit
        // on one line. Every logical list line is wrapped into several
        // visual rows; the visual rows are then drawn manually (rather than via the
        // `List` widget). Reason: `List` entirely SKIPS a multiline item that doesn't
        // fit height-wise in the remaining area (its `get_items_bounds` cuts off at
        // `height + item.height() > max_height`), leaving an empty spot — a long trailing item
        // reads as "the end of the list". Manual rendering clips a trailing item at the
        // area's height, showing its top portion. Content width = the area
        // minus the selection-marker column `▌ ` (2 columns).
        let list_area = inner;
        let content_width = (list_area.width as usize).saturating_sub(2).max(1);
        let rows = self.rows();
        let sel = self.selected.min(rows.len().saturating_sub(1));

        let (visual, sel_start, sel_height) = expand_visual_rows(&rows, sel, content_width);

        let view_h = list_area.height as usize;
        self.scroll = adjust_scroll(self.scroll, sel_start, sel_height, view_h);

        self.draw_visual_rows(frame, list_area, &visual, sel);

        // The bottom area (below the panel, outside the border): the clear confirmation or the
        // hotkey grid (computed above).
        if self.confirm_clear {
            let warn = Style::new().fg(palette.warning);
            frame.render_widget(
                Paragraph::new(Line::styled(loc.t("ui.self_model.confirm_clear"), warn)),
                status_area,
            );
        } else {
            frame.render_widget(Paragraph::new(hotkeys), status_area);
        }

        self.render_editor_popup(frame, area);
    }

    /// Draws the visible visual rows. The selected row gets a backdrop across the whole
    /// row width (the base `Paragraph` colors the whole area) and a `▌` marker; others get an indent.
    fn draw_visual_rows(
        &self,
        frame: &mut Frame,
        list_area: Rect,
        visual: &[(usize, Line<'static>)],
        sel: usize,
    ) {
        let palette = self.palette;
        let view_h = list_area.height as usize;
        for (offset, (ri, line)) in visual.iter().enumerate().skip(self.scroll).take(view_h) {
            let y = list_area.y + (offset - self.scroll) as u16;
            let row = Rect {
                x: list_area.x,
                y,
                width: list_area.width,
                height: 1,
            };
            let selected = *ri == sel;
            let prefix = if selected {
                Span::raw("▌ ")
            } else {
                Span::raw("  ")
            };
            let mut spans = vec![prefix];
            spans.extend(line.spans.iter().cloned());
            let mut para = Paragraph::new(Line::from(spans));
            if selected {
                para = para.style(Style::new().bg(palette.keycap_bg));
            }
            frame.render_widget(para, row);
        }
    }

    /// The editor drawn on top — with a real cursor. Always multiline (text wraps).
    /// A no-op when no editor is open.
    fn render_editor_popup(&mut self, frame: &mut Frame, area: Rect) {
        let palette = self.palette;
        let loc = self.loc;
        if let Some(editor) = self.editor.as_mut() {
            let popup = centered_rect(80, 50, area);
            let title = loc.tf(
                "ui.editor.multiline_footer",
                &[("newline", crate::shared::keys::newline_chord())],
            );
            dim_background(frame, &palette);
            frame.render_widget(Clear, popup);
            editor
                .input
                .render(frame, popup, RenderOpts::focused(&title), &palette);
        }
    }
}

/// Expands each logical line into visual rows wrapped to `content_width`,
/// tracking for every row the index of its logical line; returns the rows
/// plus where the selected row starts and how many visual rows it takes.
fn expand_visual_rows(
    rows: &[(Line<'static>, RowAction)],
    sel: usize,
    content_width: usize,
) -> (Vec<(usize, Line<'static>)>, usize, usize) {
    let mut visual: Vec<(usize, Line<'static>)> = Vec::new();
    let mut sel_start = 0usize;
    let mut sel_height = 1usize;
    for (ri, (line, _)) in rows.iter().enumerate() {
        let start = visual.len();
        if ri == sel {
            sel_start = start;
        }
        let mut wrapped = wrap_line(line, content_width);
        if wrapped.is_empty() {
            wrapped.push(Line::from(String::new())); // an empty separator
        }
        if ri == sel {
            sel_height = wrapped.len();
        }
        for vl in wrapped {
            visual.push((ri, vl));
        }
    }
    (visual, sel_start, sel_height)
}

/// Builds an edit intent from the editor's commit (or `None` if the edit is empty).
fn commit_edit(kind: EditKind, text: String) -> Option<SelfModelIntent> {
    let edit = match kind {
        EditKind::Summary => SelfModelEdit::SetSummary(text),
        EditKind::AddGoal => {
            if text.trim().is_empty() {
                return None;
            }
            SelfModelEdit::AddGoal(text)
        }
        EditKind::GoalText(id) => SelfModelEdit::SetGoalText { id, text },
        EditKind::Traits => SelfModelEdit::SetTraits(parse_list(&text)),
        EditKind::Interests => SelfModelEdit::SetInterests(parse_list(&text)),
        EditKind::Relationship => SelfModelEdit::SetRelationship(text),
    };
    Some(SelfModelIntent::Edit(edit))
}

/// A list → a vector of non-empty trimmed items. Separators are a comma **and**
/// a line break (the field is multiline: items can be entered either comma-separated
/// or one per line).
fn parse_list(s: &str) -> Vec<String> {
    s.split([',', '\n'])
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

/// Recomputes scroll so the selected row (visual rows
/// `[sel_start, sel_start + sel_height)`) is visible in a window of height `view_h`:
/// - if the row is above the window — raise the window's top to its start;
/// - if its bottom is past the window — lower the window to its bottom;
/// - but if the row doesn't fit entirely (taller than the window) — pin it to its **top**
///   (the top of the long item is visible, not the bottom).
///
/// Otherwise scroll is unchanged.
fn adjust_scroll(scroll: usize, sel_start: usize, sel_height: usize, view_h: usize) -> usize {
    if view_h == 0 {
        return scroll;
    }
    let sel_end = sel_start + sel_height; // exclusive
    if sel_start < scroll {
        sel_start
    } else if sel_end > scroll + view_h {
        if sel_height >= view_h {
            sel_start
        } else {
            sel_end - view_h
        }
    } else {
        scroll
    }
}

/// A centered rectangle by width/height percentage.
fn centered_rect(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let w = area.width * pct_x / 100;
    let h = area.height * pct_y / 100;
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Adds an observation to the snapshot for tests. Observations moved into notes, and
    /// the `F3` snapshot carries them reconstructed in the `narrative` field (the orchestrator
    /// fills it from self-notes), so tests put them straight into the field.
    fn push_insight(m: &mut SelfModel, text: &str) {
        m.narrative
            .push(crate::entities::self_model::NarrativeSegment {
                id: Uuid::new_v4(),
                text: text.into(),
                created_at: chrono::Utc::now(),
            });
    }

    fn model() -> SelfModel {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.summary = "ценю ясность".into();
        m.add_goal("помочь с проектом");
        m.user_model.perceived_traits = vec!["скептичный".into()];
        push_insight(&mut m, "замечен интерес к Rust");
        m
    }

    /// The footer names only the keys the selected row answers.
    ///
    /// The report that started this: standing on an observation, the footer read
    /// "Enter edit" while `begin_edit` returns `None` there — insights are
    /// deleted, never edited. The walk below stands on **every** selectable row
    /// and checks the three row-dependent hints against `selected_action`, the
    /// same value `handle_key` dispatches on.
    #[test]
    fn the_footer_names_only_what_the_selected_row_answers() {
        let mut m = model();
        m.user_model.current_interests = vec!["Rust".into()];
        m.user_model.relationship_dynamic = "рабочая".into();
        let mut s = SelfModelScreen::new(
            Some(m),
            CharacterNames::default(),
            Palette::default(),
            ru(),
            NoteOrder::default(),
        );
        let loc = ru();
        let (edit, add, goal, del) = (
            loc.t("ui.self_model.hk.edit"),
            loc.t("ui.self_model.hk.add"),
            loc.t("ui.self_model.hk.goal_status"),
            loc.t("ui.self_model.hk.delete"),
        );
        let mut seen_insight = false;
        let mut seen_goal = false;
        let mut seen_add = false;
        for i in s.selectable_indices() {
            s.selected = i;
            let action = s.selected_action().expect("a selectable row");
            let hints = s.hints();
            let has = |desc: &str| hints.iter().any(|(_, d, _)| *d == desc);
            match action {
                RowAction::Insight(_) => {
                    seen_insight = true;
                    assert!(!has(edit), "an observation is not editable: {hints:?}");
                    assert!(!has(goal), "…and has no goal status: {hints:?}");
                    assert!(has(del), "…but Del deletes it: {hints:?}");
                }
                RowAction::Goal(_) => {
                    seen_goal = true;
                    assert!(has(edit) && has(goal) && has(del), "{hints:?}");
                }
                RowAction::AddGoal => {
                    seen_add = true;
                    assert!(has(add), "the row adds rather than edits: {hints:?}");
                    assert!(!has(edit) && !has(goal) && !has(del), "{hints:?}");
                }
                // Summary, traits, interests, relationship: editable, and neither
                // `Space` nor `Del` does anything on them.
                _ => {
                    assert!(has(edit), "{action:?} is editable: {hints:?}");
                    assert!(!has(goal) && !has(del), "{action:?}: {hints:?}");
                }
            }
            // The keys that are about the whole model, not the row.
            assert!(
                hints
                    .iter()
                    .any(|(k, _, _)| *k == "Ctrl+K" || *k == "Esc" || *k == "F1"),
                "{hints:?}"
            );
        }
        assert!(
            seen_insight && seen_goal && seen_add,
            "the walk must cover all three special rows"
        );
    }

    /// Every hint the footer can hide is still reachable: the keys it names on
    /// some row are all in the screen's `F1` section, which is what makes hiding
    /// safe (docs/history/status-hints-unified.md §2.3).
    #[test]
    fn every_hidden_hint_is_in_the_help_section() {
        let mut s = SelfModelScreen::new(
            Some(model()),
            CharacterNames::default(),
            Palette::default(),
            ru(),
            NoteOrder::default(),
        );
        for i in s.selectable_indices() {
            s.selected = i;
            for (key, _, _) in s.hints() {
                // `F1` and `Ctrl+Q` are global keys, listed in the dialog's
                // "Everywhere" section rather than this screen's.
                if key == "F1" || key == "Ctrl+Q" {
                    continue;
                }
                // The two surfaces spell a key pair differently by convention
                // — the footer packs it (`↑↓`), the dialog spaces it out
                // (`↑/↓`, `Ctrl+K Ctrl+K`) — so the comparison drops the
                // separators and matches on the prefix.
                let norm = |k: &str| k.replace(['/', ' '], "");
                assert!(
                    HELP_SECTION
                        .rows
                        .iter()
                        .any(|(k, _)| norm(k).starts_with(&norm(key))),
                    "{key} is advertised but not in the F1 section"
                );
            }
        }
    }

    #[test]
    fn esc_closes_ctrl_q_and_f10_quit() {
        let mut s = SelfModelScreen::new(
            None,
            CharacterNames::default(),
            Palette::default(),
            ru(),
            NoteOrder::default(),
        );
        assert_eq!(
            s.handle_key(key(KeyCode::Esc)),
            Some(SelfModelIntent::Close)
        );
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
            Some(SelfModelIntent::Quit)
        );
        assert_eq!(
            s.handle_key(key(KeyCode::F(10))),
            Some(SelfModelIntent::Quit)
        );
    }

    #[test]
    fn enter_on_summary_opens_editor_and_commits() {
        let mut s = SelfModelScreen::new(
            Some(model()),
            CharacterNames::default(),
            Palette::default(),
            ru(),
            NoteOrder::default(),
        );
        // The first row is the self-description.
        assert_eq!(s.handle_key(key(KeyCode::Enter)), None);
        assert!(s.editor.is_some());
        // Typing and committing → a summary-edit intent.
        s.handle_key(key(KeyCode::Char('!')));
        let intent = s.handle_key(key(KeyCode::Enter)).unwrap();
        match intent {
            SelfModelIntent::Edit(SelfModelEdit::SetSummary(t)) => assert!(t.contains('!')),
            other => panic!("expected SetSummary, got {other:?}"),
        }
        assert!(s.editor.is_none());
    }

    #[test]
    fn alt_enter_inserts_newline_in_editor() {
        // A fallback line break for terminals with no kitty protocol (there Shift+Enter is
        // indistinguishable from Enter). See item 11 of the InputBox audit.
        let mut s = SelfModelScreen::new(
            Some(model()),
            CharacterNames::default(),
            Palette::default(),
            ru(),
            NoteOrder::default(),
        );
        s.handle_key(key(KeyCode::Enter)); // open the self-description editor
        s.handle_key(key(KeyCode::Char('A')));
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        s.handle_key(key(KeyCode::Char('B')));
        assert!(s.editor.is_some(), "Alt+Enter must not close the editor");
        let intent = s.handle_key(key(KeyCode::Enter)).unwrap();
        match intent {
            SelfModelIntent::Edit(SelfModelEdit::SetSummary(t)) => assert!(t.contains("A\nB")),
            other => panic!("expected SetSummary, got {other:?}"),
        }
    }

    #[test]
    fn space_cycles_goal_delete_removes() {
        let mut s = SelfModelScreen::new(
            Some(model()),
            CharacterNames::default(),
            Palette::default(),
            ru(),
            NoteOrder::default(),
        );
        s.handle_key(key(KeyCode::Down)); // onto the goal
        assert!(matches!(s.selected_action(), Some(RowAction::Goal(_))));
        let cycle = s.handle_key(key(KeyCode::Char(' '))).unwrap();
        assert!(matches!(
            cycle,
            SelfModelIntent::Edit(SelfModelEdit::CycleGoalStatus(_))
        ));
        let del = s.handle_key(key(KeyCode::Delete)).unwrap();
        assert!(matches!(
            del,
            SelfModelIntent::Edit(SelfModelEdit::DeleteGoal(_))
        ));
    }

    #[test]
    fn add_goal_commits_and_empty_is_noop() {
        let mut s = SelfModelScreen::new(
            None,
            CharacterNames::default(),
            Palette::default(),
            ru(),
            NoteOrder::default(),
        );
        // Rows of the empty model: [·Assistant·, ·spacer·, Summary, ·spacer·, AddGoal,
        // ·spacer·, ·User·, ·spacer·, Traits, ·spacer·, Interests, ·spacer·,
        // Relationship] — the cursor skips decorations, so one `Down` off the
        // self-description lands on "add goal".
        s.handle_key(key(KeyCode::Down));
        assert!(matches!(s.selected_action(), Some(RowAction::AddGoal)));
        s.handle_key(key(KeyCode::Enter));
        assert!(s.editor.is_some());
        // An empty commit → a no-op.
        assert_eq!(s.handle_key(key(KeyCode::Enter)), None);

        s.handle_key(key(KeyCode::Enter)); // open it again
        s.handle_key(key(KeyCode::Char('ц')));
        let intent = s.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(matches!(
            intent,
            SelfModelIntent::Edit(SelfModelEdit::AddGoal(_))
        ));
    }

    #[test]
    fn ctrl_k_confirm_then_clear() {
        let mut s = SelfModelScreen::new(
            Some(model()),
            CharacterNames::default(),
            Palette::default(),
            ru(),
            NoteOrder::default(),
        );
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)),
            None
        );
        assert!(s.confirm_clear);
        // A repeated Ctrl+K confirms.
        let intent = s
            .handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL))
            .unwrap();
        assert_eq!(intent, SelfModelIntent::Edit(SelfModelEdit::Clear));
        assert!(!s.confirm_clear);
    }

    #[test]
    fn ctrl_k_confirm_cancelled_by_other_key() {
        let mut s = SelfModelScreen::new(
            Some(model()),
            CharacterNames::default(),
            Palette::default(),
            ru(),
            NoteOrder::default(),
        );
        s.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert!(s.confirm_clear);
        assert_eq!(s.handle_key(key(KeyCode::Down)), None); // cancel
        assert!(!s.confirm_clear);
    }

    #[test]
    fn traits_edit_parses_comma_list() {
        let intent = commit_edit(EditKind::Traits, "  a , b ,, c ".into()).unwrap();
        match intent {
            SelfModelIntent::Edit(SelfModelEdit::SetTraits(v)) => {
                assert_eq!(v, vec!["a".to_string(), "b".into(), "c".into()]);
            }
            other => panic!("expected SetTraits, got {other:?}"),
        }
    }

    #[test]
    fn list_edit_splits_on_newlines_too() {
        // A multiline field: items can be entered one per line.
        let intent = commit_edit(EditKind::Interests, "Rust\nратату\n, TUI".into()).unwrap();
        match intent {
            SelfModelIntent::Edit(SelfModelEdit::SetInterests(v)) => {
                assert_eq!(v, vec!["Rust".to_string(), "ратату".into(), "TUI".into()]);
            }
            other => panic!("expected SetInterests, got {other:?}"),
        }
    }

    /// The row texts, decorations included — what the reader actually sees.
    fn row_texts(s: &SelfModelScreen) -> Vec<String> {
        s.rows()
            .iter()
            .map(|(l, _)| l.spans.iter().map(|sp| sp.content.as_ref()).collect())
            .collect()
    }

    /// Adds an observation with an explicit date (the rows are ordered by it).
    fn push_dated(m: &mut SelfModel, text: &str, day: u32) {
        m.narrative
            .push(crate::entities::self_model::NarrativeSegment {
                id: Uuid::new_v4(),
                text: text.into(),
                created_at: chrono::TimeZone::with_ymd_and_hms(
                    &chrono::Utc,
                    2026,
                    3,
                    day,
                    12,
                    0,
                    0,
                )
                .unwrap(),
            });
    }

    /// A model whose observations arrive **newest first** — the order the
    /// orchestrator's snapshot actually has (`note_list` is `updated_at`-descending).
    fn model_with_dated_observations() -> SelfModel {
        let mut m = SelfModel::new(Uuid::new_v4());
        push_dated(&mut m, "самое новое", 9);
        push_dated(&mut m, "среднее", 5);
        push_dated(&mut m, "самое старое", 1);
        m
    }

    /// The observation rows, top to bottom (the header and the blank rows dropped).
    fn observation_texts(s: &SelfModelScreen) -> Vec<String> {
        s.rows()
            .iter()
            .filter(|(_, a)| matches!(a, RowAction::Insight(_)))
            .map(|(l, _)| l.spans.iter().map(|sp| sp.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn observations_are_listed_newest_first_by_default() {
        // The default is reverse-chronological: what the assistant noticed last sits
        // under the header, where a person opening `F3` looks first (spec §17.7).
        // The regression this pins: the screen used to `.rev()` whatever order the
        // snapshot arrived in, and the snapshot is *already* newest-first — so the
        // oldest observation was the one on top.
        let s = SelfModelScreen::new(
            Some(model_with_dated_observations()),
            CharacterNames::default(),
            Palette::default(),
            ru(),
            NoteOrder::default(),
        );
        let texts = observation_texts(&s);
        assert_eq!(texts.len(), 3, "{texts:?}");
        assert!(texts[0].ends_with("самое новое"), "{texts:?}");
        assert!(texts[2].ends_with("самое старое"), "{texts:?}");
        // The dates read downward too — the row's own date is the sort key.
        assert!(texts[0].starts_with("[2026-03-09] "), "{texts:?}");
        assert!(texts[2].starts_with("[2026-03-01] "), "{texts:?}");
    }

    #[test]
    fn oldest_first_setting_flips_the_observation_list() {
        let s = SelfModelScreen::new(
            Some(model_with_dated_observations()),
            CharacterNames::default(),
            Palette::default(),
            ru(),
            NoteOrder::OldestFirst,
        );
        let texts = observation_texts(&s);
        assert!(texts[0].ends_with("самое старое"), "{texts:?}");
        assert!(texts[2].ends_with("самое новое"), "{texts:?}");
    }

    #[test]
    fn del_removes_the_observation_the_cursor_stands_on_in_either_order() {
        // The order is a display rule: `Del` carries the row's own id, so the
        // reversed list deletes the row the user is looking at, not its mirror.
        for (order, expected) in [
            (NoteOrder::NewestFirst, "самое новое"),
            (NoteOrder::OldestFirst, "самое старое"),
        ] {
            let m = model_with_dated_observations();
            let wanted = m
                .narrative
                .iter()
                .find(|s| s.text == expected)
                .map(|s| s.id)
                .unwrap();
            let mut s = SelfModelScreen::new(
                Some(m),
                CharacterNames::default(),
                Palette::default(),
                ru(),
                order,
            );
            // Walk to the first observation row and delete it.
            let first = s
                .rows()
                .iter()
                .position(|(_, a)| matches!(a, RowAction::Insight(_)))
                .unwrap();
            s.selected = first;
            assert_eq!(
                s.handle_key(key(KeyCode::Delete)),
                Some(SelfModelIntent::Edit(SelfModelEdit::DeleteInsight(wanted))),
                "{order:?}"
            );
        }
    }

    #[test]
    fn sections_are_headed_and_separated_by_blank_rows() {
        // Every section is a header (or a field) with a blank row around it, so two
        // fields of wrapped prose never read as one paragraph (spec §17.7).
        let mut m = model();
        push_insight(&mut m, "второе наблюдение");
        let s = SelfModelScreen::new(
            Some(m),
            CharacterNames::default(),
            Palette::default(),
            ru(),
            NoteOrder::default(),
        );
        let texts = row_texts(&s);
        let at = |needle: &str| {
            texts
                .iter()
                .position(|t| t.starts_with(needle))
                .unwrap_or_else(|| panic!("no row starting with {needle:?}: {texts:?}"))
        };
        let blank_before = |i: usize| {
            assert!(
                i > 0 && texts[i - 1].is_empty(),
                "no blank row before {i}: {texts:?}"
            )
        };
        let blank_after = |i: usize| {
            assert!(
                texts.get(i + 1).is_some_and(String::is_empty),
                "no blank row after {i}: {texts:?}"
            )
        };
        // The assistant's half heads the screen, its header set off from "About me".
        assert_eq!(texts[0], "Ассистент");
        blank_after(0);
        blank_after(at("О себе: ")); // ...and off the goals below it
        // The user's half: a header with blank rows on both sides.
        let user = at("Пользователь");
        blank_before(user);
        blank_after(user);
        // Its three fields stand apart from each other.
        blank_after(at("Черты: "));
        blank_after(at("Интересы: "));
        // The observations: a blank row under the header, and one between the two.
        let obs = at("Наблюдения");
        blank_before(obs);
        blank_after(obs);
        assert!(
            texts[obs + 2..].iter().filter(|t| t.is_empty()).count() == 1,
            "exactly one blank row separates the two observations: {texts:?}"
        );
    }

    #[test]
    fn headers_take_the_profile_names_when_set() {
        // A named profile heads the halves with its own names; an unset one falls
        // back to the localized labels (checked above).
        let s = SelfModelScreen::new(
            Some(model()),
            CharacterNames {
                user: "Владимир".into(),
                assistant: "Гайя".into(),
                system: String::new(),
            },
            Palette::default(),
            ru(),
            NoteOrder::default(),
        );
        let texts = row_texts(&s);
        assert_eq!(texts[0], "Гайя");
        assert!(texts.contains(&"Владимир".to_string()), "{texts:?}");
        assert!(!texts.contains(&"Пользователь".to_string()), "{texts:?}");
    }

    #[test]
    fn selection_opens_below_the_first_header() {
        // Row 0 is now a decoration: opening must not highlight it (`new` moves the
        // cursor onto the first selectable row), and neither must a fresh snapshot.
        let mut s = SelfModelScreen::new(
            Some(model()),
            CharacterNames::default(),
            Palette::default(),
            ru(),
            NoteOrder::default(),
        );
        assert!(matches!(s.selected_action(), Some(RowAction::Summary)));
        s.set_model(None, CharacterNames::default());
        assert!(matches!(s.selected_action(), Some(RowAction::Summary)));
    }

    #[test]
    fn navigation_skips_decoration_rows() {
        // A model with a goal and an insight: between AddGoal and Traits — spacer+header,
        // between Relationship and the insight — another spacer+header. `Down`
        // navigation must skip decorations and never land on them.
        let mut s = SelfModelScreen::new(
            Some(model()),
            CharacterNames::default(),
            Palette::default(),
            ru(),
            NoteOrder::default(),
        );
        let mut seen = Vec::new();
        loop {
            seen.push(s.selected_action().unwrap());
            let before = s.selected;
            s.handle_key(key(KeyCode::Down));
            if s.selected == before {
                break; // reached the end
            }
        }
        assert!(
            !seen.iter().any(|a| matches!(a, RowAction::Decoration)),
            "the cursor must not land on decorations: {seen:?}"
        );
        // Passed through all selectable rows from top to bottom.
        assert!(matches!(seen.first(), Some(RowAction::Summary)));
        assert!(matches!(seen.last(), Some(RowAction::Insight(_))));
    }

    #[test]
    fn render_empty_and_populated_do_not_panic() {
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut empty = SelfModelScreen::new(
            None,
            CharacterNames::default(),
            Palette::default(),
            ru(),
            NoteOrder::default(),
        );
        term.draw(|f| empty.render(f)).unwrap();
        let mut full = SelfModelScreen::new(
            Some(model()),
            CharacterNames::default(),
            Palette::default(),
            ru(),
            NoteOrder::default(),
        );
        term.draw(|f| full.render(f)).unwrap();
    }

    #[test]
    fn hotkeys_render_below_panel_and_wrap_when_narrow() {
        // The hotkey line — below the panel (outside the border): its rows start with a space in
        // column 0 (the panel's left border is a border character, so its rows don't start
        // with a space). Count the trailing hotkey rows.
        let rows = |w: u16, h: u16| -> Vec<String> {
            let mut s = SelfModelScreen::new(
                Some(model()),
                CharacterNames::default(),
                Palette::default(),
                ru(),
                NoteOrder::default(),
            );
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| s.render(f)).unwrap();
            let buf = term.backend().buffer().clone();
            (0..buf.area.height)
                .map(|y| {
                    (0..buf.area.width)
                        .map(|x| buf[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect()
        };
        let status_rows = |lines: &[String]| -> usize {
            lines
                .iter()
                .rev()
                .take_while(|l| l.starts_with(' '))
                .count()
        };
        // Wide — one hotkey line below the panel; above it — the panel's bottom border.
        let wide = rows(120, 24);
        assert_eq!(status_rows(&wide), 1);
        assert!(wide.last().unwrap().contains("Enter"));
        assert!(
            !wide[wide.len() - 2].starts_with(' '),
            "above the hotkeys — the panel's bottom border: {:?}",
            wide[wide.len() - 2]
        );
        // Narrow — the hotkeys wrap as a grid onto several lines.
        let narrow = rows(30, 24);
        assert!(status_rows(&narrow) > 1);
    }

    #[test]
    fn confirm_clear_shows_prompt_in_status_area() {
        // The clear confirmation occupies the bottom area (outside the border) as one line.
        let mut s = SelfModelScreen::new(
            Some(model()),
            CharacterNames::default(),
            Palette::default(),
            ru(),
            NoteOrder::default(),
        );
        s.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert!(s.confirm_clear);
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
        let buf = term.backend().buffer().clone();
        let last: String = (0..buf.area.width)
            .map(|x| buf[(x, buf.area.height - 1)].symbol().to_string())
            .collect();
        assert!(last.contains("Очистить всю модель"), "{last:?}");
    }

    #[test]
    fn adjust_scroll_keeps_selection_visible_and_pins_top_of_tall_item() {
        // The selected row fits entirely in the window — scroll is unchanged.
        assert_eq!(adjust_scroll(0, 0, 1, 10), 0);
        // The row is above the window — raise the window's top to its start.
        assert_eq!(adjust_scroll(5, 2, 1, 10), 2);
        // The row's bottom is past the window (the row is below the window) — lower the window to its bottom.
        assert_eq!(adjust_scroll(0, 9, 1, 5), 5); // sel_end=10, 10-5=5
        // A long item that doesn't fit height-wise: pin it to its TOP.
        assert_eq!(adjust_scroll(0, 8, 20, 5), 8);
        // Zero window height — no change.
        assert_eq!(adjust_scroll(3, 0, 1, 0), 3);
    }

    #[test]
    fn render_long_trailing_item_in_short_area_does_not_panic() {
        // A long trailing insight in a small window: previously `List` would have skipped such
        // an item entirely; now its top portion is drawn. Verify there's no
        // panic when wrapping onto many rows in a tight height.
        let mut m = SelfModel::new(Uuid::new_v4());
        m.summary = "описание".into();
        push_insight(&mut m, &"очень длинное наблюдение ".repeat(40));
        let mut s = SelfModelScreen::new(
            Some(m),
            CharacterNames::default(),
            Palette::default(),
            ru(),
            NoteOrder::default(),
        );
        let mut term = Terminal::new(TestBackend::new(40, 8)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
        // Move all the way to the bottom (onto the long insight) and redraw — scroll should
        // show its top with no panic.
        s.handle_key(key(KeyCode::End));
        term.draw(|f| s.render(f)).unwrap();
    }
}

//! The settings screen's mouse (docs/research/mouse-always-captured.md §5.8): the
//! wheel and the clicks of the section menu and of the field pane, and of the
//! popups — the field search, the choice list, the model picker, the editor —
//! each of which takes the mouse alone while it is open, as it takes the keys.

use std::time::Instant;

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use super::{Focus, SettingsIntent, SettingsScreen};
use crate::shared::mouse::{SelectUnit, wheel_delta};

/// `Enter`, as a double click sends it (F12).
fn enter() -> KeyEvent {
    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
}

/// A left press — the one report that selects a row.
fn is_press(mouse: &MouseEvent) -> bool {
    mouse.kind == MouseEventKind::Down(MouseButton::Left)
}

impl SettingsScreen {
    /// A mouse event. Whatever is open on top takes it alone — the field
    /// search, the choice list, the model picker, the editor — and the panes
    /// otherwise: the wheel scrolls the one under the pointer, the selection
    /// staying unless an edge pushes it (F11); a click on a row selects it and
    /// focuses its pane, a double click is that pane's `Enter` (F12). A press
    /// in a field places its cursor, a drag selects; outside an open popup a
    /// press does nothing — `Esc` closes it, as ever.
    pub fn handle_mouse(&mut self, mouse: MouseEvent, unit: SelectUnit) -> Option<SettingsIntent> {
        if self.search.is_some() {
            return self.search_mouse(mouse, unit);
        }
        if self.choice.is_some() {
            return self.choice_mouse(mouse, unit);
        }
        if self.picker.is_some() {
            return self.picker_mouse(mouse, unit);
        }
        if let Some(editor) = self.editor.as_mut() {
            self.field_gesture.on_mouse(&mut editor.input, mouse, unit);
            return None;
        }
        self.panes_mouse(mouse, unit)
    }

    /// The selected section's index, for tests outside the screen.
    #[cfg(test)]
    pub(crate) fn section_index(&self) -> usize {
        self.section_idx
    }

    /// The screen cell the menu drew section `section` on, for tests outside
    /// the screen that click it.
    #[cfg(test)]
    pub(crate) fn menu_cell(&self, section: usize) -> Option<(u16, u16)> {
        self.menu_scroll.cell_of(section)
    }

    /// The loop's tick, for a drag held past the editor's top or bottom edge
    /// (a multiline value scrolls under it). `true` when a frame is due.
    pub fn mouse_tick(&mut self, now: Instant) -> bool {
        let input = if let Some(st) = self.search.as_mut() {
            &mut st.input
        } else if let Some(st) = self.picker.as_mut() {
            &mut st.input
        } else if let Some(editor) = self.editor.as_mut() {
            &mut editor.input
        } else {
            return false;
        };
        self.field_gesture.tick(input, now)
    }

    fn search_mouse(&mut self, mouse: MouseEvent, unit: SelectUnit) -> Option<SettingsIntent> {
        let st = self.search.as_mut()?;
        if self.field_gesture.on_mouse(&mut st.input, mouse, unit) {
            return None;
        }
        let picked = st.scroll.on_mouse(mouse, &mut st.selected);
        (picked && unit == SelectUnit::Word)
            .then(|| self.handle_search_key(enter()))
            .flatten()
    }

    fn choice_mouse(&mut self, mouse: MouseEvent, unit: SelectUnit) -> Option<SettingsIntent> {
        let st = self.choice.as_mut()?;
        let picked = st.scroll.on_mouse(mouse, &mut st.selected);
        (picked && unit == SelectUnit::Word)
            .then(|| self.handle_choice_key(enter()))
            .flatten()
    }

    fn picker_mouse(&mut self, mouse: MouseEvent, unit: SelectUnit) -> Option<SettingsIntent> {
        let st = self.picker.as_mut()?;
        if self.field_gesture.on_mouse(&mut st.input, mouse, unit) {
            return None;
        }
        let picked = st.scroll.on_mouse(mouse, &mut st.selected);
        (picked && unit == SelectUnit::Word)
            .then(|| self.handle_picker_key(enter()))
            .flatten()
    }

    /// The section menu and the field pane.
    fn panes_mouse(&mut self, mouse: MouseEvent, unit: SelectUnit) -> Option<SettingsIntent> {
        let (col, row) = (mouse.column, mouse.row);
        if let Some(delta) = wheel_delta(mouse.kind) {
            if self.menu_scroll.holds(col, row) {
                let section = self.menu_scroll.wheel(delta, self.section_idx, 0);
                self.select_section(section);
            } else if self.fields_scroll.holds(col, row) {
                self.wheel_fields(delta);
            }
            return None;
        }
        if !is_press(&mouse) {
            return None;
        }
        if let Some(section) = self.menu_scroll.item_at(col, row) {
            self.select_section(section);
            self.focus = Focus::Menu;
            return (unit == SelectUnit::Word)
                .then(|| self.handle_menu_key(enter()))
                .flatten();
        }
        // A group header is no field, and takes no click.
        let field = self
            .fields_scroll
            .item_at(col, row)
            .and_then(|i| self.fields_rows.get(i).copied().flatten())?;
        self.focus = Focus::Fields;
        self.field_idx = field;
        (unit == SelectUnit::Word)
            .then(|| self.handle_fields_key(enter()))
            .flatten()
    }

    /// Selects a section as `↑`/`↓` on the menu do: a new section starts at its
    /// first field.
    fn select_section(&mut self, section: usize) {
        if section != self.section_idx {
            self.section_idx = section;
            self.field_idx = 0;
        }
    }

    /// The wheel over the field pane. The pane keeps a row of context around
    /// its selection (`scroll_padding(1)`), and a group header is no field: a
    /// selection the edge pushes lands on the nearest field the window still
    /// holds, on the side the window moved toward. With the focus on the menu,
    /// or on the subsection tabs, the pane has no selection to push: the view
    /// alone moves.
    fn wheel_fields(&mut self, delta: isize) {
        let at = self
            .fields_rows
            .iter()
            .position(|r| *r == Some(self.field_idx))
            .filter(|_| self.focus == Focus::Fields);
        let row = self.fields_scroll.wheel(delta, at.unwrap_or(0), 1);
        if at.is_none() {
            return;
        }
        let rows = &self.fields_rows;
        let toward = |r: usize| rows.get(r).copied().flatten();
        let ahead: Box<dyn Iterator<Item = usize>> = if delta > 0 {
            Box::new(row..rows.len())
        } else {
            Box::new((0..=row).rev())
        };
        if let Some(field) = ahead.filter_map(toward).next() {
            self.field_idx = field;
        }
    }
}

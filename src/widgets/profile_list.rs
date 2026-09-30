//! Profile-picker overlay when creating a chat (`Ctrl+N`). See spec §10, §11.2.
//!
//! The widget is self-contained: it holds a snapshot of the profile list and the
//! selection state, and answers key presses with [`ProfileListAction`] (executed
//! by `app`). Full profile management (CRUD) is at M8; here it's just picking.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem};
use uuid::Uuid;

use crate::entities::profile::ProfileSummary;
use crate::shared::theme::Palette;
use crate::shared::ui::{ListScroll, MinSize, legend_width, mark_selected};

/// Action the overlay asks the layer above to perform.
#[derive(Debug, Clone, PartialEq)]
pub enum ProfileListAction {
    /// The key press was handled inside the overlay.
    None,
    /// Close the overlay without a pick.
    Cancel,
    /// Create a chat from the picked profile.
    Pick(Uuid),
}

/// State of the profile-picker overlay.
pub struct ProfileListState {
    profiles: Vec<ProfileSummary>,
    selected: usize,
    /// The popup is capped at the screen's height, so a long profile list does
    /// scroll — and a scroll position has to survive the frame ([`ListScroll`]).
    scroll: ListScroll,
}

impl ProfileListState {
    /// Opens the overlay with a profile snapshot (selection starts on the first).
    pub fn new(profiles: Vec<ProfileSummary>) -> Self {
        Self {
            profiles,
            selected: 0,
            scroll: ListScroll::default(),
        }
    }

    /// Id of the selected profile (if the list isn't empty).
    pub fn selected_id(&self) -> Option<Uuid> {
        self.profiles.get(self.selected).map(|p| p.id)
    }

    /// Handles a key press, returning the action to execute.
    pub fn on_key(&mut self, key: KeyEvent) -> ProfileListAction {
        if key.kind != KeyEventKind::Press {
            return ProfileListAction::None;
        }
        match key.code {
            KeyCode::Esc => ProfileListAction::Cancel,
            KeyCode::Enter => match self.selected_id() {
                Some(id) => ProfileListAction::Pick(id),
                None => ProfileListAction::Cancel,
            },
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                ProfileListAction::None
            }
            KeyCode::Down => {
                if !self.profiles.is_empty() {
                    self.selected = (self.selected + 1).min(self.profiles.len() - 1);
                }
                ProfileListAction::None
            }
            _ => ProfileListAction::None,
        }
    }

    /// The smallest window the overlay is drawn in (spec §11.1.1): its key
    /// legend whole on the border — the overlay is never narrower than that —
    /// around three rows of the list.
    pub fn min_size(loc: &'static crate::shared::i18n::Locale) -> MinSize {
        MinSize::new(legend_width(loc.t("ui.profile_list.footer")), 5)
    }

    /// Draws the overlay centered in `area`.
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        palette: &Palette,
        loc: &'static crate::shared::i18n::Locale,
    ) {
        let rows = (self.profiles.len() as u16 + 2).clamp(5, area.height);
        let popup = centered_rect(50, 30.max(Self::min_size(loc).width), rows, area);
        frame.render_widget(Clear, popup);

        let block = palette
            .panel(
                format!(
                    "{} {}",
                    palette.glyphs().assistant_icon,
                    loc.t("ui.profile_list.title")
                ),
                true,
            )
            .title_bottom(Line::from(Span::styled(
                loc.t("ui.profile_list.footer"),
                palette.muted_style(),
            )));
        let items: Vec<ListItem> = self
            .profiles
            .iter()
            .map(|p| {
                ListItem::new(Line::from(Span::styled(
                    p.name.clone(),
                    Style::new().fg(palette.text),
                )))
            })
            .collect();
        // Reverse video is all that says which profile is selected — so the
        // monochrome mode, which has none, gets a marker (spec §11.6).
        let list = mark_selected(
            List::new(items)
                .block(block)
                .highlight_style(Style::new().reversed()),
            palette,
        );
        self.scroll.render(
            frame,
            list,
            popup,
            self.profiles.len(),
            popup.height.saturating_sub(2) as usize, // the panel's borders
            (!self.profiles.is_empty()).then_some(self.selected),
        );
    }
}

/// A rectangle centered in `area`: `pct_x` percent width (≥`min_w`), fixed height.
fn centered_rect(pct_x: u16, min_w: u16, height: u16, area: Rect) -> Rect {
    let w = area.width.saturating_mul(pct_x) / 100;
    let [h_area] = Layout::horizontal([Constraint::Length(w.max(min_w).min(area.width))])
        .flex(Flex::Center)
        .areas(area);
    let [v_area] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(h_area);
    v_area
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn ru() -> &'static crate::shared::i18n::Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn profile(name: &str) -> ProfileSummary {
        ProfileSummary {
            id: Uuid::new_v4(),
            name: name.to_string(),
        }
    }

    #[test]
    fn enter_picks_selected_profile() {
        let profiles = vec![profile("A"), profile("B")];
        let b_id = profiles[1].id;
        let mut s = ProfileListState::new(profiles);
        assert_eq!(s.on_key(key(KeyCode::Down)), ProfileListAction::None);
        assert_eq!(s.on_key(key(KeyCode::Enter)), ProfileListAction::Pick(b_id));
    }

    #[test]
    fn esc_cancels() {
        let mut s = ProfileListState::new(vec![profile("A")]);
        assert_eq!(s.on_key(key(KeyCode::Esc)), ProfileListAction::Cancel);
    }

    #[test]
    fn navigation_clamps_at_bounds() {
        let mut s = ProfileListState::new(vec![profile("A"), profile("B")]);
        s.on_key(key(KeyCode::Up)); // already at the top — stays put
        assert_eq!(s.selected_id(), Some(s.profiles[0].id));
        s.on_key(key(KeyCode::Down));
        s.on_key(key(KeyCode::Down)); // can't go past the last
        assert_eq!(s.selected_id(), Some(s.profiles[1].id));
    }

    fn drawn(s: &mut ProfileListState, palette: &Palette) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut term = Terminal::new(TestBackend::new(60, 12)).unwrap();
        term.draw(|f| s.render(f, f.area(), palette, ru())).unwrap();
        crate::shared::ui::tests::buffer_rows(term.backend().buffer())
    }

    /// Reverse video is all that says which profile is selected, and the
    /// monochrome mode has none: the selected row gets a marker, and the
    /// marker follows the selection (spec §11.6).
    #[test]
    fn the_selected_profile_is_marked_where_nothing_else_says_it() {
        let mut s = ProfileListState::new(vec![profile("Alpha"), profile("Beta")]);
        let row_of = |rows: &[String], name: &str| -> String {
            rows.iter()
                .find(|r| r.contains(name))
                .unwrap_or_else(|| panic!("{name} is not drawn"))
                .clone()
        };

        let mono = Palette::mono();
        let rows = drawn(&mut s, &mono);
        assert!(row_of(&rows, "Alpha").contains("│› Alpha"), "{rows:#?}");
        assert!(row_of(&rows, "Beta").contains("│  Beta"), "{rows:#?}");

        s.on_key(key(KeyCode::Down));
        let rows = drawn(&mut s, &mono);
        assert!(row_of(&rows, "Alpha").contains("│  Alpha"), "{rows:#?}");
        assert!(row_of(&rows, "Beta").contains("│› Beta"), "{rows:#?}");

        // Every other mode draws the rows it always drew.
        let rows = drawn(&mut s, &Palette::default());
        assert!(row_of(&rows, "Alpha").contains("│Alpha"), "{rows:#?}");
        assert!(row_of(&rows, "Beta").contains("│Beta"), "{rows:#?}");
    }

    /// The overlay is never narrower than its key legend, in either
    /// language: its width was half the window with a floor of 30, so in
    /// every window under 82 columns the legend — 41 — was cut at the corner.
    #[test]
    fn the_overlay_is_never_narrower_than_its_legend() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            let legend = loc.t("ui.profile_list.footer").trim();
            let need = ProfileListState::min_size(loc);
            for width in need.width..=120 {
                let mut state = ProfileListState::new(vec![ProfileSummary {
                    id: Uuid::new_v4(),
                    name: "Assistant".into(),
                }]);
                let mut term = Terminal::new(TestBackend::new(width, 12)).unwrap();
                term.draw(|f| state.render(f, f.area(), &Palette::default(), loc))
                    .unwrap();
                let rows = crate::shared::ui::tests::buffer_rows(term.backend().buffer());
                assert!(
                    rows.iter().any(|r| r.contains(legend)),
                    "{lang:?} {width}: {rows:#?}"
                );
            }
        }
    }

    #[test]
    fn render_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut s = ProfileListState::new(vec![profile("Альфа"), profile("Бета")]);
        for (w, h) in [(80u16, 24u16), (20, 6)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| s.render(f, f.area(), &Palette::default(), ru()))
                .unwrap();
        }
    }
}

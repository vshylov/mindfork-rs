//! The *Local servers* list on the chat screen: a local Ollama or LM Studio
//! that answered, one row per server and chat model
//! (docs/research/local-servers.md §4, stage 2). Opened at the start of a chat
//! with no engine configured, or by `/local`; a pick is written into the
//! settings by the orchestrator, `Esc` changes nothing.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem};

use crate::shared::api::local_servers::LocalOffer;
use crate::shared::i18n::Locale;
use crate::shared::theme::Palette;
use crate::shared::ui::{ListScroll, MinSize, legend_width, mark_selected};
use crate::shared::wrap;

/// Action the list asks the layer above to perform.
#[derive(Debug, Clone, PartialEq)]
pub enum LocalServerAction {
    /// The key press was handled inside the list.
    None,
    /// Close the list; nothing changes.
    Cancel,
    /// Use this server and model.
    Pick(LocalOffer),
}

/// State of the *Local servers* list.
pub struct LocalServerPickerState {
    offers: Vec<LocalOffer>,
    selected: usize,
    scroll: ListScroll,
}

impl LocalServerPickerState {
    /// Opens the list over what answered, the first row selected — a loaded
    /// model, when there is one (`local_servers::offers` puts it first).
    pub fn new(offers: Vec<LocalOffer>) -> Self {
        Self {
            offers,
            selected: 0,
            scroll: ListScroll::default(),
        }
    }

    /// Handles a key press, returning the action to execute.
    pub fn on_key(&mut self, key: KeyEvent) -> LocalServerAction {
        if key.kind != KeyEventKind::Press {
            return LocalServerAction::None;
        }
        match key.code {
            KeyCode::Esc => LocalServerAction::Cancel,
            KeyCode::Enter => match self.offers.get(self.selected) {
                Some(offer) => LocalServerAction::Pick(offer.clone()),
                None => LocalServerAction::Cancel,
            },
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                LocalServerAction::None
            }
            KeyCode::Down => {
                if !self.offers.is_empty() {
                    self.selected = (self.selected + 1).min(self.offers.len() - 1);
                }
                LocalServerAction::None
            }
            _ => LocalServerAction::None,
        }
    }

    /// The smallest window the list is drawn in (spec §11.1.1): its key legend
    /// whole on the border — the list is never narrower than that — around
    /// three rows.
    pub fn min_size(loc: &'static Locale) -> MinSize {
        MinSize::new(legend_width(loc.t("ui.local_servers.footer")), 5)
    }

    /// One row's text: the server and the model, then what else a pick
    /// brings — whether the model is loaded, and the embedder it configures.
    fn row_text(offer: &LocalOffer, loc: &'static Locale) -> (String, String) {
        let server = offer.server.product().unwrap_or(&offer.url);
        let head = format!("{server} · {}", offer.model);
        let mut tail = String::new();
        if offer.loaded {
            tail.push_str(&format!("  {}", loc.t("ui.local_servers.loaded")));
        }
        if let Some(embedder) = &offer.embedder {
            tail.push_str(&format!(
                "  {}",
                loc.tf("ui.local_servers.embeddings", &[("model", embedder)])
            ));
        }
        (head, tail)
    }

    /// Draws the list centered in `area`.
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        palette: &Palette,
        loc: &'static Locale,
    ) {
        // `max` then `min`, not `clamp`: a window under five rows (the gate
        // keeps the list out of one) must not panic either.
        let rows = (self.offers.len() as u16 + 2).max(5).min(area.height);
        let popup = centered_rect(70, 40.max(Self::min_size(loc).width), rows, area);
        frame.render_widget(Clear, popup);

        let block = palette
            .panel(
                format!(
                    "{}{}",
                    palette.glyphs().settings_icon,
                    loc.t("ui.local_servers.title")
                ),
                true,
            )
            .title_bottom(Line::from(Span::styled(
                loc.t("ui.local_servers.footer"),
                palette.muted_style(),
            )));
        let mark = palette.selected_mark().map_or(0, wrap::str_width);
        let width = (popup.width as usize).saturating_sub(2 + mark);
        let items: Vec<ListItem> = self
            .offers
            .iter()
            .map(|offer| {
                // The server and the model are what a pick writes, so they keep
                // their columns; what else it brings takes what is left, cut
                // with the "…" that says so.
                let (head, tail) = Self::row_text(offer, loc);
                let (head, _) = wrap::truncate_to_width(&head, width);
                let (tail, _) =
                    wrap::truncate_to_width(&tail, width.saturating_sub(wrap::str_width(&head)));
                ListItem::new(Line::from(vec![
                    Span::styled(head, Style::new().fg(palette.text)),
                    Span::styled(tail, palette.muted_style()),
                ]))
            })
            .collect();
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
            self.offers.len(),
            popup.height.saturating_sub(2) as usize, // the panel's borders
            (!self.offers.is_empty()).then_some(self.selected),
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
    use crate::shared::api::ServerKind;
    use ratatui::crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    pub(crate) fn offers() -> Vec<LocalOffer> {
        vec![
            LocalOffer {
                server: ServerKind::LmStudio,
                url: "http://127.0.0.1:1234/v1".into(),
                model: "google_gemma-4-e4b-it".into(),
                loaded: true,
                embedder: Some("text-embedding-nomic-embed-text-v1.5".into()),
            },
            LocalOffer {
                server: ServerKind::Ollama,
                url: "http://127.0.0.1:11434/v1".into(),
                model: "gemma4:e4b".into(),
                loaded: false,
                embedder: None,
            },
        ]
    }

    #[test]
    fn enter_picks_the_selected_row_and_esc_changes_nothing() {
        let mut s = LocalServerPickerState::new(offers());
        assert_eq!(s.on_key(key(KeyCode::Down)), LocalServerAction::None);
        assert_eq!(
            s.on_key(key(KeyCode::Enter)),
            LocalServerAction::Pick(offers()[1].clone())
        );
        assert_eq!(s.on_key(key(KeyCode::Esc)), LocalServerAction::Cancel);
        // Clamped at both ends.
        s.on_key(key(KeyCode::Down));
        assert_eq!(
            s.on_key(key(KeyCode::Enter)),
            LocalServerAction::Pick(offers()[1].clone())
        );
        for _ in 0..3 {
            s.on_key(key(KeyCode::Up));
        }
        assert_eq!(
            s.on_key(key(KeyCode::Enter)),
            LocalServerAction::Pick(offers()[0].clone())
        );
    }

    fn drawn(s: &mut LocalServerPickerState, width: u16, loc: &'static Locale) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut term = Terminal::new(TestBackend::new(width, 12)).unwrap();
        term.draw(|f| s.render(f, f.area(), &Palette::default(), loc))
            .unwrap();
        crate::shared::ui::tests::buffer_rows(term.backend().buffer())
    }

    /// A row says what a pick writes — the server and the model — and what
    /// else it brings: that the model is loaded, and the embedder.
    #[test]
    fn a_row_names_the_server_the_model_and_the_embedder() {
        let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::En);
        // Wide enough that nothing in the row is cut.
        let rows = drawn(&mut LocalServerPickerState::new(offers()), 160, loc);
        let lm = rows
            .iter()
            .find(|r| r.contains("LM Studio · google_gemma-4-e4b-it"))
            .unwrap_or_else(|| panic!("{rows:#?}"));
        assert!(lm.contains("nomic-embed-text-v1.5"), "{lm}");
        let ollama = rows
            .iter()
            .find(|r| r.contains("Ollama · gemma4:e4b"))
            .unwrap_or_else(|| panic!("{rows:#?}"));
        assert!(!ollama.contains("embed"), "{ollama}");
    }

    /// The list is never narrower than its key legend, in either language, and
    /// a narrow window cuts what a row brings before the server and the model.
    #[test]
    fn the_list_is_never_narrower_than_its_legend() {
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            let legend = loc.t("ui.local_servers.footer").trim();
            let need = LocalServerPickerState::min_size(loc);
            for width in need.width..=120 {
                let rows = drawn(&mut LocalServerPickerState::new(offers()), width, loc);
                assert!(
                    rows.iter().any(|r| r.contains(legend)),
                    "{lang:?} {width}: {rows:#?}"
                );
            }
        }
    }

    #[test]
    fn render_does_not_panic() {
        let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
        for (w, h) in [(80u16, 24u16), (20, 6), (1, 1)] {
            use ratatui::Terminal;
            use ratatui::backend::TestBackend;
            let mut s = LocalServerPickerState::new(offers());
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| s.render(f, f.area(), &Palette::default(), loc))
                .unwrap();
        }
    }
}

//! Streaming preview of the impersonated reply (`Ctrl+U`, spec §11.8).
//!
//! While a message is being written "on the user's behalf", the input box is
//! hidden and this non-editable widget is shown in its place: the generated
//! text streams into it. On completion (if not cancelled) the text is inserted
//! into the input box. Visually the widget looks like an input box (a border +
//! text), but with no cursor.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::shared::i18n::Locale;
use crate::shared::theme::Palette;
use crate::shared::wrap::wrap_ranges;

/// What the preview shows: `text` — the accumulated reply text, `spinner` —
/// the spinner glyph of this frame (the caller's `shared::ui::Spinner` picks
/// it from the palette's set), `done` — generation finished (the spinner
/// stops, the hint changes). `bare` — no border and no hint: the text behind
/// a two-column prompt that carries the spinner, the shape of the bare input
/// box the preview stands in for (spec §11.1.1).
pub struct Preview<'a> {
    pub text: &'a str,
    pub spinner: char,
    pub done: bool,
    pub bare: bool,
}

impl Preview<'_> {
    /// The columns of an `area_width`-wide preview that hold text: past the
    /// border, or past the bare prompt column — two columns either way. The
    /// layer above measures the preview's height with it, [`render`] wraps
    /// with it.
    pub fn text_width(area_width: u16) -> usize {
        usize::from(area_width.saturating_sub(2))
    }
}

/// Draws the impersonation preview in `area`.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    preview: &Preview,
    palette: &Palette,
    loc: &'static Locale,
) {
    // We word-wrap the text ourselves (as in `message_feed`/`input_box`) so we know
    // the exact number of visual rows and can scroll the feed to the tail — otherwise
    // a long reply only shows its start, and the growing tail runs off the bottom edge.
    let inner_w = Preview::text_width(area.width);
    let mut rows: Vec<Line<'static>> = Vec::new();
    for logical in preview.text.split('\n') {
        let chars: Vec<char> = logical.chars().collect();
        for (s, e) in wrap_ranges(&chars, inner_w) {
            rows.push(Line::from(chars[s..e].iter().collect::<String>()));
        }
    }

    if preview.bare {
        // The prompt column says what the row is doing: the spinner while
        // the reply streams, nothing once it is done.
        if !preview.done && area.width > 0 && area.height > 0 {
            let prompt = Rect {
                width: 1,
                height: 1,
                ..area
            };
            frame.render_widget(
                Paragraph::new(Span::styled(
                    preview.spinner.to_string(),
                    palette.accent_style(),
                )),
                prompt,
            );
        }
        let inner = Rect {
            x: area.x + area.width.min(2),
            width: area.width.saturating_sub(2),
            ..area
        };
        let scroll = (rows.len().saturating_sub(usize::from(inner.height))) as u16;
        frame.render_widget(Paragraph::new(Text::from(rows)).scroll((scroll, 0)), inner);
        return;
    }

    let hint = if preview.done {
        loc.t("ui.impersonation.done").to_string()
    } else {
        loc.tf(
            "ui.impersonation.active",
            &[("spinner", &preview.spinner.to_string())],
        )
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(palette.glyphs().border)
        .border_style(palette.accent_style())
        .title(Line::from(hint).style(palette.accent_style()));
    // Scroll to the tail: show the last `inner_h` rows.
    let inner_h = area.height.saturating_sub(2) as usize;
    let scroll = (rows.len().saturating_sub(inner_h)) as u16;
    let para = Paragraph::new(Text::from(rows))
        .block(block)
        .scroll((scroll, 0));
    frame.render_widget(para, area);
}

//! Streaming preview of the impersonated reply (`Ctrl+U`, spec §11.8).
//!
//! While a message is being written "on the user's behalf", the input box is
//! hidden and this non-editable widget is shown in its place: the generated
//! text streams into it. On completion (if not cancelled) the text is inserted
//! into the input box. Visually the widget looks like an input box (a border +
//! text), but with no cursor.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::shared::i18n::Locale;
use crate::shared::theme::Palette;
use crate::shared::wrap::wrap_ranges;

/// Draws the impersonation preview in `area`. `text` — the accumulated reply
/// text, `tick` — a frame counter for the spinner animation, `done` — generation
/// finished (the spinner stops, the hint changes). Spinner frames come from the
/// palette's glyph set (Braille; ASCII in compatibility mode).
pub fn render(
    frame: &mut Frame,
    area: Rect,
    text: &str,
    tick: usize,
    done: bool,
    palette: &Palette,
    loc: &'static Locale,
) {
    let hint = if done {
        loc.t("ui.impersonation.done").to_string()
    } else {
        let frames = palette.glyphs().spinner;
        let spinner = frames[(tick / 2) % frames.len()];
        loc.tf(
            "ui.impersonation.active",
            &[("spinner", &spinner.to_string())],
        )
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(palette.glyphs().border)
        .border_style(palette.accent_style())
        .title(Line::from(hint).style(palette.accent_style()));

    // We word-wrap the text ourselves (as in `message_feed`/`input_box`) so we know
    // the exact number of visual rows and can scroll the feed to the tail — otherwise
    // a long reply only shows its start, and the growing tail runs off the bottom edge.
    let inner_w = area.width.saturating_sub(2) as usize;
    let inner_h = area.height.saturating_sub(2) as usize;
    let mut rows: Vec<Line<'static>> = Vec::new();
    for logical in text.split('\n') {
        let chars: Vec<char> = logical.chars().collect();
        for (s, e) in wrap_ranges(&chars, inner_w) {
            rows.push(Line::from(chars[s..e].iter().collect::<String>()));
        }
    }
    // Scroll to the tail: show the last `inner_h` rows.
    let scroll = (rows.len().saturating_sub(inner_h)) as u16;

    let para = Paragraph::new(Text::from(rows))
        .block(block)
        .scroll((scroll, 0));
    frame.render_widget(para, area);
}

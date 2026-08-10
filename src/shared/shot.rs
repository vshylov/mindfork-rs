//! Frame capture for generated screenshots (docs/history/demo-screenshots.md).
//!
//! Walks a rendered ratatui [`Buffer`] and serializes it cell by cell —
//! symbol, foreground, background, modifiers — into a JSON-stable
//! [`ShotFrame`]. `tools/screenshots.py` turns the dumps into images; a
//! drift-gate test (stage 2) compares freshly rendered frames against the
//! committed dumps. Test-gated on purpose: capture is a development concern,
//! and the release binary gains no surface from it (design plan §5).
//!
//! Cell widths follow `unicode-width`, i.e. the same table ratatui used to
//! lay the buffer out — the dump is faithful to the *buffer grid*, not to any
//! particular terminal's rendering (the VS16 divergence documented in
//! `shared/wrap.rs` does not arise here because capture reads placement, it
//! does not compute it).

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use serde::Serialize;
use unicode_width::UnicodeWidthStr;

use crate::shared::theme::{Palette, SHOT_CANVAS_DARK, SHOT_CANVAS_LIGHT};

/// One occupied cell. Trailing halves of wide glyphs are not emitted — the
/// renderer advances by `w` columns instead.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ShotCell {
    /// The grapheme in the cell.
    pub s: String,
    /// Columns the grapheme occupies (1 or 2). Skipped in JSON when 1.
    #[serde(skip_serializing_if = "is_narrow")]
    pub w: u8,
    /// Foreground as `#rrggbb`; `None` = the frame's `canvas_fg`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fg: Option<String>,
    /// Background as `#rrggbb`; `None` = the frame's `canvas_bg`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bg: Option<String>,
    /// Modifier letters: `B`old `D`im `I`talic `U`nderlined `R`eversed
    /// `H`idden `S`truck (crossed out), `L`/`F` slow/rapid blink.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub m: String,
}

fn is_narrow(w: &u8) -> bool {
    *w == 1
}

/// A captured frame: metadata plus the cell grid, row by row.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ShotFrame {
    pub screen: String,
    pub theme: String,
    pub locale: String,
    pub width: u16,
    pub height: u16,
    /// Canvas background (`theme::SHOT_CANVAS_*`) — what a cell without an
    /// explicit background renders on.
    pub canvas_bg: String,
    /// Default text color — what a cell without an explicit foreground uses.
    pub canvas_fg: String,
    pub rows: Vec<Vec<ShotCell>>,
}

/// Serializes a rendered buffer. `screen`/`theme`/`locale` are labels carried
/// into the dump (and its filename) verbatim.
pub fn capture(
    buffer: &Buffer,
    palette: &Palette,
    screen: &str,
    theme: &str,
    locale: &str,
) -> ShotFrame {
    let area = buffer.area();
    let canvas_bg = if palette.dark {
        SHOT_CANVAS_DARK
    } else {
        SHOT_CANVAS_LIGHT
    };
    // Dark/Light palettes define `text` as concrete RGB; Auto (never captured,
    // design plan §5) would fall through to a readable default.
    let canvas_fg = hex(palette.text)
        .unwrap_or_else(|| (if palette.dark { "#c9ccd2" } else { "#1e2024" }).into());

    let mut rows = Vec::with_capacity(area.height as usize);
    for y in area.top()..area.bottom() {
        let mut row = Vec::new();
        let mut x = area.left();
        while x < area.right() {
            let cell = &buffer[(x, y)];
            let symbol = cell.symbol();
            // A cell's primary symbol is 1 or 2 columns wide; clamp defensively
            // (a zero-width symbol must still advance, or the loop stalls).
            let w = UnicodeWidthStr::width(symbol).clamp(1, 2) as u16;
            row.push(ShotCell {
                s: symbol.to_string(),
                w: w as u8,
                fg: hex(cell.fg),
                bg: hex(cell.bg),
                m: mods(cell.modifier),
            });
            x += w;
        }
        rows.push(row);
    }

    ShotFrame {
        screen: screen.into(),
        theme: theme.into(),
        locale: locale.into(),
        width: area.width,
        height: area.height,
        canvas_bg: hex(canvas_bg).expect("canvas constants are RGB"),
        canvas_fg,
        rows,
    }
}

/// `Color` → `#rrggbb`. `Reset` → `None` (the frame's canvas supplies it).
/// Named ANSI colors map to the xterm defaults so a dump never depends on a
/// terminal that isn't there; the capturable palettes are RGB throughout
/// except `light().user == Color::Blue`.
fn hex(color: Color) -> Option<String> {
    let (r, g, b) = match color {
        Color::Reset => return None,
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0x00, 0x00, 0x00),
        Color::Red => (0xcd, 0x00, 0x00),
        Color::Green => (0x00, 0xcd, 0x00),
        Color::Yellow => (0xcd, 0xcd, 0x00),
        Color::Blue => (0x00, 0x00, 0xee),
        Color::Magenta => (0xcd, 0x00, 0xcd),
        Color::Cyan => (0x00, 0xcd, 0xcd),
        Color::Gray => (0xe5, 0xe5, 0xe5),
        Color::DarkGray => (0x7f, 0x7f, 0x7f),
        Color::LightRed => (0xff, 0x00, 0x00),
        Color::LightGreen => (0x00, 0xff, 0x00),
        Color::LightYellow => (0xff, 0xff, 0x00),
        Color::LightBlue => (0x5c, 0x5c, 0xff),
        Color::LightMagenta => (0xff, 0x00, 0xff),
        Color::LightCyan => (0x00, 0xff, 0xff),
        Color::White => (0xff, 0xff, 0xff),
        Color::Indexed(i) => xterm256(i),
    };
    Some(format!("#{r:02x}{g:02x}{b:02x}"))
}

/// The standard xterm 256-color table: 16 named, a 6x6x6 cube, a gray ramp.
fn xterm256(i: u8) -> (u8, u8, u8) {
    const NAMED: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0xcd, 0x00, 0x00),
        (0x00, 0xcd, 0x00),
        (0xcd, 0xcd, 0x00),
        (0x00, 0x00, 0xee),
        (0xcd, 0x00, 0xcd),
        (0x00, 0xcd, 0xcd),
        (0xe5, 0xe5, 0xe5),
        (0x7f, 0x7f, 0x7f),
        (0xff, 0x00, 0x00),
        (0x00, 0xff, 0x00),
        (0xff, 0xff, 0x00),
        (0x5c, 0x5c, 0xff),
        (0xff, 0x00, 0xff),
        (0x00, 0xff, 0xff),
        (0xff, 0xff, 0xff),
    ];
    const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];
    match i {
        0..=15 => NAMED[i as usize],
        16..=231 => {
            let n = i - 16;
            (
                CUBE[(n / 36) as usize],
                CUBE[((n / 6) % 6) as usize],
                CUBE[(n % 6) as usize],
            )
        }
        232..=255 => {
            let v = 8 + 10 * (i - 232);
            (v, v, v)
        }
    }
}

fn mods(m: Modifier) -> String {
    const MAP: [(Modifier, char); 9] = [
        (Modifier::BOLD, 'B'),
        (Modifier::DIM, 'D'),
        (Modifier::ITALIC, 'I'),
        (Modifier::UNDERLINED, 'U'),
        (Modifier::SLOW_BLINK, 'L'),
        (Modifier::RAPID_BLINK, 'F'),
        (Modifier::REVERSED, 'R'),
        (Modifier::HIDDEN, 'H'),
        (Modifier::CROSSED_OUT, 'S'),
    ];
    MAP.iter()
        .filter(|(flag, _)| m.contains(*flag))
        .map(|(_, c)| *c)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    fn palette() -> Palette {
        Palette::for_theme(crate::shared::config::Theme::Dark)
    }

    /// A wide glyph is emitted once with `w: 2` and its trailing cell is
    /// swallowed; every row's widths sum to the grid width.
    #[test]
    fn wide_glyph_spans_two_columns() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 1));
        buf.set_string(0, 0, "日x", Style::default());
        let frame = capture(&buf, &palette(), "t", "dark", "en");
        let row = &frame.rows[0];
        assert_eq!(row[0].s, "日");
        assert_eq!(row[0].w, 2);
        assert_eq!(row[1].s, "x");
        let total: u16 = row.iter().map(|c| c.w as u16).sum();
        assert_eq!(total, 4, "row must cover the grid exactly: {row:?}");
    }

    /// `Reset` means "canvas decides" — it must serialize as absent, not as a
    /// made-up color; RGB and named colors serialize as hex.
    #[test]
    fn colors_map_to_hex_and_reset_to_none() {
        assert_eq!(hex(Color::Reset), None);
        assert_eq!(hex(Color::Rgb(15, 17, 21)).unwrap(), "#0f1115");
        assert_eq!(hex(Color::Blue).unwrap(), "#0000ee");
        assert_eq!(hex(Color::Indexed(196)).unwrap(), "#ff0000");
        assert_eq!(hex(Color::Indexed(244)).unwrap(), "#808080");
    }

    /// The modifier string is stable and ordered; styled cells carry it.
    #[test]
    fn modifiers_serialize_as_letters() {
        assert_eq!(
            mods(Modifier::BOLD | Modifier::UNDERLINED | Modifier::ITALIC),
            "BIU"
        );
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        buf.set_string(
            0,
            0,
            "ab",
            Style::default()
                .fg(Color::Rgb(1, 2, 3))
                .add_modifier(Modifier::BOLD),
        );
        let frame = capture(&buf, &palette(), "t", "dark", "en");
        assert_eq!(frame.rows[0][0].m, "B");
        assert_eq!(frame.rows[0][0].fg.as_deref(), Some("#010203"));
        assert_eq!(frame.rows[0][0].bg, None, "no background was set");
    }

    /// The frame's canvas comes from the palette side (`dark` flag), and the
    /// dark canvas matches the constant next to the palettes.
    #[test]
    fn canvas_follows_the_palette() {
        let buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        let dark = capture(&buf, &palette(), "t", "dark", "en");
        assert_eq!(dark.canvas_bg, "#0f1115");
        assert_eq!(dark.canvas_fg, "#c9ccd2");
        let light = capture(
            &buf,
            &Palette::for_theme(crate::shared::config::Theme::Light),
            "t",
            "light",
            "en",
        );
        assert_eq!(light.canvas_bg, "#fafafc");
        assert_eq!(light.canvas_fg, "#1e2024");
    }
}

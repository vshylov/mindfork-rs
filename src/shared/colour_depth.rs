//! How many colours the terminal draws (spec §11.6, *Colour depth*): 24-bit
//! RGB, or the 256 of the xterm palette.
//!
//! The themes are RGB. Terminal.app has 24-bit colour only since macOS 26, and
//! before it an RGB sequence is not drawn but misread: measured on macOS 15
//! (Terminal 455.1), the `full` mode's canvas was never painted, its light text
//! sat on white, and a row came out bright green
//! (docs/research/macos.md §14.4). Its 256 colours it draws. So where the
//! environment says the terminal has no 24-bit colour, every colour of a
//! finished frame is replaced by the nearest of the 256 ([`reduce`]).
//!
//! The rule is narrow on purpose: `TERM_PROGRAM=Apple_Terminal` without
//! `COLORTERM=truecolor`. Terminal on macOS 26 sets `truecolor`, and the one
//! before it sets nothing. A general "no `COLORTERM`, no RGB" would take colour
//! from terminals that draw RGB and say nothing — Windows' console host among
//! them.

use std::ffi::OsStr;
use std::sync::OnceLock;

use ratatui::buffer::Buffer;
use ratatui::style::Color;

/// What the terminal draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColourDepth {
    /// 24-bit RGB: the colours go out as they are.
    #[default]
    TrueColour,
    /// The xterm palette's 256: every RGB colour goes out as its nearest.
    Ansi256,
}

static DEPTH: OnceLock<ColourDepth> = OnceLock::new();

impl ColourDepth {
    /// The depth the environment names: [`ColourDepth::Ansi256`] for
    /// Terminal.app without `COLORTERM=truecolor` (or `24bit`), 24-bit
    /// everywhere else.
    pub fn for_environment(term_program: Option<&OsStr>, colorterm: Option<&OsStr>) -> Self {
        let apple = term_program.is_some_and(|p| p == "Apple_Terminal");
        let rgb = colorterm.is_some_and(|c| c == "truecolor" || c == "24bit");
        if apple && !rgb {
            ColourDepth::Ansi256
        } else {
            ColourDepth::TrueColour
        }
    }

    /// The depth in effect: recorded once at start-up ([`set`]), 24-bit until
    /// then — which is what every test and every screenshot draws with.
    pub fn current() -> Self {
        DEPTH.get().copied().unwrap_or_default()
    }
}

/// Records the terminal's depth. Called once, from start-up; later calls are
/// ignored.
pub fn set(depth: ColourDepth) {
    let _ = DEPTH.set(depth);
}

/// The six levels of each channel of the palette's 6×6×6 cube (16–231).
const CUBE: [u8; 6] = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];

/// The cube level nearest to a channel's value: the midpoints between the
/// levels are 48, 115, 155, 195 and 235.
fn cube_level(v: u8) -> usize {
    match v {
        0..48 => 0,
        48..115 => 1,
        _ => usize::from((v - 35) / 40),
    }
}

fn distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let d = |x: u8, y: u8| (i32::from(x) - i32::from(y)).unsigned_abs().pow(2);
    d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
}

/// The index of the palette colour nearest to `(r, g, b)`: the nearest cube
/// colour or the nearest of the 24 greys (232–255), whichever is closer.
/// The first 16 are never chosen — a terminal's theme redefines them.
pub fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    let (qr, qg, qb) = (cube_level(r), cube_level(g), cube_level(b));
    let cube = (CUBE[qr], CUBE[qg], CUBE[qb]);
    // `qr`, `qg`, `qb` are below 6, so the index stays within 16..=231.
    let cube_index = (16 + 36 * qr + 6 * qg + qb) as u8;
    if cube == (r, g, b) {
        return cube_index;
    }
    let average = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    // The greys are 8, 18, … 238: the nearest one to the average.
    let step = ((average.saturating_sub(3)) / 10).min(23) as u8;
    let grey = 8 + 10 * step;
    if distance((grey, grey, grey), (r, g, b)) < distance(cube, (r, g, b)) {
        232 + step
    } else {
        cube_index
    }
}

/// An RGB colour as its nearest of the 256; any other colour as it is.
pub fn reduced(color: Color) -> Color {
    match color {
        Color::Rgb(r, g, b) => Color::Indexed(nearest_256(r, g, b)),
        other => other,
    }
}

/// Replaces every RGB colour of a **finished** frame — foreground, background
/// and underline — with its nearest of the 256. The last pass of
/// `shared::ui::finish_frame`, after the canvas and the monochrome strip, so
/// it sees every colour the frame will send.
pub fn reduce(buf: &mut Buffer) {
    for cell in buf.content.iter_mut() {
        cell.fg = reduced(cell.fg);
        cell.bg = reduced(cell.bg);
        cell.underline_color = reduced(cell.underline_color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    fn env(term_program: Option<&str>, colorterm: Option<&str>) -> ColourDepth {
        ColourDepth::for_environment(term_program.map(OsStr::new), colorterm.map(OsStr::new))
    }

    #[test]
    fn only_terminal_app_without_truecolor_gets_256() {
        // macOS 15's Terminal (455.1) sets no COLORTERM; macOS 26's sets truecolor.
        assert_eq!(env(Some("Apple_Terminal"), None), ColourDepth::Ansi256);
        assert_eq!(env(Some("Apple_Terminal"), Some("")), ColourDepth::Ansi256);
        assert_eq!(
            env(Some("Apple_Terminal"), Some("truecolor")),
            ColourDepth::TrueColour
        );
        assert_eq!(
            env(Some("Apple_Terminal"), Some("24bit")),
            ColourDepth::TrueColour
        );
        // Terminals that draw RGB, whether they say so or not.
        assert_eq!(
            env(Some("iTerm.app"), Some("truecolor")),
            ColourDepth::TrueColour
        );
        assert_eq!(env(Some("ghostty"), None), ColourDepth::TrueColour);
        assert_eq!(env(None, None), ColourDepth::TrueColour);
    }

    #[test]
    fn a_cube_colour_is_itself() {
        assert_eq!(nearest_256(0, 0, 0), 16);
        assert_eq!(nearest_256(0xff, 0xff, 0xff), 231);
        assert_eq!(nearest_256(0xff, 0, 0), 196);
        assert_eq!(nearest_256(0x5f, 0x87, 0xaf), 16 + 36 + 12 + 3);
    }

    #[test]
    fn a_near_grey_goes_to_the_grey_ramp() {
        // The dark theme's canvas: far from the cube's black, next to grey 18.
        assert_eq!(nearest_256(15, 17, 21), 233);
        assert_eq!(nearest_256(8, 8, 8), 232);
        assert_eq!(nearest_256(238, 238, 238), 255);
        assert_eq!(
            nearest_256(250, 250, 250),
            231,
            "past the last grey, white wins"
        );
    }

    #[test]
    fn a_colour_goes_to_the_nearest_cube_corner() {
        // The magenta of a table header Terminal drew on macOS 15.
        assert_eq!(nearest_256(230, 140, 229), 16 + 36 * 4 + 6 * 2 + 4);
        // Every channel within its level's half-way marks.
        for v in [0u8, 47, 48, 114, 115, 154, 155, 194, 195, 234, 235, 255] {
            let level = cube_level(v);
            let below = level.checked_sub(1).map(|l| CUBE[l]);
            let above = CUBE.get(level + 1).copied();
            let here = i32::from(CUBE[level]);
            for other in [below, above].into_iter().flatten() {
                assert!(
                    (i32::from(v) - here).abs() <= (i32::from(v) - i32::from(other)).abs(),
                    "{v} -> level {level}"
                );
            }
        }
    }

    #[test]
    fn only_rgb_is_reduced() {
        assert_eq!(reduced(Color::Rgb(0xff, 0, 0)), Color::Indexed(196));
        assert_eq!(reduced(Color::Reset), Color::Reset);
        assert_eq!(reduced(Color::Blue), Color::Blue);
        assert_eq!(reduced(Color::Indexed(42)), Color::Indexed(42));
    }

    #[test]
    fn a_frame_keeps_no_rgb() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
        buf.set_string(
            0,
            0,
            "a",
            Style::new()
                .fg(Color::Rgb(15, 17, 21))
                .bg(Color::Rgb(0xff, 0, 0)),
        );
        buf.set_string(1, 0, "b", Style::new().underline_color(Color::Rgb(0, 0, 0)));
        buf.set_string(2, 0, "c", Style::new().fg(Color::Green));
        reduce(&mut buf);
        let cell = |x: u16| buf[(x, 0)].clone();
        assert_eq!(
            (cell(0).fg, cell(0).bg),
            (Color::Indexed(233), Color::Indexed(196))
        );
        assert_eq!(cell(1).underline_color, Color::Indexed(16));
        assert_eq!(
            cell(2).fg,
            Color::Green,
            "a named colour is the terminal's own"
        );
    }
}

//! The terminal's mouse reports, as the app asks for them
//! (docs/research/mouse-always-captured.md §5.1, spec §11.3).

use std::fmt;
use std::time::{Duration, Instant};

use ratatui::crossterm::Command;

/// What [`EnableMouseReports`] writes: button reports (`?1000h`), drag reports
/// — a movement with a button held (`?1002h`) — and the SGR encoding
/// (`?1006h`), which carries any column and tells a release by its button.
pub const MOUSE_REPORTS_ON: &str = "\x1b[?1000h\x1b[?1002h\x1b[?1006h";

/// Asks the terminal for button and drag reports in the SGR encoding.
///
/// Not crossterm's `EnableMouseCapture`, which also asks for **every movement**
/// (`?1003h`) and for the legacy rxvt encoding (`?1015h`): nothing in the app
/// answers a hover, and each movement was a report, a wake-up and a frame
/// (research §3, §4.6). crossterm parses all three encodings, so asking for
/// fewer changes nothing it reads.
///
/// **Windows**: the console is asked the way crossterm asks it — its input
/// mode, no VT. No console mode gives buttons and drags alone, so the console
/// reports movement regardless; the input reader drops it
/// (`app::runtime`'s `collect_press`).
///
/// Lifted by crossterm's `DisableMouseCapture`, which resets all five of its
/// modes — harmless for the two never set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnableMouseReports;

impl Command for EnableMouseReports {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        f.write_str(MOUSE_REPORTS_ON)
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        ratatui::crossterm::event::EnableMouseCapture.execute_winapi()
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        false
    }
}

/// What a left press selects by: one press a character at a time, a second on
/// the same cell a word, a third the line (docs/research/mouse-always-captured.md
/// F5). A drag after a double or a triple press grows the selection by that
/// unit, the way a text field in a GUI does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectUnit {
    Char,
    Word,
    Line,
}

/// How soon a press must follow the one before it, on the same cell, to count
/// as the next of a double or a triple click. The figure GUIs default to
/// (Windows' `GetDoubleClickTime`); Codex counts 400 ms (research §4.5).
pub const MULTI_CLICK: Duration = Duration::from_millis(500);

/// Counts the presses of a double and a triple click: the same cell, each
/// within [`MULTI_CLICK`] of the one before. A fourth press starts over at a
/// character, so a fast run of clicks cycles rather than sticking on the line.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClickCounter {
    last: Option<(u16, u16, Instant, SelectUnit)>,
}

impl ClickCounter {
    /// A left press at `(col, row)` at `now`: the unit it selects by.
    pub fn press(&mut self, col: u16, row: u16, now: Instant) -> SelectUnit {
        let unit = match self.last {
            Some((c, r, at, unit))
                if (c, r) == (col, row) && now.saturating_duration_since(at) <= MULTI_CLICK =>
            {
                match unit {
                    SelectUnit::Char => SelectUnit::Word,
                    SelectUnit::Word => SelectUnit::Line,
                    SelectUnit::Line => SelectUnit::Char,
                }
            }
            _ => SelectUnit::Char,
        };
        self.last = Some((col, row, now, unit));
        unit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presses_on_one_cell_count_up_to_a_line_and_start_over() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut clicks = ClickCounter::default();
        assert_eq!(clicks.press(3, 4, t0), SelectUnit::Char);
        assert_eq!(clicks.press(3, 4, t0 + ms(200)), SelectUnit::Word);
        assert_eq!(clicks.press(3, 4, t0 + ms(400)), SelectUnit::Line);
        assert_eq!(clicks.press(3, 4, t0 + ms(600)), SelectUnit::Char);
        // Each press is measured from the one before it, not from the first.
        assert_eq!(clicks.press(3, 4, t0 + ms(1100)), SelectUnit::Word);
    }

    #[test]
    fn another_cell_or_a_slow_press_is_a_first_click() {
        let t0 = Instant::now();
        let mut clicks = ClickCounter::default();
        clicks.press(3, 4, t0);
        assert_eq!(clicks.press(4, 4, t0), SelectUnit::Char, "another cell");
        assert_eq!(
            clicks.press(4, 4, t0 + MULTI_CLICK + Duration::from_millis(1)),
            SelectUnit::Char,
            "too slow"
        );
        assert_eq!(
            clicks.press(4, 4, t0 + MULTI_CLICK * 2),
            SelectUnit::Word,
            "exactly on the limit still counts"
        );
    }

    /// The exact bytes: buttons, drags, SGR — and neither every movement
    /// (`?1003h`) nor the rxvt encoding (`?1015h`).
    #[test]
    fn the_reports_asked_for_are_buttons_and_drags_in_sgr() {
        let mut out = String::new();
        EnableMouseReports.write_ansi(&mut out).unwrap();
        assert_eq!(out, "\u{1b}[?1000h\u{1b}[?1002h\u{1b}[?1006h");
        assert!(!out.contains("1003"));
        assert!(!out.contains("1015"));
    }
}

//! The terminal's mouse reports, as the app asks for them
//! (docs/research/mouse-always-captured.md §5.1, spec §11.3).

use std::fmt;

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

#[cfg(test)]
mod tests {
    use super::*;

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

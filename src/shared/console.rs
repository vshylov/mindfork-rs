//! Keeping a fatal message readable when the console is about to disappear.
//!
//! A console application started from Explorer — a double-click on
//! `mindfork.exe`, the shortcut the installer creates — gets a console of its
//! own, and Windows destroys that window the moment the process exits. Every
//! refusal the app prints before the interface opens (another instance already
//! running, a corrupt `defaults.json`, storage that will not open) therefore
//! reached a window that closed too fast to read, which is indistinguishable
//! from "it does nothing when I click it".
//!
//! Whether that is the situation is a question Windows answers:
//! `GetConsoleProcessList` reports how many processes are attached to this
//! console. Measured on a development machine
//! (docs/research/robustness-and-defaults.md §2.2): started from a shell — **3**;
//! started with a console of its own — **1**. One means nobody else is there, so
//! nothing survives our exit.
//!
//! Unix has no equivalent problem (a terminal that was already open stays open),
//! so there the answer is always "no".

use std::io::{IsTerminal, Read};

use crate::shared::i18n::Locale;

/// Whether this process is the **only** one attached to its console.
///
/// Windows only, via `GetConsoleProcessList`; anywhere else this is `false`, and
/// deliberately so — a unix terminal outlives the process that printed into it.
#[cfg(windows)]
fn sole_console_owner() -> bool {
    use windows_sys::Win32::System::Console::GetConsoleProcessList;

    // The buffer is a formality: the count is the return value, and a buffer too
    // small only means the answer is larger than 2 — which is not 1 either way.
    let mut pids = [0u32; 4];
    let count = unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) };
    count == 1
}

#[cfg(not(windows))]
fn sole_console_owner() -> bool {
    false
}

/// Whether a message just printed would be lost without waiting for the reader.
///
/// A pure function so both arms are testable: the environment is asked once, in
/// [`hold_if_sole_owner`]. `stdout_is_terminal` is the second condition and not a
/// formality — output redirected to a file or a pipe has a reader that is not a
/// person, and blocking there would hang a script (the same reasoning that makes
/// `refuses_tui_launch` in `main.rs` ask about stdout rather than stdin).
fn should_hold(sole_owner: bool, stdout_is_terminal: bool) -> bool {
    sole_owner && stdout_is_terminal
}

/// Waits for Enter when the console belongs to this process alone, so the line
/// just printed can be read. Does nothing otherwise, and never fails: a console
/// that cannot be read from is one more reason to exit rather than to hang.
pub fn hold_if_sole_owner(loc: &Locale) {
    if !should_hold(sole_console_owner(), std::io::stdout().is_terminal()) {
        return;
    }
    eprintln!("{}", loc.t("cli.console.press_enter"));
    // One byte is enough — Enter, or anything else the user sends before it. A
    // closed stdin returns `Ok(0)` immediately, which is the right answer too.
    let mut byte = [0u8; 1];
    let _ = std::io::stdin().read(&mut byte);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_sole_owner_with_a_terminal_waits() {
        // The case this exists for: a double-clicked binary, alone in its console.
        assert!(should_hold(true, true));
        // A shell is attached — the message stays on screen by itself.
        assert!(!should_hold(false, true));
        // Redirected output: the reader is a file or a pipe, and waiting would
        // hang whatever is driving the app.
        assert!(!should_hold(true, false));
        assert!(!should_hold(false, false));
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_never_waits() {
        assert!(!sole_console_owner());
    }
}

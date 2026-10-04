//! Terminal input for the CLI commands (no TUI): the hidden backup-password
//! prompt, and discarding whatever was typed while a long command was running.
//!
//! The prompt is used by the CLI `restore` path (docs/history/backup-password.md
//! §4 F6): restoring a foreign archive on a fresh machine is exactly the case
//! where there is no stored password, and the only alternative would be
//! `--password` on the command line — which lands in the shell history and the
//! process list.
//!
//! No new dependency: `crossterm` is already used by the TUI, and raw mode is
//! what suppresses the echo. Both functions are **skipped when stdin is not a
//! terminal** (a pipe, a CI job, a service): prompting there would hang forever
//! instead of failing with a message, and discarding would eat piped input.

use std::io::{IsTerminal, Write};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal;

/// Whether an interactive prompt is possible at all (stdin is a terminal).
pub fn is_interactive() -> bool {
    std::io::stdin().is_terminal()
}

/// Prompts for a password with the input hidden.
///
/// `Ok(None)` — the user cancelled (`Esc`/`Ctrl+C`) or submitted an empty line.
/// `Err` — the terminal could not be switched into raw mode.
///
/// Deliberately hand-rolled rather than pulling in `rpassword`: the surface is a
/// dozen lines, and the project's precedent is its own micro-solution when the
/// alternative is a crate for one function (ADR 0003, `features/cli.rs`).
///
/// The prompt goes to **stderr**, like every password prompt that expects its
/// command's output to be redirected: `mindfork stats backup.zip --json > a.json`
/// would otherwise hide the question from the user and write it into the file.
pub fn read_password(prompt: &str) -> std::io::Result<Option<String>> {
    eprint!("{prompt}");
    std::io::stderr().flush()?;

    terminal::enable_raw_mode()?;
    let result = read_line_raw();
    // Restore the terminal whatever happened — a leaked raw mode would leave the
    // user's shell without echo.
    let _ = terminal::disable_raw_mode();
    eprintln!();

    result
}

/// A backstop against an unresponsive terminal: a drain can't outlive this many
/// events. Real type-ahead is a handful of keystrokes.
const MAX_DISCARDED_EVENTS: usize = 4096;

/// Discards keystrokes typed while a long command was running.
///
/// Packing or unpacking a real data root takes seconds, and keys pressed in the
/// meantime — an impatient `Enter` after the password prompt, most of all — sit
/// in the console input buffer untouched. They were typed at *us*, but nothing
/// here reads them, so on exit the shell inherits them and replays them as its
/// own command line. Discarding is the standard fix, and it is safe because
/// there is no other consumer: the CLI is done reading by the time this runs.
///
/// Deliberately silent about failures — a terminal we can't poll is exactly the
/// case where there is nothing to discard.
pub fn discard_type_ahead() {
    if !is_interactive() {
        return;
    }
    for _ in 0..MAX_DISCARDED_EVENTS {
        match event::poll(Duration::ZERO) {
            Ok(true) => {
                if event::read().is_err() {
                    return;
                }
            }
            _ => return,
        }
    }
}

/// Reads characters until Enter, in raw mode (nothing is echoed).
fn read_line_raw() -> std::io::Result<Option<String>> {
    let mut buf = String::new();
    loop {
        // Key *release* events also arrive on Windows; only presses count.
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        match password_key(&key) {
            PasswordKey::Done => return Ok(Some(buf).filter(|s| !s.is_empty())),
            PasswordKey::Cancel => return Ok(None),
            PasswordKey::Erase => {
                buf.pop();
            }
            PasswordKey::Text(c) => buf.push(c),
            PasswordKey::Nothing => {}
        }
    }
}

/// What one key press does to a password being typed.
#[derive(Debug, PartialEq)]
enum PasswordKey {
    Done,
    Cancel,
    Erase,
    Text(char),
    Nothing,
}

fn password_key(key: &KeyEvent) -> PasswordKey {
    match key.code {
        KeyCode::Enter => PasswordKey::Done,
        KeyCode::Esc => PasswordKey::Cancel,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => PasswordKey::Cancel,
        KeyCode::Backspace => PasswordKey::Erase,
        // A password is data, not a shortcut: only a bare (or shifted)
        // character is text — `Ctrl+<char>` must not end up in the buffer.
        // AltGr's characters are text: Windows reports AltGr as Ctrl+Alt, and
        // `@` is AltGr+Q on a German keyboard — dropping it would make the
        // password wrong in silence (`keys::is_altgr_text`).
        KeyCode::Char(c)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                || crate::shared::keys::is_altgr_text(key) =>
        {
            PasswordKey::Text(c)
        }
        _ => PasswordKey::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What each key does to a password: the text it types, and the keys that
    /// end, cancel or erase — an AltGr character among the text (`@` on a
    /// German keyboard arrives as Ctrl+Alt on Windows), a Ctrl shortcut not.
    #[test]
    fn a_password_takes_text_altgr_included_and_no_shortcut() {
        let key = |code, modifiers| KeyEvent::new(code, modifiers);
        let ctrl_alt = KeyModifiers::CONTROL | KeyModifiers::ALT;
        crate::shared::keys::pretend_altgr(Some('@'));
        for (k, want) in [
            (
                key(KeyCode::Char('p'), KeyModifiers::NONE),
                PasswordKey::Text('p'),
            ),
            (
                key(KeyCode::Char('P'), KeyModifiers::SHIFT),
                PasswordKey::Text('P'),
            ),
            (key(KeyCode::Char('@'), ctrl_alt), PasswordKey::Text('@')),
            (key(KeyCode::Char('a'), ctrl_alt), PasswordKey::Nothing),
            (
                key(KeyCode::Char('v'), KeyModifiers::CONTROL),
                PasswordKey::Nothing,
            ),
            (
                key(KeyCode::Char('c'), KeyModifiers::CONTROL),
                PasswordKey::Cancel,
            ),
            (key(KeyCode::Esc, KeyModifiers::NONE), PasswordKey::Cancel),
            (key(KeyCode::Enter, KeyModifiers::NONE), PasswordKey::Done),
            (
                key(KeyCode::Backspace, KeyModifiers::NONE),
                PasswordKey::Erase,
            ),
            (key(KeyCode::Tab, KeyModifiers::NONE), PasswordKey::Nothing),
        ] {
            assert_eq!(password_key(&k), want, "{k:?}");
        }
        crate::shared::keys::pretend_altgr(None);
    }
}

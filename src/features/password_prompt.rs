//! Reading a backup password from the terminal without echoing it.
//!
//! Used by the CLI `restore` path (docs/backup-password.md §4 F6): restoring a
//! foreign archive on a fresh machine is exactly the case where there is no
//! stored password, and the only alternative would be `--password` on the
//! command line — which lands in the shell history and the process list.
//!
//! No new dependency: `crossterm` is already used by the TUI, and raw mode is
//! what suppresses the echo. The prompt is **skipped when stdin is not a
//! terminal** (a pipe, a CI job, a service): prompting there would hang forever
//! instead of failing with a message.

use std::io::{IsTerminal, Write};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
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
pub fn read_password(prompt: &str) -> std::io::Result<Option<String>> {
    print!("{prompt}");
    std::io::stdout().flush()?;

    terminal::enable_raw_mode()?;
    let result = read_line_raw();
    // Restore the terminal whatever happened — a leaked raw mode would leave the
    // user's shell without echo.
    let _ = terminal::disable_raw_mode();
    println!();

    result
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
        match key.code {
            KeyCode::Enter => return Ok(Some(buf).filter(|s| !s.is_empty())),
            KeyCode::Esc => return Ok(None),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
            KeyCode::Backspace => {
                buf.pop();
            }
            // A password is data, not a shortcut: only a bare (or shifted)
            // character is text — `Ctrl+<char>` must not end up in the buffer.
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => buf.push(c),
            _ => {}
        }
    }
}

//! `mindfork keys` — what the app receives for each key, and what the chat's
//! input box does with it (docs/research/macos.md §13.4, fork F7).
//!
//! A terminal decides which chords reach a program at all, and in what form:
//! Terminal.app keeps `Ctrl+←/→` for Spaces and sends Option+← as `ESC b`,
//! Konsole's `Shift+Enter` is a sequence crossterm drops. "The key did nothing"
//! says none of that; the event that arrived does. So the terminal is put into
//! the very modes the app reads keys in ([`runtime::enable_key_modes`]), each
//! event is printed as crossterm reports it, and beside it the outcome — a
//! sketch of an input box that starts every key from the same text, run through
//! the real [`InputBox`] and the chat's own line-break rule.
//!
//! Nothing is created: no data root, no log. `--output` copies the lines into
//! a file, for a session read from elsewhere (macos.md §13.3).

use std::fs::File;
use std::io::{IsTerminal, Write, stdout};
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
#[cfg(unix)]
use ratatui::crossterm::event::PopKeyboardEnhancementFlags;
use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent,
    KeyModifiers, MouseEvent,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode};

use crate::app::runtime::{self, Chunk};
use crate::shared::i18n::Locale;
use crate::shared::keys;
use crate::widgets::input_box::{InputBox, KeyOutcome};

/// The text every key starts from, and the cursor's column in it — between two
/// words, so a move by a word and by a character land on different marks.
const SAMPLE: &str = "one two three";
const SAMPLE_CURSOR: usize = 8;

/// Where an echoed line goes: the terminal, raw, and the `--output` file.
struct Echo {
    file: Option<File>,
}

impl Echo {
    /// One line. Raw mode does not turn `\n` into a new line, so the terminal
    /// gets `\r\n`; the file gets plain `\n`.
    fn line(&mut self, text: &str) -> Result<()> {
        let mut out = stdout();
        write!(out, "{text}\r\n")?;
        out.flush()?;
        if let Some(file) = &mut self.file {
            writeln!(file, "{text}")?;
        }
        Ok(())
    }
}

/// Restores the terminal however the loop ends — an error included.
struct Restore {
    mouse: bool,
}

impl Drop for Restore {
    fn drop(&mut self) {
        if self.mouse {
            let _ = execute!(stdout(), DisableMouseCapture);
        }
        let _ = execute!(stdout(), DisableBracketedPaste);
        #[cfg(unix)]
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
        let _ = disable_raw_mode();
    }
}

/// What the echo itself does with an event, beside printing it.
#[derive(Debug, PartialEq, Eq)]
enum Control {
    Go,
    ToggleMouse,
    Quit,
}

/// The app's own quit (`Ctrl+Q`, `F10`) ends the echo too, and its mouse
/// switch (`Ctrl+W`) switches it here; everything else is only shown.
fn control_of(key: &KeyEvent) -> Control {
    if key.code == KeyCode::F(10) {
        return Control::Quit;
    }
    if key.modifiers == KeyModifiers::CONTROL {
        match keys::hotkey_char(key) {
            Some('q') => return Control::Quit,
            Some('w') => return Control::ToggleMouse,
            _ => {}
        }
    }
    Control::Go
}

/// Runs the echo until `Ctrl+Q` or `F10`. A terminal on stdout was checked by
/// the caller (`main`'s launch gate).
pub fn run(output: Option<&Path>, loc: &Locale) -> Result<ExitCode> {
    debug_assert!(stdout().is_terminal());
    let file = output
        .map(|path| {
            // A record of one terminal is not written over by the next one's.
            File::create_new(path).with_context(|| {
                loc.tf(
                    "cli.keys.cannot_write",
                    &[("path", &path.display().to_string())],
                )
            })
        })
        .transpose()?;
    let mut echo = Echo { file };

    enable_raw_mode()?;
    let mut restore = Restore { mouse: false };
    let kitty = runtime::enable_key_modes();
    for line in header(loc, kitty) {
        echo.line(&line)?;
    }
    // Read as the app reads: a batch at a time, presses only, a burst of text
    // keys taken for one paste (Windows has no bracketed paste) — the echo
    // shows what the screens are handed, not each key of a paste.
    'echo: loop {
        for chunk in runtime::chunk_batch(runtime::read_batch()?) {
            let control = match &chunk {
                Chunk::Event(Event::Key(key)) => control_of(key),
                _ => Control::Go,
            };
            if control == Control::ToggleMouse {
                restore.mouse = !restore.mouse;
                if restore.mouse {
                    execute!(stdout(), EnableMouseCapture)?;
                } else {
                    execute!(stdout(), DisableMouseCapture)?;
                }
            }
            echo.line(&describe(&chunk, &control, restore.mouse, loc))?;
            if control == Control::Quit {
                break 'echo;
            }
        }
    }
    drop(restore);
    if let Some(path) = output {
        println!(
            "{}",
            loc.tf("cli.keys.saved", &[("path", &path.display().to_string())])
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// The lines printed before the first key: the terminal, what it answered, and
/// how to read and leave the echo.
fn header(loc: &Locale, kitty: Option<bool>) -> Vec<String> {
    let kitty = match kitty {
        Some(true) => loc.t("cli.keys.kitty_on"),
        Some(false) => loc.t("cli.keys.kitty_off"),
        None => loc.t("cli.keys.kitty_none"),
    };
    vec![
        loc.tf("cli.keys.title", &[("version", env!("CARGO_PKG_VERSION"))]),
        loc.tf("cli.keys.terminal", &[("terminal", &terminal_facts())]),
        kitty.to_string(),
        loc.tf("cli.keys.newline", &[("chord", keys::newline_chord())]),
        loc.tf("cli.keys.sample", &[("sample", &sketch(&sample_box()))]),
        loc.t("cli.keys.controls").to_string(),
        String::new(),
    ]
}

/// The OS and the variables a terminal names itself by — the facts a report
/// needs and a user would not know to give.
fn terminal_facts() -> String {
    let mut facts = vec![std::env::consts::OS.to_string()];
    for name in ["TERM_PROGRAM", "TERM_PROGRAM_VERSION", "TERM", "COLORTERM"] {
        if let Ok(value) = std::env::var(name)
            && !value.is_empty()
        {
            facts.push(format!("{name}={value}"));
        }
    }
    if std::env::var_os("WT_SESSION").is_some() {
        facts.push("WT_SESSION".to_string());
    }
    facts.join(", ")
}

/// One echoed line for a piece of a batch. A key's release never gets here:
/// the batch keeps presses only, as the app's does.
fn describe(chunk: &Chunk, control: &Control, mouse: bool, loc: &Locale) -> String {
    let (label, outcome) = match chunk {
        // A burst of text keys the app takes for one paste (Windows).
        Chunk::Paste(text) => ("Paste (keys)".to_string(), paste_outcome(text, loc)),
        Chunk::Event(Event::Paste(text)) => ("Paste".to_string(), paste_outcome(text, loc)),
        Chunk::Event(Event::Key(key)) => {
            let outcome = match control {
                Control::Quit => loc.t("cli.keys.quit").to_string(),
                Control::ToggleMouse if mouse => loc.t("cli.keys.mouse_on").to_string(),
                Control::ToggleMouse => loc.t("cli.keys.mouse_off").to_string(),
                Control::Go => key_outcome(key, loc),
            };
            (key_label(key), outcome)
        }
        Chunk::Event(Event::Mouse(mouse)) => ("Mouse".to_string(), mouse_outcome(mouse)),
        Chunk::Event(Event::Resize(width, height)) => (
            "Resize".to_string(),
            loc.tf(
                "cli.keys.window",
                &[
                    ("width", &width.to_string()),
                    ("height", &height.to_string()),
                ],
            ),
        ),
        Chunk::Event(other) => (format!("{other:?}"), String::new()),
    };
    format!("{label:<24} → {outcome}").trim_end().to_string()
}

/// A paste's size and its first characters. A paste rebuilt from keys carries
/// `Enter` as `\r`, which the input box turns into a line break as it does `\n`.
fn paste_outcome(text: &str, loc: &Locale) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    loc.tf(
        "cli.keys.paste",
        &[
            ("chars", &text.chars().count().to_string()),
            ("lines", &(text.matches('\n').count() + 1).to_string()),
            ("preview", &preview(&text)),
        ],
    )
}

/// The key as crossterm reported it: the modifiers in a fixed order, then the
/// code — a character quoted and escaped, so a space or a control character is
/// seen for what it is.
fn key_label(key: &KeyEvent) -> String {
    let mut parts: Vec<String> = modifier_names(key.modifiers)
        .into_iter()
        .map(str::to_string)
        .collect();
    parts.push(match key.code {
        KeyCode::Char(c) => format!("'{}'", c.escape_debug()),
        KeyCode::F(n) => format!("F{n}"),
        other => format!("{other:?}"),
    });
    let mut label = parts.join("+");
    for (name, _) in key.state.iter_names() {
        label.push(' ');
        label.push_str(name);
    }
    label
}

/// The modifiers in a fixed order, by the names the app's help uses.
fn modifier_names(modifiers: KeyModifiers) -> Vec<&'static str> {
    [
        (KeyModifiers::CONTROL, "Ctrl"),
        (KeyModifiers::ALT, "Alt"),
        (KeyModifiers::SHIFT, "Shift"),
        (KeyModifiers::SUPER, "Super"),
        (KeyModifiers::HYPER, "Hyper"),
        (KeyModifiers::META, "Meta"),
    ]
    .into_iter()
    .filter(|(flag, _)| modifiers.contains(*flag))
    .map(|(_, name)| name)
    .collect()
}

/// What the chat's input box does with a key, in the order the chat screen
/// asks (`screens::chat::input`): the line break first, then `Enter`, a
/// `Ctrl` shortcut by its physical letter, and the box itself.
fn key_outcome(key: &KeyEvent, loc: &Locale) -> String {
    let mut field = sample_box();
    if keys::is_line_break(key) {
        field.insert_newline();
        return sketch(&field);
    }
    if key.code == KeyCode::Enter {
        return loc.t("cli.keys.send").to_string();
    }
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && !keys::is_altgr_text(key)
        && let Some(physical) = keys::hotkey_char(key)
    {
        let chord = format!("Ctrl+{}", physical.to_ascii_uppercase());
        return loc.tf("cli.keys.shortcut", &[("chord", &chord)]);
    }
    match field.on_key(*key) {
        KeyOutcome::Ignored => loc.t("cli.keys.ignored").to_string(),
        KeyOutcome::Edited | KeyOutcome::Moved => sketch(&field),
    }
}

/// A mouse event as crossterm reported it, at a 1-based cell.
fn mouse_outcome(mouse: &MouseEvent) -> String {
    let mut text = format!("{:?} at {},{}", mouse.kind, mouse.column + 1, mouse.row + 1);
    for name in modifier_names(mouse.modifiers) {
        text.push(' ');
        text.push_str(name);
    }
    text
}

/// The input box every key starts from: [`SAMPLE`], the cursor at
/// [`SAMPLE_CURSOR`].
fn sample_box() -> InputBox {
    let mut field = InputBox::new();
    field.set_text(SAMPLE);
    let left = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
    for _ in SAMPLE_CURSOR..SAMPLE.chars().count() {
        field.on_key(left);
    }
    field
}

/// The box's text with its cursor `|`, its selection `«…»` and its line
/// breaks `⏎` — what a key did, without a frame to draw it in.
fn sketch(field: &InputBox) -> String {
    let cursor = field.cursor();
    let selection = field.selection_span();
    let mut out = String::new();
    for (row, line) in field.line_strings().iter().enumerate() {
        if row > 0 {
            out.push('⏎');
        }
        let chars: Vec<char> = line.chars().collect();
        for col in 0..=chars.len() {
            let at = (row, col);
            if selection.is_some_and(|(_, end)| end == at) {
                out.push('»');
            }
            if cursor == at {
                out.push('|');
            }
            if selection.is_some_and(|(start, _)| start == at) {
                out.push('«');
            }
            if let Some(&c) = chars.get(col) {
                out.extend(c.escape_debug());
            }
        }
    }
    out
}

/// The first characters of a paste, escaped, a line break as `⏎`.
fn preview(text: &str) -> String {
    const SHOWN: usize = 40;
    let mut out: String = text
        .chars()
        .take(SHOWN)
        .map(|c| {
            if c == '\n' {
                "⏎".to_string()
            } else {
                c.escape_debug().to_string()
            }
        })
        .collect();
    if text.chars().count() > SHOWN {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};
    use ratatui::crossterm::event::{KeyEventState, MouseButton, MouseEventKind};

    fn en() -> &'static Locale {
        locale(Lang::En)
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn outcome(code: KeyCode, modifiers: KeyModifiers) -> String {
        key_outcome(&key(code, modifiers), en())
    }

    #[test]
    fn every_key_starts_between_two_words() {
        assert_eq!(sketch(&sample_box()), "one two |three");
    }

    /// The Mac's word motions — Terminal.app's `ESC b`, iTerm2's `Alt+←` — and
    /// the PC's `Ctrl+←` all land on the same mark; a bare `←` moves one
    /// character, so a word move can't be mistaken for it.
    #[test]
    fn a_word_move_and_a_character_move_land_apart() {
        for (code, modifiers) in [
            (KeyCode::Char('b'), KeyModifiers::ALT),
            (KeyCode::Left, KeyModifiers::ALT),
            (KeyCode::Left, KeyModifiers::CONTROL),
        ] {
            assert_eq!(
                outcome(code, modifiers),
                "one |two three",
                "{code:?} {modifiers:?}"
            );
        }
        assert_eq!(outcome(KeyCode::Left, KeyModifiers::NONE), "one two| three");
        assert_eq!(
            outcome(KeyCode::Char('f'), KeyModifiers::ALT),
            "one two three|"
        );
    }

    #[test]
    fn the_three_line_breaks_break_the_line_and_enter_sends() {
        for (code, modifiers) in [
            (KeyCode::Enter, KeyModifiers::SHIFT),
            (KeyCode::Enter, KeyModifiers::ALT),
            (KeyCode::Char('j'), KeyModifiers::CONTROL),
        ] {
            assert_eq!(
                outcome(code, modifiers),
                "one two ⏎|three",
                "{code:?} {modifiers:?}"
            );
        }
        assert_eq!(
            outcome(KeyCode::Enter, KeyModifiers::NONE),
            "sends the message"
        );
    }

    #[test]
    fn typing_deleting_and_selecting_show_in_the_sketch() {
        assert_eq!(
            outcome(KeyCode::Char('x'), KeyModifiers::NONE),
            "one two x|three"
        );
        assert_eq!(outcome(KeyCode::Backspace, KeyModifiers::ALT), "one |three");
        assert_eq!(
            outcome(KeyCode::Backspace, KeyModifiers::NONE),
            "one two|three"
        );
        assert_eq!(
            outcome(KeyCode::Right, KeyModifiers::SHIFT),
            "one two «t»|hree"
        );
        assert_eq!(
            outcome(KeyCode::Left, KeyModifiers::SHIFT),
            "one two|« »three"
        );
        assert_eq!(outcome(KeyCode::Home, KeyModifiers::NONE), "|one two three");
    }

    /// An `Alt`+letter other than a word motion is not typed — the app's rule
    /// since Option chords used to type their letter — and a `Ctrl` chord is
    /// named by the letter the app matched, whatever the layout typed.
    #[test]
    fn shortcuts_and_untyped_chords_say_so() {
        assert_eq!(
            outcome(KeyCode::Char('x'), KeyModifiers::ALT),
            "the input box does nothing with it"
        );
        assert_eq!(
            outcome(KeyCode::Char('l'), KeyModifiers::CONTROL),
            "the shortcut Ctrl+L"
        );
        assert_eq!(
            outcome(KeyCode::F(1), KeyModifiers::NONE),
            "the input box does nothing with it"
        );
    }

    #[test]
    fn the_label_is_the_event_as_crossterm_gave_it() {
        assert_eq!(
            key_label(&key(KeyCode::Char('b'), KeyModifiers::ALT)),
            "Alt+'b'"
        );
        assert_eq!(
            key_label(&key(
                KeyCode::Enter,
                KeyModifiers::SHIFT | KeyModifiers::CONTROL
            )),
            "Ctrl+Shift+Enter"
        );
        assert_eq!(
            key_label(&key(KeyCode::Char(' '), KeyModifiers::NONE)),
            "' '"
        );
        assert_eq!(
            key_label(&key(KeyCode::Char('\u{1}'), KeyModifiers::NONE)),
            "'\\u{1}'"
        );
        assert_eq!(key_label(&key(KeyCode::F(1), KeyModifiers::NONE)), "F1");
        let mut keypad = key(KeyCode::Left, KeyModifiers::NONE);
        keypad.state = KeyEventState::KEYPAD;
        assert_eq!(key_label(&keypad), "Left KEYPAD");
    }

    fn line(event: Event, control: &Control, mouse: bool) -> String {
        describe(&Chunk::Event(event), control, mouse, en())
    }

    #[test]
    fn quit_and_the_mouse_switch_are_the_apps_own_keys() {
        assert_eq!(
            control_of(&key(KeyCode::Char('q'), KeyModifiers::CONTROL)),
            Control::Quit
        );
        assert_eq!(
            control_of(&key(KeyCode::F(10), KeyModifiers::NONE)),
            Control::Quit
        );
        assert_eq!(
            control_of(&key(KeyCode::Char('w'), KeyModifiers::CONTROL)),
            Control::ToggleMouse
        );
        assert_eq!(
            control_of(&key(KeyCode::Char('q'), KeyModifiers::NONE)),
            Control::Go
        );
        let quit = Event::Key(key(KeyCode::Char('q'), KeyModifiers::CONTROL));
        assert_eq!(
            line(quit, &Control::Quit, false),
            format!("{:<24} → quits", "Ctrl+'q'")
        );
        let switch = Event::Key(key(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert!(line(switch, &Control::ToggleMouse, true).ends_with("mouse capture on"));
    }

    /// The batch is split as the app splits it: a burst of text keys — a
    /// paste on Windows — is one paste, its `Enter`s line breaks, while one key
    /// alone stays a key.
    #[test]
    fn the_batch_is_split_as_the_app_splits_it() {
        let press = |c| Event::Key(key(KeyCode::Char(c), KeyModifiers::NONE));
        let enter = Event::Key(key(KeyCode::Enter, KeyModifiers::NONE));
        let chunks = runtime::chunk_batch(vec![press('h'), press('i'), enter, press('!')]);
        let lines: Vec<String> = chunks
            .iter()
            .map(|chunk| describe(chunk, &Control::Go, false, en()))
            .collect();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("Paste (keys)"), "{lines:?}");
        assert!(
            lines[0].contains("characters: 4, lines: 2: hi⏎!"),
            "{lines:?}"
        );
        let one = runtime::chunk_batch(vec![press('x')]);
        assert_eq!(
            describe(&one[0], &Control::Go, false, en()),
            format!("{:<24} → one two x|three", "'x'")
        );
    }

    #[test]
    fn a_paste_a_click_and_a_resize_are_shown() {
        let paste = line(Event::Paste("hi\nthere".into()), &Control::Go, false);
        assert!(paste.starts_with("Paste "), "{paste}");
        assert!(
            paste.contains("characters: 8, lines: 2: hi⏎there"),
            "{paste}"
        );
        let long = preview(&"x".repeat(50));
        assert_eq!(long.chars().count(), 41);
        assert!(long.ends_with('…'));
        let click = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 4,
            row: 9,
            modifiers: KeyModifiers::SHIFT,
        });
        assert!(line(click, &Control::Go, true).ends_with("Down(Left) at 5,10 Shift"));
        assert!(line(Event::Resize(80, 24), &Control::Go, false).ends_with("80×24"));
    }

    #[test]
    fn the_header_names_the_terminal_and_the_way_out() {
        let lines = header(en(), Some(false));
        let text = lines.join("\n");
        assert!(text.contains(std::env::consts::OS), "{text}");
        assert!(text.contains("one two |three"), "{text}");
        assert!(text.contains("Ctrl+Q"), "{text}");
        assert!(text.contains("not answered"), "{text}");
        assert_eq!(lines.last().map(String::as_str), Some(""));
    }
}

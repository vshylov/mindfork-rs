//! Parses the quit slash-commands in the input box (`/exit`, `/quit`). Pure,
//! testable logic modeled on [`super::compact_command`]: the chat screen calls it
//! on send; a recognized command turns into an intent, while an unrecognized
//! string goes out as a regular message. Error messages are localized in the
//! interface language (axis B, docs/history/i18n-ui.md) — the caller passes its
//! `Locale`.
//!
//! Quitting already has two keys (`Ctrl+Q` and `F10`, spec §11.7) precisely
//! because a terminal may swallow either one; the commands are the third route,
//! and the only one no terminal can intercept — a slash command is ordinary typed
//! text. That is the same reasoning that made `/image paste` exist next to
//! `Ctrl+V` (spec §9.10). VS Code's integrated terminal is the motivating case:
//! it binds `Ctrl+Q` and `F10` to the editor.
//!
//! Two spellings rather than one: `/exit` and `/quit` are the two words every
//! REPL, shell and database client accepts, and a user reaching for a way out
//! types whichever they already know rather than reading a help overlay.

use crate::shared::i18n::Locale;

/// The accepted spellings, in the order the help overlay lists them. Both are
/// exact command words — [`parse`] never matches a longer word that merely starts
/// with one of them (`/exits`, `/quit-now`).
pub const ALIASES: [&str; 2] = ["/exit", "/quit"];

/// Tries to parse an input string as a quit command (`/exit` or `/quit`).
///
/// - `None` — the string is neither: it should be sent as a regular message.
/// - `Some(Ok(()))` — the command is correct (it takes no arguments).
/// - `Some(Err(msg))` — this is a quit command, but with a syntax error (a
///   localized hint in `msg`), so a typo doesn't silently go out to the model as
///   a chat message.
pub fn parse(input: &str, loc: &Locale) -> Option<Result<(), String>> {
    let mut tokens = input.split_whitespace();
    let first = tokens.next()?;
    let cmd = ALIASES.iter().find(|a| first.eq_ignore_ascii_case(a))?;
    // The command takes no arguments. Trailing tokens are reported rather than
    // ignored: as with `/reindex` and `/compact`, there's no subcommand to
    // disambiguate a typo from, so silence would hide the mistake — and here the
    // silence would be read as "the app refused to quit".
    match tokens.next() {
        None => Some(Ok(())),
        Some(arg) => Some(Err(loc.tf("ui.exit.bad_arg", &[("cmd", cmd), ("arg", arg)]))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reference locale (ru) for parse tests — the exact wording isn't asserted
    /// here (see `errors_are_localized_for_all_langs` below), only the Ok/Err shape.
    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    /// Both spellings, with the surrounding whitespace and letter case a real
    /// input box produces. Driven off [`ALIASES`] so a third spelling cannot be
    /// added without being covered here.
    #[test]
    fn every_alias_parses_bare_in_any_case_and_padding() {
        for cmd in ALIASES {
            for text in [
                cmd.to_string(),
                format!("  {cmd}  "),
                format!("\t{cmd}\n"),
                cmd.to_uppercase(),
                format!("  {}  ", cmd.to_uppercase()),
            ] {
                assert_eq!(parse(&text, ru()), Some(Ok(())), "input {text:?}");
            }
        }
    }

    #[test]
    fn trailing_arguments_are_rejected() {
        for cmd in ALIASES {
            for text in [
                format!("{cmd} now"),
                format!("{cmd} --force"),
                format!("  {}  now  ", cmd.to_uppercase()),
            ] {
                assert!(matches!(parse(&text, ru()), Some(Err(_))), "input {text:?}");
            }
        }
    }

    /// The offending argument is named back, and so is the spelling the user
    /// actually typed — a hint that answers with the *other* command word would
    /// read as a different command having been recognized.
    #[test]
    fn the_error_names_the_typed_command_and_the_argument() {
        let Some(Err(msg)) = parse("/quit nooow", ru()) else {
            panic!("expected a syntax error");
        };
        assert!(msg.contains("nooow"), "{msg}");
        assert!(msg.contains("/quit"), "{msg}");
        assert!(
            !msg.contains("/exit"),
            "the untyped spelling leaked in: {msg}"
        );
    }

    #[test]
    fn other_input_is_none() {
        // Neighbouring commands must keep their own meaning.
        assert_eq!(parse("/compact", ru()), None);
        assert_eq!(parse("/reindex", ru()), None);
        assert_eq!(parse("/rag rebuild", ru()), None);
        assert_eq!(parse("/tts stop", ru()), None);
        // A longer word that merely starts with a command name isn't it.
        assert_eq!(parse("/exits", ru()), None);
        assert_eq!(parse("/exit-now", ru()), None);
        assert_eq!(parse("/quitter", ru()), None);
        // Plain text and an empty string go out as regular messages — including
        // the words themselves, which are ordinary things to say to a model.
        assert_eq!(parse("exit", ru()), None);
        assert_eq!(parse("how do I quit vim?", ru()), None);
        assert_eq!(parse("", ru()), None);
        assert_eq!(parse("   ", ru()), None);
    }

    /// Per-locale coverage (i18n gate discipline, docs/history/i18n-ui.md §3.5):
    /// the error path renders under EVERY built-in language with no unsubstituted
    /// `{…}` and, for `en`, with no Cyrillic leaking through from the ru default.
    /// The message must also still name a route out — the key exists so that a
    /// mistyped quit doesn't leave the user without one (docs/lessons.md §4).
    #[test]
    fn errors_are_localized_for_all_langs() {
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            let Some(Err(msg)) = parse("/exit now", loc) else {
                panic!("expected a syntax error in {lang:?}");
            };
            assert!(
                !msg.contains('{') && !msg.contains('}'),
                "unsubstituted placeholder in {lang:?}: {msg}"
            );
            assert!(
                msg.contains("/exit"),
                "the message must name the command in {lang:?}: {msg}"
            );
            assert!(
                msg.contains("Ctrl+Q") && msg.contains("F10"),
                "the message must name the keys that also quit in {lang:?}: {msg}"
            );
            if lang == crate::shared::i18n::Lang::En {
                assert!(
                    !msg.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)),
                    "Cyrillic leaked into the en message: {msg}"
                );
            }
        }
    }
}

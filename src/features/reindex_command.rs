//! Parses the reindex slash-command in the input box (`/reindex`). Pure,
//! testable logic modeled on [`super::rag_command`]: the chat screen calls it on
//! send; a recognized command turns into an intent, while an unrecognized string
//! goes out as a regular message. Error messages are localized in the interface
//! language (axis B, docs/i18n-ui.md) — the caller passes its `Locale`.
//!
//! The command is deliberately top-level, not a `/rag` subcommand: re-embedding
//! spans notes, chat attachments and EVERY profile's knowledge base, so filing it
//! under the `/rag` family would misdescribe its scope (`/rag rebuild` keeps its
//! own meaning — re-chunking one profile after a chunking-parameter change). See
//! docs/research/embedding-model-change-reindex.md §8.1 (decision S4).

use crate::shared::i18n::Locale;

/// Tries to parse an input string as the `/reindex` command.
///
/// - `None` — the string isn't `/reindex`: it should be sent as a regular
///   message.
/// - `Some(Ok(()))` — the command is correct (it takes no arguments).
/// - `Some(Err(msg))` — this is `/reindex`, but with a syntax error (a localized
///   hint in `msg`), so a typo doesn't silently go out to the model as a chat
///   message.
pub fn parse(input: &str, loc: &Locale) -> Option<Result<(), String>> {
    let mut tokens = input.split_whitespace();
    let first = tokens.next()?;
    if !first.eq_ignore_ascii_case("/reindex") {
        return None;
    }
    // The command takes no arguments. Trailing tokens are reported rather than
    // ignored: unlike `/rag list`, there's no subcommand to disambiguate a typo
    // from, so silence would hide the mistake.
    match tokens.next() {
        None => Some(Ok(())),
        Some(_) => Some(Err(loc.tf(
            "ui.reindex.err.unexpected_args",
            &[("usage", loc.t("ui.reindex.usage"))],
        ))),
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

    #[test]
    fn parses_bare_command() {
        assert_eq!(parse("/reindex", ru()), Some(Ok(())));
    }

    #[test]
    fn surrounding_whitespace_is_tolerated() {
        assert_eq!(parse("  /reindex  ", ru()), Some(Ok(())));
        assert_eq!(parse("\t/reindex\n", ru()), Some(Ok(())));
    }

    #[test]
    fn command_is_case_insensitive() {
        assert_eq!(parse("/REINDEX", ru()), Some(Ok(())));
        assert_eq!(parse("  /REINDEX  ", ru()), Some(Ok(())));
        assert_eq!(parse("/Reindex", ru()), Some(Ok(())));
    }

    #[test]
    fn trailing_arguments_are_rejected() {
        assert!(matches!(parse("/reindex now", ru()), Some(Err(_))));
        assert!(matches!(parse("/reindex --all", ru()), Some(Err(_))));
        assert!(matches!(parse("  /Reindex  all  ", ru()), Some(Err(_))));
    }

    #[test]
    fn other_input_is_none() {
        // Neighbouring commands must keep their own meaning.
        assert_eq!(parse("/rag rebuild", ru()), None);
        assert_eq!(parse("/tts stop", ru()), None);
        // A longer word that merely starts with the command name isn't it.
        assert_eq!(parse("/reindexer", ru()), None);
        assert_eq!(parse("/reindex-all", ru()), None);
        // Plain text and an empty string go out as regular messages.
        assert_eq!(parse("reindex the base please", ru()), None);
        assert_eq!(parse("", ru()), None);
        assert_eq!(parse("   ", ru()), None);
    }

    /// Per-locale coverage (i18n gate discipline, docs/history/i18n-ui.md §3.5):
    /// the error path renders under EVERY built-in language with no unsubstituted
    /// `{…}` and, for `en`, with no Cyrillic leaking through from the ru default.
    #[test]
    fn errors_are_localized_for_all_langs() {
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            let Some(Err(msg)) = parse("/reindex now", loc) else {
                panic!("expected a syntax error in {lang:?}");
            };
            assert!(
                !msg.contains('{') && !msg.contains('}'),
                "unsubstituted placeholder in {lang:?}: {msg}"
            );
            assert!(
                msg.contains("/reindex"),
                "the message must name the command in {lang:?}: {msg}"
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

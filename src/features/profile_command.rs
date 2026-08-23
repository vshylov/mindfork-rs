//! Parses the profile slash-command (`/profile list`, `/profile new [name]`,
//! `/profile delete <name>`, `/profile system [text|clear]`,
//! `/profile greeting [text|clear]`). Pure, testable logic modeled on
//! [`super::file_command`]: the chat screen calls it on send; a recognized
//! command turns into an intent, an unrecognized string goes out as a regular
//! message. Error text is localized in the interface language (axis B).
//!
//! **Why a module rather than a row in [`super::ui_command`]'s registry**: this
//! is the one typed route with a subcommand *and* an argument — which is
//! exactly the line that registry draws (`/file`, `/image`, `/rag` and `/tts`
//! sit on this side of it too).
//!
//! **Why it exists at all**: profile CRUD lives only behind `Ctrl+N`/`Ctrl+D`
//! inside the settings screen's "Profiles" section, and both keys are claimed by
//! a browser tab (`Ctrl+N` opens a window) — stage 2 of
//! [docs/history/command-only-control.md](../../docs/history/command-only-control.md),
//! fork F5. A physical-key alternate (`Insert`) was rejected there: a Mac client
//! keyboard has no such key, and a browser terminal is reached from whatever
//! machine the user is sitting at. Stage 3 (docs/history/commands-stage3.md) added the
//! text subcommands for the same reason: the system message and greeting were
//! editable only in the settings editors.

use crate::shared::i18n::Locale;

/// What to do with a free-text field: `Show` (bare — the current value comes
/// back as an editable command line, the `/rename` pattern), `Clear` (the
/// reserved word `clear` — fork F3 of docs/history/commands-stage3.md; the literal
/// text "clear" stays settable in settings), or `Set` the given text. Shared
/// with [`super::impersonation_command`] — the same three-way split, decided
/// once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextEdit {
    /// Bare subcommand: hand the current value back for editing.
    Show,
    /// The reserved word `clear`: remove the value.
    Clear,
    /// Set the value to this text (internal newlines preserved).
    Set(String),
}

/// Classifies a text subcommand's raw argument. The remainder is taken raw
/// (trimmed at the edges only), so a multi-line system message survives; the
/// name-shaped subcommands normalize instead ([`name_argument`]).
pub(crate) fn text_edit(raw: &str) -> TextEdit {
    if raw.is_empty() {
        TextEdit::Show
    } else if raw.eq_ignore_ascii_case("clear") {
        TextEdit::Clear
    } else {
        TextEdit::Set(raw.to_string())
    }
}

/// Normalizes a name-shaped argument: whitespace runs collapse to single
/// spaces (a name has no meaningful newlines), surrounding quotes come off,
/// and an empty result reads as "no argument".
pub(crate) fn name_argument(raw: &str) -> Option<String> {
    let joined = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = joined.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Splits a command line into its head word and the raw remainder (edge-trimmed,
/// internal whitespace — newlines included — preserved). The head match is what
/// decides whether a parser claims the line at all.
pub(crate) fn head_and_rest(input: &str) -> Option<(&str, &str)> {
    let trimmed = input.trim();
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let head = parts.next().filter(|h| !h.is_empty())?;
    Some((head, parts.next().unwrap_or("").trim()))
}

/// The `"/family sub rest"` prologue this parser and its sibling
/// (`impersonation_command`) share — the budget-for-the-seam rule
/// (docs/lessons.md §2): `None` when the line is not this family's, otherwise
/// the subcommand word and its raw argument (`None` inside — the bare family
/// word, whose "needs a subcommand" message the caller owes).
pub(crate) fn subcommand_of<'a>(
    input: &'a str,
    family: &str,
) -> Option<Option<(&'a str, &'a str)>> {
    let (first, rest) = head_and_rest(input)?;
    if !first.eq_ignore_ascii_case(family) {
        return None;
    }
    Some(head_and_rest(rest))
}

/// A recognized `/profile` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileCommand {
    /// Show the assistant profiles (`/profile list`).
    List,
    /// Create a profile; `None` — the default name the settings screen's
    /// `Ctrl+N` uses.
    New { name: Option<String> },
    /// Delete a profile by name (an exact match, or an unambiguous prefix).
    Delete { name: String },
    /// The active chat's profile's system message (`/profile system [text|clear]`).
    /// Applies to new conversations — a chat snapshots it at creation (spec §5.1).
    System(TextEdit),
    /// The active chat's profile's greeting (`/profile greeting [text|clear]`).
    /// Applies to new conversations — the greeting is inserted at creation.
    Greeting(TextEdit),
}

/// Tries to parse an input string as a `/profile` command.
///
/// - `None` — not a `/profile` command: send it as a regular message.
/// - `Some(Ok(cmd))` — a correct command.
/// - `Some(Err(msg))` — a `/profile` command with a syntax error (a localized
///   hint naming the usage line).
pub fn parse(input: &str, loc: &Locale) -> Option<Result<ProfileCommand, String>> {
    let parts = subcommand_of(input, "/profile")?;
    let usage = loc.t("ui.profile.usage");
    let Some((sub, raw)) = parts else {
        return Some(Err(
            loc.tf("ui.profile.err.missing_subcommand", &[("usage", usage)])
        ));
    };

    if sub.eq_ignore_ascii_case("list") {
        Some(Ok(ProfileCommand::List))
    } else if sub.eq_ignore_ascii_case("new") {
        Some(Ok(ProfileCommand::New {
            name: name_argument(raw),
        }))
    } else if sub.eq_ignore_ascii_case("delete") {
        // `delete`, not `remove`: this one really does remove the companion and
        // hides its conversations with it, and the wording should not soften
        // that. (`/file remove` made the opposite call for the opposite reason —
        // it does *not* delete anything from disk.)
        match name_argument(raw) {
            Some(name) => Some(Ok(ProfileCommand::Delete { name })),
            None => Some(Err(
                loc.tf("ui.profile.err.missing_name", &[("usage", usage)])
            )),
        }
    } else if sub.eq_ignore_ascii_case("system") {
        Some(Ok(ProfileCommand::System(text_edit(raw))))
    } else if sub.eq_ignore_ascii_case("greeting") {
        Some(Ok(ProfileCommand::Greeting(text_edit(raw))))
    } else {
        Some(Err(loc.tf(
            "ui.profile.err.unknown_subcommand",
            &[("sub", sub), ("usage", usage)],
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};

    fn ru() -> &'static Locale {
        locale(Lang::Ru)
    }

    #[test]
    fn subcommands_parse_in_any_case_and_padding() {
        for (text, expected) in [
            ("/profile list", ProfileCommand::List),
            ("  /PROFILE  List  ", ProfileCommand::List),
            ("/profile new", ProfileCommand::New { name: None }),
            (
                "/profile new Гайя",
                ProfileCommand::New {
                    name: Some("Гайя".into()),
                },
            ),
            (
                "/profile delete Гайя",
                ProfileCommand::Delete {
                    name: "Гайя".into(),
                },
            ),
            ("/profile system", ProfileCommand::System(TextEdit::Show)),
            (
                "/profile SYSTEM CLEAR",
                ProfileCommand::System(TextEdit::Clear),
            ),
            (
                "/profile system Ты — Гайя.",
                ProfileCommand::System(TextEdit::Set("Ты — Гайя.".into())),
            ),
            (
                "/profile greeting",
                ProfileCommand::Greeting(TextEdit::Show),
            ),
            (
                "/profile greeting clear",
                ProfileCommand::Greeting(TextEdit::Clear),
            ),
            (
                "/profile greeting Привет!",
                ProfileCommand::Greeting(TextEdit::Set("Привет!".into())),
            ),
        ] {
            assert_eq!(parse(text, ru()), Some(Ok(expected)), "input {text:?}");
        }
    }

    /// A name is prose: spaces stay, surrounding quotes come off. Splitting on
    /// the first space would quietly create a profile named after its first word
    /// alone.
    #[test]
    fn a_name_keeps_its_spaces_and_loses_its_quotes() {
        for text in [
            "/profile new Дневной помощник",
            "/profile new \"Дневной помощник\"",
        ] {
            assert_eq!(
                parse(text, ru()),
                Some(Ok(ProfileCommand::New {
                    name: Some("Дневной помощник".into())
                })),
                "input {text:?}"
            );
        }
    }

    /// A system message is a document, not a name: internal newlines survive
    /// (the whole point of setting it from the multi-line box), and quotes are
    /// content rather than wrapping.
    #[test]
    fn a_text_argument_keeps_newlines_and_quotes() {
        let text = "/profile system Ты — «Гайя».\nОтвечай кратко.";
        assert_eq!(
            parse(text, ru()),
            Some(Ok(ProfileCommand::System(TextEdit::Set(
                "Ты — «Гайя».\nОтвечай кратко.".into()
            ))))
        );
        // The word `clear` only clears when it is the whole argument.
        assert_eq!(
            parse("/profile greeting clear the air", ru()),
            Some(Ok(ProfileCommand::Greeting(TextEdit::Set(
                "clear the air".into()
            ))))
        );
    }

    #[test]
    fn a_missing_or_unknown_subcommand_is_reported() {
        for text in ["/profile", "/profile rename Гайя", "/profile delete"] {
            assert!(matches!(parse(text, ru()), Some(Err(_))), "input {text:?}");
        }
        // The unknown subcommand is quoted back, so a typo is visible.
        let Some(Err(msg)) = parse("/profile renam Гайя", ru()) else {
            panic!("expected a report");
        };
        assert!(msg.contains("renam"), "{msg}");
    }

    #[test]
    fn other_input_is_none() {
        for text in [
            "/profiles list",
            "/profile-new",
            "/new Гайя",
            "profile list",
            "how do I add a /profile?",
            "",
        ] {
            assert_eq!(parse(text, ru()), None, "input {text:?}");
        }
    }

    /// Per-locale gate (docs/history/i18n-ui.md §3.5): every error renders in
    /// every bundled language with no unsubstituted placeholder, names the usage
    /// line (a route that works, docs/lessons.md §4), and the `en` text carries
    /// no Cyrillic from the `ru` default.
    #[test]
    fn errors_are_localized_for_all_langs() {
        for &lang in Lang::ALL {
            let loc = locale(lang);
            for text in ["/profile", "/profile delete", "/profile renam x"] {
                let Some(Err(msg)) = parse(text, loc) else {
                    panic!("{text:?} should have been reported in {lang:?}");
                };
                assert!(
                    !msg.contains('{') && !msg.contains('}'),
                    "unsubstituted placeholder in {lang:?}: {msg}"
                );
                assert!(
                    msg.contains("/profile"),
                    "the message must name the command in {lang:?}: {msg}"
                );
                if lang == Lang::En {
                    assert!(
                        !msg.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)),
                        "Cyrillic leaked into the en message: {msg}"
                    );
                }
            }
        }
    }
}

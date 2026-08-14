//! Parses the profile slash-command (`/profile list`, `/profile new [name]`,
//! `/profile delete <name>`). Pure, testable logic modeled on
//! [`super::file_command`]: the chat screen calls it on send; a recognized
//! command turns into an intent, an unrecognized string goes out as a regular
//! message. Error text is localized in the interface language (axis B).
//!
//! **Why a module rather than a row in [`super::ui_command`]'s registry**: this
//! is the one typed route with real syntax — a subcommand *plus* a free-text
//! argument — which is exactly the line that registry draws (`/file`, `/image`,
//! `/rag` and `/tts` sit on this side of it too).
//!
//! **Why it exists at all**: profile CRUD lives only behind `Ctrl+N`/`Ctrl+D`
//! inside the settings screen's "Profiles" section, and both keys are claimed by
//! a browser tab (`Ctrl+N` opens a window) — stage 2 of
//! [docs/history/command-only-control.md](../../docs/history/command-only-control.md),
//! fork F5. A physical-key alternate (`Insert`) was rejected there: a Mac client
//! keyboard has no such key, and a browser terminal is reached from whatever
//! machine the user is sitting at.

use crate::shared::i18n::Locale;

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
}

/// Tries to parse an input string as a `/profile` command.
///
/// - `None` — not a `/profile` command: send it as a regular message.
/// - `Some(Ok(cmd))` — a correct command.
/// - `Some(Err(msg))` — a `/profile` command with a syntax error (a localized
///   hint naming the usage line).
pub fn parse(input: &str, loc: &Locale) -> Option<Result<ProfileCommand, String>> {
    let mut tokens = input.split_whitespace();
    let first = tokens.next()?;
    if !first.eq_ignore_ascii_case("/profile") {
        return None;
    }
    let usage = loc.t("ui.profile.usage");
    let Some(sub) = tokens.next() else {
        return Some(Err(
            loc.tf("ui.profile.err.missing_subcommand", &[("usage", usage)])
        ));
    };
    let rest: Vec<&str> = tokens.collect();
    let argument = {
        let joined = rest.join(" ");
        let trimmed = joined.trim().trim_matches(|c| c == '"' || c == '\'').trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    };

    if sub.eq_ignore_ascii_case("list") {
        Some(Ok(ProfileCommand::List))
    } else if sub.eq_ignore_ascii_case("new") {
        Some(Ok(ProfileCommand::New { name: argument }))
    } else if sub.eq_ignore_ascii_case("delete") {
        // `delete`, not `remove`: this one really does remove the companion and
        // hides its conversations with it, and the wording should not soften
        // that. (`/file remove` made the opposite call for the opposite reason —
        // it does *not* delete anything from disk.)
        match argument {
            Some(name) => Some(Ok(ProfileCommand::Delete { name })),
            None => Some(Err(
                loc.tf("ui.profile.err.missing_name", &[("usage", usage)])
            )),
        }
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

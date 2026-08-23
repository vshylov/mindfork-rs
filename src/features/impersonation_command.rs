//! Parses the impersonation-profile slash-command (`/impersonation list`,
//! `/impersonation new [name]`, `/impersonation delete <name>`,
//! `/impersonation use <name|default>`, `/impersonation system [text|clear]`).
//! Pure, testable logic modeled on [`super::profile_command`] — the personas
//! (spec §11.8, `AppConfig.impersonation_profiles`) are the *user* side of what
//! that module manages, and their CRUD lives behind the same browser-taken
//! `Ctrl+N`/`Ctrl+D` in the settings screen. Stage 3 of the command-only
//! track (docs/history/commands-stage3.md; the word is the user's F2 decision —
//! the settings subsection's own name over the shorter noun).
//!
//! The exact-word match keeps `/impersonation` apart from the *action*
//! `/impersonate` (the registry's row): neither is a prefix of the other to
//! the parser, which compares whole words.

use crate::shared::i18n::Locale;

use super::profile_command::{TextEdit, name_argument, subcommand_of, text_edit};

/// A recognized `/impersonation` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImpersonationCommand {
    /// Show the impersonation profiles (`/impersonation list`).
    List,
    /// Create a persona; `None` — the default name the settings screen's
    /// `Ctrl+N` uses. Creating does **not** link it (fork F4): several
    /// assistant profiles can share one persona, and selection is its own step.
    New { name: Option<String> },
    /// Delete a persona by name (exact match, or an unambiguous prefix).
    Delete { name: String },
    /// Link a persona to the active chat's profile.
    Use { name: String },
    /// Unlink (`/impersonation use default`): impersonation falls back to the
    /// shared default text — the settings choice's "not set" option.
    UseDefault,
    /// The linked persona's system message (`/impersonation system [text|clear]`).
    System(TextEdit),
}

/// Tries to parse an input string as an `/impersonation` command.
///
/// - `None` — not an `/impersonation` command: send it as a regular message.
/// - `Some(Ok(cmd))` — a correct command.
/// - `Some(Err(msg))` — the right word with a syntax error (a localized hint
///   naming the usage line).
pub fn parse(input: &str, loc: &Locale) -> Option<Result<ImpersonationCommand, String>> {
    let parts = subcommand_of(input, "/impersonation")?;
    let usage = loc.t("ui.imp.usage");
    let Some((sub, raw)) = parts else {
        return Some(Err(
            loc.tf("ui.imp.err.missing_subcommand", &[("usage", usage)])
        ));
    };

    if sub.eq_ignore_ascii_case("list") {
        Some(Ok(ImpersonationCommand::List))
    } else if sub.eq_ignore_ascii_case("new") {
        Some(Ok(ImpersonationCommand::New {
            name: name_argument(raw),
        }))
    } else if sub.eq_ignore_ascii_case("delete") {
        match name_argument(raw) {
            Some(name) => Some(Ok(ImpersonationCommand::Delete { name })),
            None => Some(Err(loc.tf(
                "ui.imp.err.missing_name",
                &[("sub", "delete"), ("usage", usage)],
            ))),
        }
    } else if sub.eq_ignore_ascii_case("use") {
        // `default` is reserved (documented in the usage line): a persona
        // literally named "default" stays selectable in settings.
        match name_argument(raw) {
            Some(name) if name.eq_ignore_ascii_case("default") => {
                Some(Ok(ImpersonationCommand::UseDefault))
            }
            Some(name) => Some(Ok(ImpersonationCommand::Use { name })),
            None => Some(Err(loc.tf(
                "ui.imp.err.missing_name",
                &[("sub", "use"), ("usage", usage)],
            ))),
        }
    } else if sub.eq_ignore_ascii_case("system") {
        Some(Ok(ImpersonationCommand::System(text_edit(raw))))
    } else {
        Some(Err(loc.tf(
            "ui.imp.err.unknown_subcommand",
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
            ("/impersonation list", ImpersonationCommand::List),
            ("  /IMPERSONATION  List  ", ImpersonationCommand::List),
            (
                "/impersonation new",
                ImpersonationCommand::New { name: None },
            ),
            (
                "/impersonation new Владимир",
                ImpersonationCommand::New {
                    name: Some("Владимир".into()),
                },
            ),
            (
                "/impersonation delete Владимир",
                ImpersonationCommand::Delete {
                    name: "Владимир".into(),
                },
            ),
            (
                "/impersonation use Владимир",
                ImpersonationCommand::Use {
                    name: "Владимир".into(),
                },
            ),
            (
                "/impersonation use DEFAULT",
                ImpersonationCommand::UseDefault,
            ),
            (
                "/impersonation system",
                ImpersonationCommand::System(TextEdit::Show),
            ),
            (
                "/impersonation system clear",
                ImpersonationCommand::System(TextEdit::Clear),
            ),
            (
                "/impersonation system Ты — Владимир.\nПиши кратко.",
                ImpersonationCommand::System(TextEdit::Set("Ты — Владимир.\nПиши кратко.".into())),
            ),
        ] {
            assert_eq!(parse(text, ru()), Some(Ok(expected)), "input {text:?}");
        }
    }

    #[test]
    fn a_missing_or_unknown_subcommand_is_reported() {
        for text in [
            "/impersonation",
            "/impersonation delete",
            "/impersonation use",
            "/impersonation rename Вова",
        ] {
            assert!(matches!(parse(text, ru()), Some(Err(_))), "input {text:?}");
        }
        let Some(Err(msg)) = parse("/impersonation renam Вова", ru()) else {
            panic!("expected a report");
        };
        assert!(msg.contains("renam"), "{msg}");
    }

    /// The action command and near-words stay out: `/impersonate` belongs to
    /// the registry, longer words are prose, and the module must not shadow
    /// either (the F2 near-miss risk, accepted with the user's word choice).
    #[test]
    fn other_input_is_none() {
        for text in [
            "/impersonate",
            "/impersonate Вова",
            "/impersonations list",
            "/impersonation-new",
            "impersonation list",
            "what does /impersonation do?",
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
            for text in [
                "/impersonation",
                "/impersonation delete",
                "/impersonation use",
                "/impersonation renam x",
            ] {
                let Some(Err(msg)) = parse(text, loc) else {
                    panic!("{text:?} should have been reported in {lang:?}");
                };
                assert!(
                    !msg.contains('{') && !msg.contains('}'),
                    "unsubstituted placeholder in {lang:?}: {msg}"
                );
                assert!(
                    msg.contains("/impersonation"),
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

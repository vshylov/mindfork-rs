//! Parses the code-workspace slash-command in the input box
//! (`/project attach <dir>`, `/project detach`, `/project status`). Pure,
//! testable logic: the chat screen calls it on send; a recognized command turns
//! into an intent, an unrecognized string goes out as an ordinary message.
//! Error messages are localized in the interface language (axis B) — the caller
//! passes its `Locale`.
//!
//! It has a module of its own rather than a row in `features/ui_command.rs`
//! because it takes a free-form path: the registry there models closed
//! vocabularies, and a directory may contain spaces (the `/file`, `/image` and
//! `/export` precedent).
//!
//! [`ProjectProgress`] is the command-outcome type, defined here in `features`
//! so both `app` (emits the events) and `screens` (renders the notes) can use it
//! without breaking FSD's dependency direction — the arrangement `FileProgress`
//! already uses.
//!
//! See docs/code-workspace.md, spec §9.12.

use crate::shared::i18n::Locale;

/// A recognized `/project` command.
#[derive(Debug, Clone, PartialEq)]
pub enum ProjectCommand {
    /// Attach a project directory to the current chat.
    Attach { path: String },
    /// Detach the project from the current chat.
    Detach,
    /// Report what is attached to the current chat.
    Status,
}

/// Outcome of a `/project` command, for the feed note.
#[derive(Debug, Clone, PartialEq)]
pub enum ProjectProgress {
    /// A project was attached: the canonical root and its last component.
    Attached { root: String, name: String },
    /// The project was detached (`root` — what it had been).
    Detached { root: String },
    /// `/project status`: the attached root, or `None` for "nothing attached".
    Status { root: Option<String> },
    /// The command failed (no such directory, not a directory, no active chat…).
    Failed(String),
}

/// Tries to parse an input string as a `/project` command.
///
/// - `None` — not a `/project` command: send it as an ordinary message.
/// - `Some(Ok(cmd))` — a correct command.
/// - `Some(Err(msg))` — a `/project` command with a syntax error (a localized
///   hint in `msg`, naming the usage that works).
pub fn parse(input: &str, loc: &Locale) -> Option<Result<ProjectCommand, String>> {
    let mut tokens = input.split_whitespace();
    let first = tokens.next()?;
    if !first.eq_ignore_ascii_case("/project") {
        return None;
    }
    let Some(sub) = tokens.next() else {
        return Some(Err(loc.tf(
            "ui.project.err.missing_subcommand",
            &[("usage", loc.t("ui.project.usage"))],
        )));
    };
    let rest: Vec<&str> = tokens.collect();

    if sub.eq_ignore_ascii_case("attach") {
        match argument(&rest) {
            Some(path) => Some(Ok(ProjectCommand::Attach { path })),
            None => Some(Err(loc.tf(
                "ui.project.err.missing_path",
                &[("usage", loc.t("ui.project.usage"))],
            ))),
        }
    } else if sub.eq_ignore_ascii_case("detach") {
        Some(Ok(ProjectCommand::Detach))
    } else if sub.eq_ignore_ascii_case("status") {
        Some(Ok(ProjectCommand::Status))
    } else {
        Some(Err(loc.tf(
            "ui.project.err.unknown_subcommand",
            &[("sub", sub), ("usage", loc.t("ui.project.usage"))],
        )))
    }
}

/// Joins the tokens after the subcommand into one argument: a path may contain
/// spaces, and surrounding quotes are stripped (a shell habit, and what a user
/// gets from "copy as path" on Windows).
fn argument(tokens: &[&str]) -> Option<String> {
    let joined = tokens.join(" ");
    let arg = joined.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    (!arg.is_empty()).then(|| arg.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reference locale (ru) for parse tests — the wording is not asserted
    /// here (that is `errors_name_a_route_in_every_language`), only the shape.
    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    #[test]
    fn parses_the_three_subcommands() {
        assert_eq!(
            parse("/project attach D:/proj", ru()).unwrap().unwrap(),
            ProjectCommand::Attach {
                path: "D:/proj".into()
            }
        );
        assert_eq!(
            parse("/project detach", ru()).unwrap().unwrap(),
            ProjectCommand::Detach
        );
        assert_eq!(
            parse("/project status", ru()).unwrap().unwrap(),
            ProjectCommand::Status
        );
    }

    /// A path with spaces is the common case on Windows, and "copy as path"
    /// wraps it in quotes.
    #[test]
    fn a_path_may_contain_spaces_and_quotes() {
        let cmd = parse(r#"/project attach "C:\My Projects\app""#, ru())
            .unwrap()
            .unwrap();
        assert_eq!(
            cmd,
            ProjectCommand::Attach {
                path: r"C:\My Projects\app".into()
            }
        );
    }

    #[test]
    fn a_non_command_is_left_alone() {
        assert!(parse("project attach x", ru()).is_none());
        assert!(parse("tell me about /project", ru()).is_none());
        // A longer word that merely starts the same must not be swallowed.
        assert!(parse("/projects", ru()).is_none());
    }

    #[test]
    fn subcommands_are_case_insensitive() {
        assert_eq!(
            parse("/PROJECT Detach", ru()).unwrap().unwrap(),
            ProjectCommand::Detach
        );
    }

    /// Every refusal must name the route that works, or the user is left
    /// guessing at a command they already tried (docs/lessons.md §4).
    #[test]
    fn errors_name_a_route_in_every_language() {
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            let usage = loc.t("ui.project.usage");
            for input in ["/project", "/project attach", "/project frobnicate"] {
                let Some(Err(msg)) = parse(input, loc) else {
                    panic!("{lang:?}: {input} must be a localized refusal");
                };
                assert!(
                    msg.contains(usage),
                    "{lang:?}: {input} must name the usage: {msg}"
                );
                assert!(
                    !msg.contains('{'),
                    "{lang:?}: unsubstituted placeholder in {msg}"
                );
            }
        }
    }
}

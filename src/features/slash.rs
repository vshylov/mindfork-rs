//! The shared head of a slash command that carries a free-form argument
//! (`/project attach <dir>`, `/file attach <path>`, `/image attach <path|url>`,
//! `/rag add <path>`).
//!
//! Such commands cannot be rows in the declarative registry
//! ([`super::ui_command`]): that table models closed vocabularies, while these
//! take a path, which may contain spaces and may be quoted. So each has a parser
//! of its own — and every one of them opens by splitting the same way, checking
//! the same first token and joining the same tail. Written out per command, that
//! is a *sliding self-duplicate*: the same tokens with different literals, which
//! is exactly the shape the duplication gate measures and the shape
//! docs/lessons.md §2 records four times over.
//!
//! `/project` is written against this seam. The three parsers that predate it
//! are deliberately **not** rewritten here — a mechanical refactor and a feature
//! do not share a PR (AGENTS.md §2) — but they are its obvious next callers.

/// What a slash command's head parsed to.
#[derive(Debug, Clone, PartialEq)]
pub enum Head<'a> {
    /// `/name` alone — the caller answers with its usage.
    Bare,
    /// `/name <sub> [rest…]`.
    Sub { sub: &'a str, rest: Vec<&'a str> },
}

/// Splits `input` as `/<name> …`, case-insensitively.
///
/// `None` means the string is not this command at all and should be sent as an
/// ordinary message — including for a longer word that merely starts the same
/// way (`/projects` is not `/project`), since the first token is compared whole.
pub fn head<'a>(input: &'a str, name: &str) -> Option<Head<'a>> {
    let mut tokens = input.split_whitespace();
    let first = tokens.next()?;
    let expected = format!("/{name}");
    if !first.eq_ignore_ascii_case(&expected) {
        return None;
    }
    match tokens.next() {
        None => Some(Head::Bare),
        Some(sub) => Some(Head::Sub {
            sub,
            rest: tokens.collect(),
        }),
    }
}

/// Joins the tokens after the subcommand into one argument: a path may contain
/// spaces, and surrounding quotes are stripped (a shell habit, and what "copy as
/// path" gives on Windows). `None` — nothing left.
pub fn argument(tokens: &[&str]) -> Option<String> {
    let joined = tokens.join(" ");
    let arg = joined.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    (!arg.is_empty()).then(|| arg.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_the_command_and_its_subcommand() {
        assert_eq!(head("/project", "project"), Some(Head::Bare));
        assert_eq!(
            head("/PROJECT Attach D:/x", "project"),
            Some(Head::Sub {
                sub: "Attach",
                rest: vec!["D:/x"]
            })
        );
    }

    /// The first token is compared whole, so a longer word starting the same way
    /// is an ordinary message rather than a mangled command.
    #[test]
    fn a_longer_word_is_not_the_command() {
        assert_eq!(head("/projects", "project"), None);
        assert_eq!(head("project attach x", "project"), None);
        assert_eq!(head("tell me about /project", "project"), None);
    }

    #[test]
    fn an_argument_keeps_spaces_and_loses_quotes() {
        assert_eq!(
            argument(&["\"C:\\My", "Projects\\app\""]),
            Some("C:\\My Projects\\app".to_string())
        );
        assert_eq!(argument(&[]), None);
        assert_eq!(argument(&["  "]), None);
    }
}

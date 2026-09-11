//! Parses chat file-attachment slash-commands in the input box
//! (`/file attach <path>`, `/file remove <name|#N>`, `/file list`). Pure,
//! testable logic: the chat screen calls it on send; a recognized command turns
//! into an intent, while an unrecognized string goes out as a regular message.
//! Error messages are localized in the interface language (axis B) — the caller
//! passes its `Locale`.
//!
//! [`FileProgress`] also lives here — the command-outcome type, defined in the
//! `features` layer so both `app` (emits events) and `screens` (renders notes)
//! can use it without breaking FSD's dependency direction (the same arrangement
//! as [`RagProgress`](super::rag_ingest::RagProgress)).
//!
//! See docs/file-attachments.md, spec §9.7.

use crate::entities::attachment::{AttachMode, AttachmentInfo};
use crate::shared::i18n::Locale;

/// A recognized `/file` command.
#[derive(Debug, Clone, PartialEq)]
pub enum FileCommand {
    /// Attach a file to the current chat.
    Attach { path: String },
    /// Remove an attachment by display name, path, or `#N` position.
    Remove { target: String },
    /// Show what is attached to the current chat.
    List,
}

/// Outcome of a `/file` command, for the feed note / status.
#[derive(Debug, Clone, PartialEq)]
pub enum FileProgress {
    /// A file was attached: its card + the chat's new inline total.
    Attached {
        info: AttachmentInfo,
        total_tokens: usize,
        /// The encoding the file was read in, when it was not UTF-8 — the note names it
        /// (docs/research/local-file-encoding.md F4b).
        read_as: Option<&'static str>,
    },
    /// An attachment was removed.
    Removed { name: String },
    /// The chat's attachment list (`/file list`); empty — nothing attached.
    Listed { items: Vec<AttachmentInfo> },
    /// Building the semantic index over a by-reference file is under way
    /// (a banner with a spinner; `done`/`total` are chunks).
    Indexing {
        name: String,
        done: usize,
        total: usize,
    },
    /// The semantic index for a file is ready — `attachment_search` can reach it.
    Indexed { name: String, chunks: usize },
    /// No index was built (no embedder configured, or a write failed). Not an
    /// error: the pinned block and `attachment_read` keep working in full.
    IndexSkipped { name: String, reason: String },
    /// The command failed (no such file, undecodable content, no active chat…).
    Failed(String),
}

/// Tries to parse an input string as a `/file` command.
///
/// - `None` — the string isn't a `/file` command (doesn't start with `/file`):
///   it should be sent as a regular message.
/// - `Some(Ok(cmd))` — a correct command.
/// - `Some(Err(msg))` — a `/file` command with a syntax error (a localized hint
///   in `msg`).
pub fn parse(input: &str, loc: &Locale) -> Option<Result<FileCommand, String>> {
    let mut tokens = input.split_whitespace();
    let first = tokens.next()?;
    if !first.eq_ignore_ascii_case("/file") {
        return None;
    }
    let Some(sub) = tokens.next() else {
        return Some(Err(loc.tf(
            "ui.file.err.missing_subcommand",
            &[("usage", loc.t("ui.file.usage"))],
        )));
    };
    let rest: Vec<&str> = tokens.collect();

    if sub.eq_ignore_ascii_case("attach") {
        match argument(&rest) {
            Some(path) => Some(Ok(FileCommand::Attach { path })),
            None => Some(Err(loc.tf(
                "ui.file.err.missing_path",
                &[("usage", loc.t("ui.file.usage"))],
            ))),
        }
    } else if sub.eq_ignore_ascii_case("remove") {
        // Only `remove`, never `delete` — the same wording decision RAG made
        // after users worried a command would delete the file from disk.
        match argument(&rest) {
            Some(target) => Some(Ok(FileCommand::Remove { target })),
            None => Some(Err(loc.tf(
                "ui.file.err.missing_target",
                &[("usage", loc.t("ui.file.usage"))],
            ))),
        }
    } else if sub.eq_ignore_ascii_case("list") {
        Some(Ok(FileCommand::List))
    } else {
        Some(Err(loc.tf(
            "ui.file.err.unknown_subcommand",
            &[("sub", sub), ("usage", loc.t("ui.file.usage"))],
        )))
    }
}

/// Joins the tokens after the subcommand into one argument: a path or name may
/// contain spaces, and surrounding quotes are stripped. `None` — nothing left.
fn argument(tokens: &[&str]) -> Option<String> {
    let joined = tokens.join(" ");
    let arg = joined.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    (!arg.is_empty()).then(|| arg.to_string())
}

/// Resolves a `/file remove` target to a position in the chat's attachment list:
/// `#N` (1-based, as shown by `/file list`) or a name/path match. Pure — the
/// orchestrator applies the result.
pub fn resolve_target(
    items: &[crate::entities::attachment::Attachment],
    target: &str,
) -> Option<usize> {
    let t = target.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    if let Some(digits) = t.strip_prefix('#')
        && let Ok(n) = digits.trim().parse::<usize>()
    {
        // `then` (lazy), not `then_some`: the latter evaluates `n - 1` eagerly and
        // underflows on `#0`.
        return (n >= 1 && n <= items.len()).then(|| n - 1);
    }
    items.iter().position(|a| a.matches(t))
}

/// A short human label for the attachment's mode (localized).
pub fn mode_label(mode: AttachMode, loc: &'static Locale) -> &'static str {
    match mode {
        AttachMode::Inline => loc.t("ui.file.mode.inline"),
        AttachMode::ByReference => loc.t("ui.file.mode.by_reference"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::attachment::Attachment;

    /// A reference locale (ru) for parse tests — the exact wording isn't
    /// asserted here (see `errors_are_localized_for_all_langs`), only the
    /// Ok/Err shape.
    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    fn attach(path: &str) -> Option<Result<FileCommand, String>> {
        Some(Ok(FileCommand::Attach { path: path.into() }))
    }

    #[test]
    fn non_file_input_is_none() {
        assert_eq!(parse("regular message", ru()), None);
        assert_eq!(parse("/rag add x", ru()), None);
        assert_eq!(parse("", ru()), None);
    }

    #[test]
    fn parses_attach_with_path() {
        assert_eq!(
            parse("/file attach d:\\dir\\notes.md", ru()),
            attach("d:\\dir\\notes.md")
        );
    }

    #[test]
    fn path_with_spaces_and_quotes() {
        assert_eq!(
            parse("/file attach C:\\Program Files\\a.txt", ru()),
            attach("C:\\Program Files\\a.txt")
        );
        assert_eq!(
            parse("/file attach \"d:\\my docs\\a.txt\"", ru()),
            attach("d:\\my docs\\a.txt")
        );
    }

    #[test]
    fn command_is_case_insensitive() {
        assert_eq!(parse("/FILE ATTACH d:\\x.txt", ru()), attach("d:\\x.txt"));
        assert_eq!(parse("/File List", ru()), Some(Ok(FileCommand::List)));
    }

    #[test]
    fn parses_remove_and_list() {
        assert_eq!(
            parse("/file remove notes.md", ru()),
            Some(Ok(FileCommand::Remove {
                target: "notes.md".into()
            }))
        );
        assert_eq!(
            parse("/file remove #2", ru()),
            Some(Ok(FileCommand::Remove {
                target: "#2".into()
            }))
        );
        assert_eq!(parse("/file list", ru()), Some(Ok(FileCommand::List)));
        // Extra tokens after `list` are ignored (it takes no argument).
        assert_eq!(
            parse("/file list everything", ru()),
            Some(Ok(FileCommand::List))
        );
    }

    #[test]
    fn delete_is_not_a_command() {
        // `delete` is deliberately unsupported (see the parser's comment).
        assert!(matches!(parse("/file delete a.txt", ru()), Some(Err(_))));
    }

    #[test]
    fn errors_on_missing_parts() {
        assert!(matches!(parse("/file", ru()), Some(Err(_))));
        assert!(matches!(parse("/file attach", ru()), Some(Err(_))));
        assert!(matches!(parse("/file remove", ru()), Some(Err(_))));
        assert!(matches!(parse("/file purge x", ru()), Some(Err(_))));
    }

    #[test]
    fn resolve_target_by_index_name_and_path() {
        let items = vec![
            Attachment::new("a.txt", "/tmp/a.txt", "x".into(), 1, AttachMode::Inline),
            Attachment::new("b.md", "/tmp/b.md", "y".into(), 1, AttachMode::Inline),
        ];
        assert_eq!(resolve_target(&items, "#1"), Some(0));
        assert_eq!(resolve_target(&items, "#2"), Some(1));
        assert_eq!(resolve_target(&items, "b.md"), Some(1));
        assert_eq!(resolve_target(&items, "/tmp/a.txt"), Some(0));
        // Out of range / unknown → None (the caller reports it).
        assert_eq!(resolve_target(&items, "#0"), None);
        assert_eq!(resolve_target(&items, "#9"), None);
        assert_eq!(resolve_target(&items, "missing.txt"), None);
        // `#` followed by a non-number falls through to a name match.
        assert_eq!(resolve_target(&items, "#nope"), None);
    }

    /// Per-locale coverage (i18n gate discipline, docs/history/i18n-ui.md §3.5):
    /// every error path renders under EVERY built-in language with no
    /// unsubstituted `{…}` and, for `en`, with no Cyrillic leaking through from
    /// the ru default.
    #[test]
    fn errors_are_localized_for_all_langs() {
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            for input in ["/file", "/file attach", "/file remove", "/file purge x"] {
                let Some(Err(msg)) = parse(input, loc) else {
                    panic!("expected a syntax error for {input:?} in {lang:?}");
                };
                assert!(
                    !msg.contains('{') && !msg.contains('}'),
                    "unsubstituted placeholder in {lang:?}: {msg}"
                );
                if lang == crate::shared::i18n::Lang::En {
                    assert!(
                        !msg.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)),
                        "Cyrillic leaked into the en message: {msg}"
                    );
                }
            }
            // Mode labels are localized too.
            for mode in [AttachMode::Inline, AttachMode::ByReference] {
                assert!(!mode_label(mode, loc).is_empty());
            }
        }
    }
}

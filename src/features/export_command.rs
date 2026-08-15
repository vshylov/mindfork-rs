//! Parses the export slash-command (`/export [md|json] [path]`). Pure, testable
//! logic modeled on [`super::file_command`]; the chat screen calls it on send,
//! and the orchestrator — which owns the conversation and does the disk I/O —
//! writes the file. Error text is localized in the interface language (axis B).
//!
//! See [docs/research/chat-export-file.md](../../docs/research/chat-export-file.md).

use crate::shared::i18n::Locale;

/// What an export is written as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportFormat {
    /// The conversation as text — byte-for-byte what `F5` puts on the clipboard.
    /// Named `.md` because the *content* is Markdown already: that is how models
    /// write, and it is what the feed renders (fork F6).
    #[default]
    Markdown,
    /// The `mindfork-import` v1 document (docs/import-format.md), so an export
    /// can be imported back. Carries no tool calls — the format has nowhere to
    /// put them, which the caller says out loud (fork F2).
    Json,
}

impl ExportFormat {
    /// The extension a generated filename gets.
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Markdown => "md",
            ExportFormat::Json => "json",
        }
    }

    /// The format named by a word, if it is one of ours.
    fn from_word(word: &str) -> Option<Self> {
        match word.to_ascii_lowercase().as_str() {
            "md" | "markdown" => Some(ExportFormat::Markdown),
            "json" => Some(ExportFormat::Json),
            _ => None,
        }
    }

    /// The format a path's extension implies, if any.
    fn from_path(path: &str) -> Option<Self> {
        let ext = std::path::Path::new(path).extension()?.to_str()?;
        Self::from_word(ext)
    }
}

/// A recognized `/export` command: what to write, and where (`None` — generate a
/// name in the current directory).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportCommand {
    pub format: ExportFormat,
    pub path: Option<String>,
}

/// Tries to parse an input string as an `/export` command.
///
/// The grammar is `/export [md|json] [path]`, and the two optional parts are
/// told apart by the only rule that cannot surprise anyone: a **first token that
/// is exactly a format word is the format**, everything after it is the path,
/// and a path alone infers its format from its extension. So `/export json`,
/// `/export transcript.md`, `/export json transcript.txt` and `/export` all mean what they
/// look like. An unknown extension is not an error — the user asked for that
/// name, and the default format applies.
///
/// - `None` — not an `/export` command: send it as a regular message.
/// - `Some(Ok(cmd))` — a correct command.
/// - `Some(Err(msg))` — a localized report (an empty path after a format word).
pub fn parse(input: &str, loc: &Locale) -> Option<Result<ExportCommand, String>> {
    let trimmed = input.trim();
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let head = parts.next()?;
    if !head.eq_ignore_ascii_case("/export") {
        return None;
    }
    let rest = parts.next().unwrap_or_default().trim();
    if rest.is_empty() {
        return Some(Ok(ExportCommand {
            format: ExportFormat::default(),
            path: None,
        }));
    }
    // A leading format word, and whatever follows it is the path.
    let mut words = rest.splitn(2, char::is_whitespace);
    let first = words.next().unwrap_or_default();
    if let Some(format) = ExportFormat::from_word(first) {
        let path = words.next().unwrap_or_default().trim();
        return Some(Ok(ExportCommand {
            format,
            path: clean_path(path),
        }));
    }
    // Otherwise the whole remainder is a path: a filename may contain spaces,
    // and splitting it would write to somewhere the user did not name.
    let path = clean_path(rest);
    match path {
        // `/export "  "` — quotes around nothing. Reported rather than treated
        // as the bare form: the user meant to name a file.
        None => Some(Err(loc.t("ui.export.err.empty_path").to_string())),
        Some(path) => Some(Ok(ExportCommand {
            format: ExportFormat::from_path(&path).unwrap_or_default(),
            path: Some(path),
        })),
    }
}

/// Trims a path and strips the quotes a shell-trained user (or a file manager's
/// "copy as path") puts around it. `None` — nothing left.
fn clean_path(path: &str) -> Option<String> {
    let path = path.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    (!path.is_empty()).then(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};

    fn ru() -> &'static Locale {
        locale(Lang::Ru)
    }

    fn ok(input: &str) -> ExportCommand {
        parse(input, ru())
            .unwrap_or_else(|| panic!("{input:?} was not recognized"))
            .unwrap_or_else(|e| panic!("{input:?} did not parse: {e}"))
    }

    /// The whole grammar in one table — the four shapes a user can type, plus
    /// the case and padding a real input box produces.
    #[test]
    fn the_grammar() {
        use ExportFormat::*;
        for (input, format, path) in [
            // Bare: the default format, a name to be generated.
            ("/export", Markdown, None),
            ("  /EXPORT  ", Markdown, None),
            // A format word alone.
            ("/export json", Json, None),
            ("/export md", Markdown, None),
            ("/export MARKDOWN", Markdown, None),
            // A path alone — the format follows its extension.
            ("/export transcript.json", Json, Some("transcript.json")),
            ("/export transcript.md", Markdown, Some("transcript.md")),
            // An unfamiliar extension is not an error: the default applies and
            // the user still gets the name they asked for.
            ("/export transcript.txt", Markdown, Some("transcript.txt")),
            ("/export notes", Markdown, Some("notes")),
            // Both, with the word winning over the extension.
            ("/export json transcript.txt", Json, Some("transcript.txt")),
            (
                "/export md transcript.json",
                Markdown,
                Some("transcript.json"),
            ),
            // A path with spaces stays one path; quotes come off.
            (
                "/export my transcript.md",
                Markdown,
                Some("my transcript.md"),
            ),
            (
                "/export \"my transcript.md\"",
                Markdown,
                Some("my transcript.md"),
            ),
            (
                "/export json 'out dir/a b.txt'",
                Json,
                Some("out dir/a b.txt"),
            ),
        ] {
            let cmd = ok(input);
            assert_eq!(cmd.format, format, "format of {input:?}");
            assert_eq!(cmd.path.as_deref(), path, "path of {input:?}");
        }
    }

    /// An absolute path is passed through untouched — resolving it is the
    /// writer's job, and mangling it here would write somewhere else.
    #[test]
    fn an_absolute_path_survives() {
        for path in ["/tmp/chat.md", "C:\\Users\\me\\chat.md", "~/chat.json"] {
            assert_eq!(ok(&format!("/export {path}")).path.as_deref(), Some(path));
        }
    }

    #[test]
    fn quotes_around_nothing_are_reported() {
        assert!(matches!(parse("/export \"\"", ru()), Some(Err(_))));
        assert!(matches!(parse("/export '  '", ru()), Some(Err(_))));
    }

    #[test]
    fn other_input_is_none() {
        for text in [
            "/exports",
            "/export-now",
            "/exit",
            "export chat.md",
            "how do I /export this?",
            "",
        ] {
            assert_eq!(parse(text, ru()), None, "input {text:?}");
        }
    }

    /// Per-locale gate (docs/history/i18n-ui.md §3.5).
    #[test]
    fn errors_are_localized_for_all_langs() {
        for &lang in Lang::ALL {
            let loc = locale(lang);
            let Some(Err(msg)) = parse("/export \"\"", loc) else {
                panic!("expected a report in {lang:?}");
            };
            assert!(
                !msg.contains('{') && !msg.contains('}'),
                "unsubstituted placeholder in {lang:?}: {msg}"
            );
            assert!(
                msg.contains("/export"),
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

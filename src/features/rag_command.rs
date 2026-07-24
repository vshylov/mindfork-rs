//! Parses RAG slash-commands in the input box (`/rag add <path> [-r]`). Pure,
//! testable logic: the chat screen calls it on send; a recognized command
//! turns into an intent, while an unrecognized string goes out as a regular message.
//! See spec §9.3 (RAG).

/// A recognized RAG command.
#[derive(Debug, Clone, PartialEq)]
pub enum RagCommand {
    /// Index a file or directory into the knowledge base.
    Add { path: String, recursive: bool },
    /// Remove a file or directory (and everything under it) from the base.
    Delete { path: String },
    /// Show the active profile's knowledge-base sources (chunk count, date).
    List,
    /// Reindex the active profile's knowledge base (after changing the chunk size/
    /// overlap or the embedding model).
    Rebuild,
}

/// A short syntax hint (shown on a parse error).
pub const USAGE: &str = "использование: /rag add|remove <путь> [-r] · /rag list · /rag rebuild";

/// Tries to parse an input string as a RAG command.
///
/// - `None` — the string isn't a RAG command (doesn't start with `/rag`): it should
///   be sent as a regular message.
/// - `Some(Ok(cmd))` — a correct command.
/// - `Some(Err(msg))` — this is a RAG command, but with a syntax error (a hint in `msg`).
pub fn parse(input: &str) -> Option<Result<RagCommand, String>> {
    let mut tokens = input.split_whitespace();
    let first = tokens.next()?;
    if !first.eq_ignore_ascii_case("/rag") {
        return None;
    }
    let Some(sub) = tokens.next() else {
        return Some(Err(format!("укажите подкоманду. {USAGE}")));
    };
    let rest: Vec<&str> = tokens.collect();

    if sub.eq_ignore_ascii_case("add") {
        let (path, recursive) = match extract_path(&rest) {
            Ok(parts) => parts,
            Err(msg) => return Some(Err(msg)),
        };
        Some(Ok(RagCommand::Add { path, recursive }))
    } else if sub.eq_ignore_ascii_case("remove") {
        // The delete subcommand is only `remove` (the word `delete` is deliberately
        // not supported: users worried it would delete the file itself on disk).
        // Deletion doesn't need `-r` (a directory is removed with all its contents), but
        // we accept and ignore it so the syntax stays symmetric with `add`.
        let (path, _recursive) = match extract_path(&rest) {
            Ok(parts) => parts,
            Err(msg) => return Some(Err(msg)),
        };
        Some(Ok(RagCommand::Delete { path }))
    } else if sub.eq_ignore_ascii_case("list") {
        Some(Ok(RagCommand::List))
    } else if sub.eq_ignore_ascii_case("rebuild") {
        Some(Ok(RagCommand::Rebuild))
    } else {
        Some(Err(format!("неизвестная подкоманда «{sub}». {USAGE}")))
    }
}

/// Extracts the path and the recursion flag from the tokens after the subcommand. The path may contain
/// spaces (we gather non-flag tokens), strip surrounding quotes.
fn extract_path(tokens: &[&str]) -> Result<(String, bool), String> {
    let mut recursive = false;
    let mut path_parts: Vec<&str> = Vec::new();
    for tok in tokens {
        match *tok {
            "-r" | "--recursive" => recursive = true,
            _ => path_parts.push(tok),
        }
    }
    let joined = path_parts.join(" ");
    let path = joined.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    if path.is_empty() {
        return Err(format!("укажите путь. {USAGE}"));
    }
    Ok((path.to_string(), recursive))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(path: &str, recursive: bool) -> Option<Result<RagCommand, String>> {
        Some(Ok(RagCommand::Add {
            path: path.into(),
            recursive,
        }))
    }

    #[test]
    fn non_rag_input_is_none() {
        assert_eq!(parse("regular message"), None);
        assert_eq!(parse("/help something"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn parses_file_and_dir() {
        assert_eq!(
            parse("/rag add d:\\dir\\file.txt"),
            add("d:\\dir\\file.txt", false)
        );
        assert_eq!(parse("/rag add d:\\dir"), add("d:\\dir", false));
    }

    #[test]
    fn parses_recursive_flag_anywhere() {
        assert_eq!(parse("/rag add d:\\dir -r"), add("d:\\dir", true));
        assert_eq!(parse("/rag add -r d:\\dir"), add("d:\\dir", true));
        assert_eq!(parse("/rag add d:\\dir --recursive"), add("d:\\dir", true));
    }

    #[test]
    fn path_with_spaces_and_quotes() {
        assert_eq!(
            parse("/rag add C:\\Program Files\\docs -r"),
            add("C:\\Program Files\\docs", true)
        );
        assert_eq!(parse("/rag add \"d:\\my docs\""), add("d:\\my docs", false));
    }

    #[test]
    fn command_is_case_insensitive() {
        assert_eq!(parse("/RAG ADD d:\\x"), add("d:\\x", false));
    }

    fn del(path: &str) -> Option<Result<RagCommand, String>> {
        Some(Ok(RagCommand::Delete { path: path.into() }))
    }

    #[test]
    fn parses_remove() {
        assert_eq!(
            parse("/rag remove d:\\dir\\file.txt"),
            del("d:\\dir\\file.txt")
        );
        assert_eq!(parse("/rag remove d:\\dir"), del("d:\\dir"));
        // `-r` for remove is accepted and ignored (a directory is recursive anyway).
        assert_eq!(parse("/rag remove d:\\dir -r"), del("d:\\dir"));
    }

    #[test]
    fn delete_is_not_a_command() {
        // The word `delete` is deliberately not supported — it's an unknown subcommand.
        assert!(matches!(
            parse("/rag delete d:\\dir\\file.txt"),
            Some(Err(_))
        ));
    }

    #[test]
    fn parses_list_and_rebuild() {
        assert_eq!(parse("/rag list"), Some(Ok(RagCommand::List)));
        assert_eq!(parse("/RAG List"), Some(Ok(RagCommand::List)));
        assert_eq!(parse("/rag rebuild"), Some(Ok(RagCommand::Rebuild)));
        // Extra tokens after list/rebuild are ignored (they don't need paths).
        assert_eq!(parse("/rag list everything"), Some(Ok(RagCommand::List)));
    }

    #[test]
    fn errors_on_missing_parts() {
        assert!(matches!(parse("/rag"), Some(Err(_))));
        assert!(matches!(parse("/rag add"), Some(Err(_))));
        assert!(matches!(parse("/rag add -r"), Some(Err(_))));
        assert!(matches!(parse("/rag remove"), Some(Err(_))));
        assert!(matches!(parse("/rag purge x"), Some(Err(_))));
    }
}

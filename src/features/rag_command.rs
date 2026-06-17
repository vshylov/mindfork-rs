//! Разбор slash-команд RAG в поле ввода (`/rag add <путь> [-r]`). Чистая,
//! тестируемая логика: экран чата вызывает её при отправке; распознанная команда
//! превращается в намерение, а нераспознанная строка уходит обычным сообщением.
//! См. spec §9.3 (RAG).

/// Распознанная команда RAG.
#[derive(Debug, Clone, PartialEq)]
pub enum RagCommand {
    /// Индексировать файл или директорию в базу знаний.
    Add { path: String, recursive: bool },
    /// Удалить из базы файл или директорию (со всем, что под ней).
    Delete { path: String },
}

/// Краткая подсказка по синтаксису (показывается при ошибке разбора).
pub const USAGE: &str = "использование: /rag add|delete <путь к файлу или папке> [-r]";

/// Пытается разобрать строку ввода как команду RAG.
///
/// - `None` — строка не является командой RAG (начинается не с `/rag`): её следует
///   отправить как обычное сообщение.
/// - `Some(Ok(cmd))` — корректная команда.
/// - `Some(Err(msg))` — это команда RAG, но с ошибкой синтаксиса (подсказка `msg`).
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
    } else if sub.eq_ignore_ascii_case("delete") || sub.eq_ignore_ascii_case("remove") {
        // Для удаления `-r` не нужен (директория сносится со всем содержимым), но
        // принимаем и игнорируем его, чтобы синтаксис был симметричен `add`.
        let (path, _recursive) = match extract_path(&rest) {
            Ok(parts) => parts,
            Err(msg) => return Some(Err(msg)),
        };
        Some(Ok(RagCommand::Delete { path }))
    } else {
        Some(Err(format!("неизвестная подкоманда «{sub}». {USAGE}")))
    }
}

/// Извлекает путь и флаг рекурсии из токенов после подкоманды. Путь может содержать
/// пробелы (собираем не-флаговые токены), снимаем обрамляющие кавычки.
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
        assert_eq!(parse("обычное сообщение"), None);
        assert_eq!(parse("/help что-то"), None);
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
    fn parses_delete_and_remove_aliases() {
        assert_eq!(
            parse("/rag delete d:\\dir\\file.txt"),
            del("d:\\dir\\file.txt")
        );
        assert_eq!(parse("/rag remove d:\\dir"), del("d:\\dir"));
        // `-r` для delete принимается и игнорируется (директория и так рекурсивна).
        assert_eq!(parse("/rag delete d:\\dir -r"), del("d:\\dir"));
    }

    #[test]
    fn errors_on_missing_parts() {
        assert!(matches!(parse("/rag"), Some(Err(_))));
        assert!(matches!(parse("/rag add"), Some(Err(_))));
        assert!(matches!(parse("/rag add -r"), Some(Err(_))));
        assert!(matches!(parse("/rag delete"), Some(Err(_))));
        assert!(matches!(parse("/rag purge x"), Some(Err(_))));
    }
}

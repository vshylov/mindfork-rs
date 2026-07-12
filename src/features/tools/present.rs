//! Презентация вызовов инструментов для ленты (spec §11.3): как показать
//! аргументы и результат конкретного инструмента — подсвеченный код, консольный
//! вывод, компактный заголовок вместо сырого JSON.
//!
//! **Чистый слой** (без ratatui): отдаёт структуру [`ToolPresentation`], которую
//! рисует [`crate::widgets::message_feed`] (навешивая цвета/гуттеры/подсветку).
//! Знание про конкретные инструменты (какое поле — код, на каком языке, как
//! разобрать консольный вывод) живёт здесь, в слое инструментов; виджет остаётся
//! generic. `arguments` приходит уже сериализованным JSON-текстом (как в
//! [`crate::widgets::message_feed::FeedToolCall`]) — презентер парсит его сам, при
//! сбое разбора мягко деградирует к прежнему inline-виду.
//!
//! Имена инструментов сверяются строковыми литералами — это стабильный
//! wire-протокол (их видит и модель), меняются крайне редко.

use serde_json::Value;

/// Порог «крупного» строкового аргумента: многострочный ИЛИ длиннее этого числа
/// символов — показывается отдельным блоком под заголовком, а не в `name(...)`.
const BIG_ARG_CHARS: usize = 100;
/// Потолок длины суффикса заголовка (символов) — длинное значение усекается «…».
const HEADER_MAX_CHARS: usize = 100;

/// Инструменты, чей результат — структурированная проза (URL, пассажи, заметки):
/// рендерим его как markdown, а не плоским текстом. Простые подтверждения
/// («Заметка сохранена») сюда не входят — они остаются приглушённым `Plain`.
const PROSE_RESULT_TOOLS: &[&str] = &["web_search", "fetch_url", "rag_search", "note_recall"];

/// Блок содержимого tool-карточки (аргумент или результат).
#[derive(Debug, Clone, PartialEq)]
pub enum ToolBlock {
    /// Подсвеченный код на языке `lang` (пустой `lang` → без подсветки).
    Code { lang: String, text: String },
    /// Консольный вывод процесса (stdout/stderr/код возврата).
    Console(Console),
    /// Плоский текст (обёрнутый, приглушённый) — результат по умолчанию.
    Plain(String),
    /// Markdown-рендер (для текстовых результатов-прозы).
    Markdown(String),
}

/// Разобранный консольный вывод `python_exec`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Console {
    pub stdout: String,
    pub stderr: String,
    pub exit: Option<i32>,
}

/// Как показать вызов инструмента в ленте.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolPresentation {
    /// Суффикс заголовка: `name(suffix)`. `None` → показываем просто `name`.
    pub header_suffix: Option<String>,
    /// Блоки под заголовком (крупные аргументы — код/текст).
    pub args: Vec<ToolBlock>,
    /// Блоки результата.
    pub result: Vec<ToolBlock>,
}

/// Строит презентацию вызова `name` с сериализованными `arguments` и `result`.
pub fn present(name: &str, arguments: &str, result: &str) -> ToolPresentation {
    let val: Option<Value> = serde_json::from_str(arguments).ok();
    let (header_suffix, args) = present_args(name, arguments, val.as_ref());
    let result = present_result(name, val.as_ref(), result);
    ToolPresentation {
        header_suffix,
        args,
        result,
    }
}

/// Заголовок + блоки аргументов.
fn present_args(name: &str, raw: &str, val: Option<&Value>) -> (Option<String>, Vec<ToolBlock>) {
    // Аргументы — не JSON-объект (сбой разбора, массив, скаляр): показываем сырой
    // текст inline, как раньше, без блоков.
    let Some(Value::Object(map)) = val else {
        let t = raw.trim();
        return (
            if t.is_empty() {
                None
            } else {
                Some(truncate_header(t))
            },
            Vec::new(),
        );
    };

    // Спец. «код-поле» инструмента: python — `code` (python), fs_write — `content`
    // (язык по расширению `path`).
    let code_field: Option<(&str, String)> = match name {
        "python_exec" => Some(("code", "python".into())),
        "fs_write" => Some(("content", ext_lang(map.get("path")))),
        _ => None,
    };

    let mut blocks = Vec::new();
    let mut consumed: Option<String> = None;
    if let Some((field, lang)) = &code_field
        && let Some(Value::String(code)) = map.get(*field)
        && !code.trim().is_empty()
    {
        blocks.push(ToolBlock::Code {
            lang: lang.clone(),
            text: code.clone(),
        });
        consumed = Some((*field).to_string());
    }
    // Нет спец. поля → крупное строковое поле показываем отдельным Plain-блоком
    // (напр. `content` у note_save, `text` у rag_add).
    if blocks.is_empty() {
        for (k, v) in map.iter() {
            if let Value::String(s) = v
                && is_big(s)
            {
                blocks.push(ToolBlock::Plain(s.clone()));
                consumed = Some(k.clone());
                break;
            }
        }
    }

    // Оставшиеся короткие скалярные поля → компактный заголовок.
    let mut pairs: Vec<(String, String)> = Vec::new();
    for (k, v) in map.iter() {
        if consumed.as_deref() == Some(k.as_str()) {
            continue;
        }
        if let Some(s) = scalar_str(v)
            && !s.trim().is_empty()
        {
            pairs.push((k.clone(), s));
        }
    }
    (header_from_pairs(&pairs), blocks)
}

/// Блоки результата.
fn present_result(name: &str, args: Option<&Value>, result: &str) -> Vec<ToolBlock> {
    if result.trim().is_empty() {
        return Vec::new();
    }
    match name {
        "python_exec" => match parse_console(result) {
            Some(c) => vec![ToolBlock::Console(c)],
            None => vec![ToolBlock::Plain(result.to_string())],
        },
        // Результат `fs_read` — содержимое файла: подсвечиваем по расширению пути
        // (кроме сообщений об ошибке).
        "fs_read" if !result.starts_with("Не удалось") => {
            let lang = ext_lang(args.and_then(|v| v.get("path")));
            vec![ToolBlock::Code {
                lang,
                text: result.to_string(),
            }]
        }
        n if PROSE_RESULT_TOOLS.contains(&n) => vec![ToolBlock::Markdown(result.to_string())],
        _ => vec![ToolBlock::Plain(result.to_string())],
    }
}

/// Метка кода возврата (`python.console.exit`) во всех вшитых локалях. Формат вывода
/// `python::format_output_parts` локализован (ось A), поэтому парсер распознаёт метку
/// на любом языке профиля. Метки `stdout:`/`stderr:` универсальны (не переводятся).
fn exit_labels() -> Vec<&'static str> {
    crate::shared::i18n::Lang::ALL
        .iter()
        .map(|&l| crate::shared::i18n::locale(l).t("python.console.exit"))
        .collect()
}

/// Разбирает вывод `python_exec` (см. `python::format_output_parts`) в секции stdout/
/// stderr/код возврата. `None` — если текст не похож на этот формат (сообщения об
/// ошибке запуска, «(пустой вывод, успех)») → показываем его плоским текстом.
fn parse_console(result: &str) -> Option<Console> {
    #[derive(PartialEq)]
    enum Sec {
        None,
        Stdout,
        Stderr,
    }
    let exit_labels = exit_labels();
    let mut c = Console::default();
    let mut sec = Sec::None;
    let mut out: Vec<&str> = Vec::new();
    let mut err: Vec<&str> = Vec::new();
    for line in result.lines() {
        if line == "stdout:" {
            sec = Sec::Stdout;
        } else if line == "stderr:" {
            sec = Sec::Stderr;
        } else if let Some(rest) = exit_labels.iter().find_map(|lbl| line.strip_prefix(lbl)) {
            c.exit = rest.trim().parse::<i32>().ok();
            sec = Sec::None;
        } else {
            match sec {
                Sec::Stdout => out.push(line),
                Sec::Stderr => err.push(line),
                // Строка вне известной секции → это не наш формат.
                Sec::None => return None,
            }
        }
    }
    if out.is_empty() && err.is_empty() && c.exit.is_none() {
        return None;
    }
    // Секции склеены через join("\n\n") — снимаем хвостовые пустые строки-разделители.
    c.stdout = join_trim(&out);
    c.stderr = join_trim(&err);
    Some(c)
}

/// Склеивает строки секции, отбрасывая хвостовые пустые (разделитель `\n\n`).
fn join_trim(lines: &[&str]) -> String {
    let mut v = lines.to_vec();
    while v.last().is_some_and(|l| l.trim().is_empty()) {
        v.pop();
    }
    v.join("\n")
}

/// Компактный заголовок из коротких пар: 0 — нет; 1 — только значение (путь/запрос/
/// id — самодостаточны); ≥2 — `k=v, …` (иначе значения неоднозначны).
fn header_from_pairs(pairs: &[(String, String)]) -> Option<String> {
    match pairs.len() {
        0 => None,
        1 => Some(truncate_header(&pairs[0].1)),
        _ => Some(truncate_header(
            &pairs
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(", "),
        )),
    }
}

/// Скалярное значение JSON в строку (объекты/массивы/null → `None`).
fn scalar_str(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

/// «Крупная» ли строка (многострочная или длиннее порога).
fn is_big(s: &str) -> bool {
    s.contains('\n') || s.chars().count() > BIG_ARG_CHARS
}

/// Расширение файла из JSON-значения `path` (нижним регистром; `""` — если нет).
fn ext_lang(path: Option<&Value>) -> String {
    path.and_then(Value::as_str)
        .and_then(|p| p.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Однострочит и усекает суффикс заголовка до [`HEADER_MAX_CHARS`] символов.
fn truncate_header(s: &str) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= HEADER_MAX_CHARS {
        flat
    } else {
        let cut: String = flat.chars().take(HEADER_MAX_CHARS).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_shows_code_block_and_console() {
        let p = present(
            "python_exec",
            r#"{"code":"print(1)\nx = 2"}"#,
            "stdout:\nhello\nworld",
        );
        assert_eq!(p.header_suffix, None, "код уходит в блок, заголовок пуст");
        assert_eq!(
            p.args,
            vec![ToolBlock::Code {
                lang: "python".into(),
                text: "print(1)\nx = 2".into(),
            }]
        );
        assert_eq!(
            p.result,
            vec![ToolBlock::Console(Console {
                stdout: "hello\nworld".into(),
                stderr: String::new(),
                exit: None,
            })]
        );
    }

    #[test]
    fn python_console_parses_all_sections() {
        // Формат `python::format_output`: секции через "\n\n".
        let out = "stdout:\nok line\n\nstderr:\nTraceback\n\nкод возврата: 1";
        let c = parse_console(out).unwrap();
        assert_eq!(c.stdout, "ok line");
        assert_eq!(c.stderr, "Traceback");
        assert_eq!(c.exit, Some(1));
    }

    #[test]
    fn python_non_console_result_is_plain() {
        // «(пустой вывод, успех)» и сообщения об ошибке — не наш формат → Plain.
        let p = present("python_exec", r#"{"code":"pass"}"#, "(пустой вывод, успех)");
        assert_eq!(
            p.result,
            vec![ToolBlock::Plain("(пустой вывод, успех)".into())]
        );
        assert!(parse_console("Не удалось запустить Python (python): нет").is_none());
    }

    #[test]
    fn python_console_parses_localized_exit_label() {
        // Формат вывода локализован (ось A) — parse_console распознаёт метку кода
        // возврата на любом языке (здесь en «exit code:»).
        let en = crate::shared::i18n::locale(crate::shared::i18n::Lang::En);
        let out = format!("stdout:\nok\n\n{} 1", en.t("python.console.exit"));
        let c = parse_console(&out).unwrap();
        assert_eq!(c.stdout, "ok");
        assert_eq!(c.exit, Some(1));
    }

    #[test]
    fn python_console_preserves_blank_lines_inside_stdout() {
        // Пустые строки ВНУТРИ stdout не должны обрывать секцию (парсер по меткам).
        let out = "stdout:\na\n\nb\n\nстрока";
        let c = parse_console(out).unwrap();
        assert_eq!(c.stdout, "a\n\nb\n\nстрока");
    }

    #[test]
    fn fs_write_content_is_code_by_extension() {
        let p = present(
            "fs_write",
            r#"{"path":"src/main.rs","content":"fn main() {}"}"#,
            "Записано в src/main.rs (12 символов).",
        );
        assert_eq!(p.header_suffix.as_deref(), Some("src/main.rs"));
        assert_eq!(
            p.args,
            vec![ToolBlock::Code {
                lang: "rs".into(),
                text: "fn main() {}".into(),
            }]
        );
        assert_eq!(
            p.result,
            vec![ToolBlock::Plain(
                "Записано в src/main.rs (12 символов).".into()
            )]
        );
    }

    #[test]
    fn fs_read_result_is_highlighted_by_path() {
        let p = present("fs_read", r#"{"path":"a.py"}"#, "print('hi')");
        assert_eq!(p.header_suffix.as_deref(), Some("a.py"));
        assert!(p.args.is_empty());
        assert_eq!(
            p.result,
            vec![ToolBlock::Code {
                lang: "py".into(),
                text: "print('hi')".into(),
            }]
        );
    }

    #[test]
    fn fs_read_error_stays_plain() {
        let p = present(
            "fs_read",
            r#"{"path":"a.py"}"#,
            "Не удалось прочитать a.py: нет",
        );
        assert!(matches!(p.result.as_slice(), [ToolBlock::Plain(_)]));
    }

    #[test]
    fn single_scalar_arg_becomes_header_value() {
        let p = present("web_search", r#"{"query":"погода в Москве"}"#, "результаты");
        assert_eq!(p.header_suffix.as_deref(), Some("погода в Москве"));
        assert!(p.args.is_empty());
        // web_search — «проза» → markdown-результат.
        assert_eq!(p.result, vec![ToolBlock::Markdown("результаты".into())]);
    }

    #[test]
    fn multi_scalar_args_become_key_value_header() {
        let p = present(
            "note_link",
            r#"{"from_id":"a","to_id":"b","relation":"supports"}"#,
            "Связь создана",
        );
        // serde_json::Map (BTreeMap) → ключи отсортированы.
        assert_eq!(
            p.header_suffix.as_deref(),
            Some("from_id=a, relation=supports, to_id=b")
        );
        assert_eq!(p.result, vec![ToolBlock::Plain("Связь создана".into())]);
    }

    #[test]
    fn big_text_field_goes_to_block_short_fields_to_header() {
        let long = "слово ".repeat(40); // >100 символов
        let args = serde_json::json!({"content": long, "tags": "заметки"}).to_string();
        let p = present("note_save", &args, "Заметка сохранена");
        // Крупное `content` уехало в блок; одиночное оставшееся поле — значением.
        assert_eq!(p.header_suffix.as_deref(), Some("заметки"));
        assert_eq!(p.args, vec![ToolBlock::Plain(long.clone())]);
    }

    #[test]
    fn invalid_json_args_fall_back_to_inline() {
        let p = present("whatever", "не json", "результат");
        assert_eq!(p.header_suffix.as_deref(), Some("не json"));
        assert!(p.args.is_empty());
        assert_eq!(p.result, vec![ToolBlock::Plain("результат".into())]);
    }

    #[test]
    fn empty_args_and_result_give_bare_name() {
        let p = present("current_time", "{}", "");
        assert_eq!(p.header_suffix, None);
        assert!(p.args.is_empty());
        assert!(p.result.is_empty());
    }

    #[test]
    fn long_header_value_is_truncated() {
        // Невалидный JSON → сырой аргумент inline; длинный усекается «…».
        let raw = "a".repeat(HEADER_MAX_CHARS + 50);
        let p = present("whatever", &raw, "");
        let h = p.header_suffix.unwrap();
        assert!(h.ends_with('…'));
        assert_eq!(h.chars().count(), HEADER_MAX_CHARS + 1);
    }

    #[test]
    fn long_single_line_arg_becomes_block_not_truncated() {
        // Длинное однострочное текстовое поле уходит блоком целиком (не усекается).
        let long = "a".repeat(HEADER_MAX_CHARS + 50);
        let args = serde_json::json!({ "content": long }).to_string();
        let p = present("note_save", &args, "ок");
        assert_eq!(p.header_suffix, None);
        assert_eq!(p.args, vec![ToolBlock::Plain(long)]);
    }
}

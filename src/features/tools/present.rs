//! Tool-call presentation for the feed (spec §11.3): how to show a specific
//! tool's arguments and result — highlighted code, console output,
//! a compact header instead of raw JSON.
//!
//! **A pure layer** (no ratatui): returns the [`ToolPresentation`] structure, which
//! [`crate::widgets::message_feed`] draws (attaching colors/gutters/highlighting).
//! Knowledge about specific tools (which field is code, in what language, how to
//! parse console output) lives here, in the tools layer; the widget stays
//! generic. `arguments` arrives as already-serialized JSON text (as in
//! [`crate::widgets::message_feed::FeedToolCall`]) — the presenter parses it itself; on a
//! parse failure it gracefully degrades to the previous inline view.
//!
//! Tool names are matched by string literals — this is a stable wire
//! protocol (the model sees them too), changing extremely rarely.

use serde_json::Value;

/// The threshold for a "large" string argument: multiline OR longer than this many
/// characters — shown as a separate block under the header, not in `name(...)`.
const BIG_ARG_CHARS: usize = 100;
/// Ceiling on the header-suffix length (characters) — a long value is truncated with "…".
const HEADER_MAX_CHARS: usize = 100;

/// Tools whose result is structured prose (URLs, summaries, notes):
/// render it as markdown, not as flat text. Plain confirmations
/// ("Note saved") aren't included here — they stay muted `Plain`.
///
/// `rag_search` and `attachment_search` are deliberately **absent**: their payload
/// is verbatim fragments of the user's files, i.e. arbitrary text rather than
/// prose we authored. Markdown-parsing it lets a block construct *inside* a
/// fragment escape its list item — a `## Heading` on any line after the first
/// breaks the fragment in two and renders as a document heading in the middle of
/// the result (verified against the real renderer). Since `chunk_markdown`
/// deliberately prepends a section heading to every `*.md` chunk, that was the
/// common case, not a corner one. File fragments render verbatim.
const PROSE_RESULT_TOOLS: &[&str] = &["web_search", "fetch_url", "note_recall"];

/// A content block of a tool card (an argument or a result).
#[derive(Debug, Clone, PartialEq)]
pub enum ToolBlock {
    /// Highlighted code in language `lang` (an empty `lang` → no highlighting).
    Code { lang: String, text: String },
    /// A process's console output (stdout/stderr/exit code).
    Console(Console),
    /// Flat text (wrapped, muted) — the default result.
    Plain(String),
    /// A markdown render (for prose text results).
    Markdown(String),
}

/// The parsed console output of `python_exec`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Console {
    pub stdout: String,
    pub stderr: String,
    pub exit: Option<i32>,
}

/// How to show a tool call in the feed.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolPresentation {
    /// The header suffix: `name(suffix)`. `None` → show just `name`.
    pub header_suffix: Option<String>,
    /// Blocks under the header (large arguments — code/text).
    pub args: Vec<ToolBlock>,
    /// Result blocks.
    pub result: Vec<ToolBlock>,
}

/// Builds the presentation of a call to `name` with serialized `arguments` and `result`.
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

/// The header + argument blocks.
fn present_args(name: &str, raw: &str, val: Option<&Value>) -> (Option<String>, Vec<ToolBlock>) {
    // The arguments aren't a JSON object (a parse failure, an array, a scalar): show the
    // raw text inline, as before, with no blocks.
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

    // A tool's special "code field": python — `code` (python), fs_write — `content`
    // (language by `path`'s extension).
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
    // No special field → show the large string field as a separate Plain block
    // (e.g. `content` for note_save, `text` for rag_add).
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

    // The remaining short scalar fields → a compact header.
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

/// Result blocks.
fn present_result(name: &str, args: Option<&Value>, result: &str) -> Vec<ToolBlock> {
    if result.trim().is_empty() {
        return Vec::new();
    }
    match name {
        "python_exec" => match parse_console(result) {
            Some(c) => vec![ToolBlock::Console(c)],
            None => vec![ToolBlock::Plain(result.to_string())],
        },
        // The `fs_read` result is a file's content: highlight it by the path's extension
        // (except for error messages). The tool's output is localized per profile
        // (axis A, `tool.fs_read.result.read_failed`), so recognizing a failure by a
        // hardcoded Russian prefix broke for non-Russian profiles (a real
        // pre-existing bug) — `fs_read_failure_prefixes` checks it across every
        // known built-in/external locale, mirroring `exit_labels()` (below).
        "fs_read"
            if !fs_read_failure_prefixes()
                .iter()
                .any(|p| result.starts_with(p)) =>
        {
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

/// The exit-code label (`python.console.exit`) across all known locales (built-in +
/// external). The output format of `python::format_output_parts` is localized (axis A), so
/// the parser recognizes the label in any profile language, including externally added ones. The
/// `stdout:`/`stderr:` labels are universal (not translated).
fn exit_labels() -> Vec<&'static str> {
    crate::shared::i18n::Lang::all()
        .iter()
        .map(|&l| crate::shared::i18n::locale(l).t("python.console.exit"))
        .collect()
}

/// The fixed prefix of `fs_read`'s localized failure message
/// (`tool.fs_read.result.read_failed`, e.g. "Could not read {path}: {err}") across all
/// known locales (built-in + external) — up to the first `{path}` placeholder. The
/// tool's result is localized per profile (axis A), so recognizing a failure by a single
/// hardcoded (Russian) prefix broke for non-Russian profiles; this mirrors
/// `exit_labels()` above.
fn fs_read_failure_prefixes() -> Vec<&'static str> {
    crate::shared::i18n::Lang::all()
        .iter()
        .filter_map(|&l| {
            crate::shared::i18n::locale(l)
                .t("tool.fs_read.result.read_failed")
                .split('{')
                .next()
        })
        .collect()
}

/// Parses `python_exec`'s output (see `python::format_output_parts`) into stdout/
/// stderr/exit-code sections. `None` — if the text doesn't look like this format (launch
/// error messages, "(empty output, success)") → show it as flat text.
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
                // A line outside a known section → this isn't our format.
                Sec::None => return None,
            }
        }
    }
    if out.is_empty() && err.is_empty() && c.exit.is_none() {
        return None;
    }
    // Sections are joined via join("\n\n") — strip the trailing empty separator lines.
    c.stdout = join_trim(&out);
    c.stderr = join_trim(&err);
    Some(c)
}

/// Joins a section's lines, dropping trailing empty ones (the `\n\n` separator).
fn join_trim(lines: &[&str]) -> String {
    let mut v = lines.to_vec();
    while v.last().is_some_and(|l| l.trim().is_empty()) {
        v.pop();
    }
    v.join("\n")
}

/// A compact header from short pairs: 0 — none; 1 — just the value (a path/query/
/// id is self-sufficient); ≥2 — `k=v, …` (otherwise the values would be ambiguous).
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

/// A scalar JSON value as a string (objects/arrays/null → `None`).
fn scalar_str(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

/// Whether the string is "large" (multiline or longer than the threshold).
fn is_big(s: &str) -> bool {
    s.contains('\n') || s.chars().count() > BIG_ARG_CHARS
}

/// A file's extension from the JSON value `path` (lowercased; `""` if absent).
fn ext_lang(path: Option<&Value>) -> String {
    path.and_then(Value::as_str)
        .and_then(|p| p.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Flattens to one line and truncates the header suffix to [`HEADER_MAX_CHARS`] characters.
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
        assert_eq!(
            p.header_suffix, None,
            "the code goes into a block, the header is empty"
        );
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
        // The `python::format_output` format: sections separated by "\n\n".
        let out = "stdout:\nok line\n\nstderr:\nTraceback\n\nкод возврата: 1";
        let c = parse_console(out).unwrap();
        assert_eq!(c.stdout, "ok line");
        assert_eq!(c.stderr, "Traceback");
        assert_eq!(c.exit, Some(1));
    }

    #[test]
    fn python_non_console_result_is_plain() {
        // "(empty output, success)" and error messages aren't our format → Plain.
        let p = present("python_exec", r#"{"code":"pass"}"#, "(пустой вывод, успех)");
        assert_eq!(
            p.result,
            vec![ToolBlock::Plain("(пустой вывод, успех)".into())]
        );
        assert!(parse_console("Не удалось запустить Python (python): нет").is_none());
    }

    #[test]
    fn python_console_parses_localized_exit_label() {
        // The output format is localized (axis A) — parse_console recognizes the exit-code
        // label in any language (here en "exit code:").
        let en = crate::shared::i18n::locale(crate::shared::i18n::Lang::En);
        let out = format!("stdout:\nok\n\n{} 1", en.t("python.console.exit"));
        let c = parse_console(&out).unwrap();
        assert_eq!(c.stdout, "ok");
        assert_eq!(c.exit, Some(1));
    }

    #[test]
    fn python_console_preserves_blank_lines_inside_stdout() {
        // Blank lines INSIDE stdout must not end the section (the parser goes by labels).
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
    fn fs_read_error_stays_plain_for_localized_prefix() {
        // The tool's output is localized per profile (axis A) — the failure prefix
        // must be recognized in any language, not just ru (here en).
        let en = crate::shared::i18n::locale(crate::shared::i18n::Lang::En);
        let msg = en.tf(
            "tool.fs_read.result.read_failed",
            &[("path", "a.py"), ("err", "not found")],
        );
        let p = present("fs_read", r#"{"path":"a.py"}"#, &msg);
        assert!(matches!(p.result.as_slice(), [ToolBlock::Plain(_)]));
    }

    #[test]
    fn single_scalar_arg_becomes_header_value() {
        let p = present("web_search", r#"{"query":"погода в Москве"}"#, "результаты");
        assert_eq!(p.header_suffix.as_deref(), Some("погода в Москве"));
        assert!(p.args.is_empty());
        // web_search is "prose" → a markdown result.
        assert_eq!(p.result, vec![ToolBlock::Markdown("результаты".into())]);
    }

    /// Regression: file-fragment results must render **verbatim**, not as
    /// markdown. Parsing them let a block construct inside a fragment escape its
    /// list item — a heading on any line after the first split the fragment and
    /// rendered as a document heading mid-result, and `chunk_markdown` prepends a
    /// heading to every `*.md` chunk, so that was the common case.
    #[test]
    fn file_fragment_results_are_not_parsed_as_markdown() {
        for tool in ["rag_search", "attachment_search"] {
            let p = present(tool, r#"{"query":"x"}"#, "1. [notes.md]\n## Раздел\nтекст");
            assert!(
                matches!(p.result.as_slice(), [ToolBlock::Plain(_)]),
                "{tool} must render its fragments verbatim, got {:?}",
                p.result
            );
        }
        // Tools whose result is prose we authored keep markdown rendering.
        for tool in ["web_search", "fetch_url", "note_recall"] {
            let p = present(tool, r#"{"query":"x"}"#, "**итог**");
            assert!(
                matches!(p.result.as_slice(), [ToolBlock::Markdown(_)]),
                "{tool} should stay markdown, got {:?}",
                p.result
            );
        }
    }

    #[test]
    fn multi_scalar_args_become_key_value_header() {
        let p = present(
            "note_link",
            r#"{"from_id":"a","to_id":"b","relation":"supports"}"#,
            "Связь создана",
        );
        // serde_json::Map (BTreeMap) → keys are sorted.
        assert_eq!(
            p.header_suffix.as_deref(),
            Some("from_id=a, relation=supports, to_id=b")
        );
        assert_eq!(p.result, vec![ToolBlock::Plain("Связь создана".into())]);
    }

    #[test]
    fn big_text_field_goes_to_block_short_fields_to_header() {
        let long = "слово ".repeat(40); // >100 characters
        let args = serde_json::json!({"content": long, "tags": "заметки"}).to_string();
        let p = present("note_save", &args, "Заметка сохранена");
        // The large `content` went into a block; the single remaining field — as a value.
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
        // Invalid JSON → the raw argument inline; a long one is truncated with "…".
        let raw = "a".repeat(HEADER_MAX_CHARS + 50);
        let p = present("whatever", &raw, "");
        let h = p.header_suffix.unwrap();
        assert!(h.ends_with('…'));
        assert_eq!(h.chars().count(), HEADER_MAX_CHARS + 1);
    }

    #[test]
    fn long_single_line_arg_becomes_block_not_truncated() {
        // A long single-line text field goes into a block in full (not truncated).
        let long = "a".repeat(HEADER_MAX_CHARS + 50);
        let args = serde_json::json!({ "content": long }).to_string();
        let p = present("note_save", &args, "ок");
        assert_eq!(p.header_suffix, None);
        assert_eq!(p.args, vec![ToolBlock::Plain(long)]);
    }
}

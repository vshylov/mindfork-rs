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
const PROSE_RESULT_TOOLS: &[&str] = &["web_search", "fetch_url", "note_recall", "call_subagent"];

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

/// The parsed console output of a process — `python_exec` and the code
/// workspace's `code_build`/`code_run`/`code_test` share the shape.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Console {
    /// What was run and how it ended (the command line, the duration, or the
    /// timeout note). Empty for `python_exec`, which runs code rather than a
    /// command the user could read back.
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub exit: Option<i32>,
    /// The `files:` section `python_exec` appends: what the call saved to the chat and
    /// what it did not keep (docs/history/sandbox-file-exchange.md §11 S9). Verbatim — its lines
    /// are the tool's own, already localized.
    pub files: String,
}

/// Assembles a process result into the shape [`parse_console`] reads back —
/// the **one** place that decides it, so the two halves cannot drift.
///
/// The `command:`/`stdout:`/`stderr:` labels are universal (not translated) and
/// the exit-code label is localized (axis A: the model reads this text, and
/// `parse_console` recognizes the label across every locale). Streams arrive
/// **already truncated** — the two callers cap them differently on purpose
/// (`python_exec` drops the tail, a build keeps head and tail), and that choice
/// is not this function's business.
pub(crate) fn format_console(
    command: Option<&str>,
    stdout: &str,
    stderr: &str,
    success: bool,
    code: Option<i32>,
    loc: &crate::shared::i18n::Locale,
) -> String {
    let mut parts = Vec::new();
    if let Some(command) = command.map(str::trim).filter(|c| !c.is_empty()) {
        parts.push(format!("command:\n{command}"));
    }
    if !stdout.trim().is_empty() {
        parts.push(format!("stdout:\n{stdout}"));
    }
    if !stderr.trim().is_empty() {
        parts.push(format!("stderr:\n{stderr}"));
    }
    if !success {
        parts.push(format!(
            "{} {}",
            loc.t("python.console.exit"),
            code.unwrap_or(-1)
        ));
    }
    if parts.is_empty() {
        loc.t("python.console.empty").to_string()
    } else {
        parts.join("\n\n")
    }
}

/// How much of a call's **arguments** to show.
///
/// The header (`name(…)`) is a title: it flattens whitespace, truncates past
/// [`HEADER_MAX_CHARS`] and can only carry scalars, so a long query is cut and a
/// structured argument (an array/object) does not appear in it at all. That is
/// the right summary for a collapsed card, and the wrong thing when the reader
/// has explicitly asked to see the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgDetail {
    /// The header only — a summary. A collapsed card, and the dangerous-tool
    /// confirmation popup (a decision prompt, not a viewer).
    Compact,
    /// Everything. The header stays the title, and whatever it could not carry
    /// is listed below **in full** — see [`ToolPresentation::args`].
    Full,
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

/// Builds the presentation of a call to `name` with serialized `arguments` and
/// `result`. `detail` decides how much of the arguments is shown — see [`ArgDetail`].
pub fn present(name: &str, arguments: &str, result: &str, detail: ArgDetail) -> ToolPresentation {
    let val: Option<Value> = serde_json::from_str(arguments).ok();
    let (header_suffix, args) = present_args(name, arguments, val.as_ref(), detail);
    let result = present_result(name, val.as_ref(), result);
    ToolPresentation {
        header_suffix,
        args,
        result,
    }
}

/// The header + argument blocks.
///
/// The two detail levels are genuinely different presentations, not one with a
/// flag sprinkled through it: [`ArgDetail::Compact`] folds what it can into the
/// header line, [`ArgDetail::Full`] puts the tool's **name alone** in the header
/// and enumerates every argument below.
fn present_args(
    name: &str,
    raw: &str,
    val: Option<&Value>,
    detail: ArgDetail,
) -> (Option<String>, Vec<ToolBlock>) {
    match detail {
        ArgDetail::Compact => compact_args(name, raw, val),
        ArgDetail::Full => (None, full_args(name, raw, val)),
    }
}

/// The tool's own code field as a highlighted block, with the field name that
/// produced it — `None` when this tool has no such field or the value is empty.
fn code_block(name: &str, map: &serde_json::Map<String, Value>) -> Option<(ToolBlock, String)> {
    let (field, lang) = code_field(name, map)?;
    let Some(Value::String(code)) = map.get(field) else {
        return None;
    };
    if code.trim().is_empty() {
        return None;
    }
    Some((
        ToolBlock::Code {
            lang,
            text: code.clone(),
        },
        field.to_string(),
    ))
}

/// The first large string field **in display order** ([`ordered_fields`]) as a
/// plain block (e.g. `content` for note_save, `text` for rag_add) — the fallback
/// when the tool has no code field of its own.
fn big_string_block(
    name: &str,
    map: &serde_json::Map<String, Value>,
) -> Option<(ToolBlock, String)> {
    ordered_fields(name, map)
        .into_iter()
        .find_map(|(k, v)| match v {
            Value::String(s) if is_big(s) => Some((ToolBlock::Plain(s.clone()), k.clone())),
            _ => None,
        })
}

/// The short scalar fields, in display order ([`ordered_fields`]), skipping the
/// one already shown as a block.
fn scalar_pairs(
    name: &str,
    map: &serde_json::Map<String, Value>,
    consumed: Option<&str>,
) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for (k, v) in ordered_fields(name, map) {
        if consumed == Some(k.as_str()) {
            continue;
        }
        if let Some(s) = scalar_str(v)
            && !s.trim().is_empty()
        {
            pairs.push((k.clone(), s));
        }
    }
    pairs
}

/// A summary: as much as fits on the header line, nothing below except a value
/// too large for it (code, a long string).
fn compact_args(name: &str, raw: &str, val: Option<&Value>) -> (Option<String>, Vec<ToolBlock>) {
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

    // At most one block, and the tool's own code field outranks the fallback.
    let (blocks, consumed) = match code_block(name, map).or_else(|| big_string_block(name, map)) {
        Some((block, field)) => (vec![block], Some(field)),
        None => (Vec::new(), None),
    };
    // The remaining short scalar fields → a compact header.
    (
        header_from_pairs(&scalar_pairs(name, map, consumed.as_deref())),
        blocks,
    )
}

/// The whole request: one `key: value` line per field, untruncated; a value that
/// cannot share a line with its key — code, a large or multiline string — goes
/// under a `key:` label as its own block, so code keeps its highlighting. Arrays
/// and objects (which the header cannot represent at all) are shown as compact
/// JSON.
///
/// Field order is the tool's own, when [`FIELD_ORDER`] knows it, and otherwise
/// `serde_json::Map`'s, i.e. **alphabetical** (a `BTreeMap` without the
/// `preserve_order` feature) — never the order the model wrote them in, which
/// the wire format does not preserve for us. Stable either way, which is what
/// matters for something read repeatedly.
fn full_args(name: &str, raw: &str, val: Option<&Value>) -> Vec<ToolBlock> {
    let Some(Value::Object(map)) = val else {
        // Not a JSON object (a parse failure, an array, a bare scalar): the raw
        // text is all there is, and it is shown whole.
        let t = raw.trim();
        return if t.is_empty() {
            Vec::new()
        } else {
            vec![ToolBlock::Plain(t.to_string())]
        };
    };
    let code = code_field(name, map);
    let mut blocks = Vec::new();
    for (k, v) in ordered_fields(name, map) {
        match v {
            // A field with a language of its own (python's `code`, fs_write's
            // `content`) — highlighted, under its label.
            Value::String(s)
                if code.as_ref().is_some_and(|(f, _)| *f == k) && !s.trim().is_empty() =>
            {
                blocks.push(ToolBlock::Plain(format!("{k}:")));
                blocks.push(ToolBlock::Code {
                    lang: code.as_ref().map(|(_, l)| l.clone()).unwrap_or_default(),
                    text: s.clone(),
                });
            }
            // Anything multiline or long enough that `key: value` would not read
            // as one line.
            Value::String(s) if is_big(s) => {
                blocks.push(ToolBlock::Plain(format!("{k}:")));
                blocks.push(ToolBlock::Plain(s.clone()));
            }
            _ => blocks.push(ToolBlock::Plain(match scalar_str(v) {
                // An empty/whitespace scalar prints as JSON (`k: ""`) — `k: `
                // alone reads as a rendering glitch rather than as the value.
                Some(s) if !s.trim().is_empty() => format!("{k}: {s}"),
                _ => format!("{k}: {v}"),
            })),
        }
    }
    blocks
}

/// Display order of a tool's arguments, for the tools whose schema order is not
/// the alphabetical one a `serde_json::Map` iterates in (a `BTreeMap` without
/// the `preserve_order` feature — the wire format does not preserve the model's
/// own order for us either, see [`full_args`]).
///
/// Alphabetical is stable, which is what a card read repeatedly needs, but it is
/// not *meaningful*: `call_subagent` listed its request before the persona it
/// was addressed to, `code_edit` its replacement before the file. So each entry
/// here is the tool's **schema order** — the same order the model reads the
/// parameters in, and the order its author put them in.
///
/// Only the tools where the two orders differ are listed; for everything else
/// (and for MCP tools, whose names are not known here) the map's own order is
/// already the schema's. Names are string literals for the same reason the rest
/// of this module matches them that way — they are a stable wire protocol; a
/// typo or a rename is caught by `field_order_matches_the_registry_schemas`.
const FIELD_ORDER: &[(&str, &[&str])] = &[
    ("call_subagent", &["name", "system_message", "message"]),
    (
        "code_edit",
        &["path", "old_string", "new_string", "replace_all"],
    ),
    ("code_grep", &["pattern", "path", "glob"]),
    ("code_list", &["path", "depth"]),
    ("code_read", &["path", "offset", "limit"]),
    ("code_write", &["path", "content"]),
    ("fetch_url", &["url", "focus", "summarize"]),
    ("fs_write", &["path", "content", "append"]),
    ("note_link", &["from_id", "to_id", "relation"]),
    ("note_merge", &["ids", "content"]),
    ("note_recall", &["query", "tags", "limit"]),
    ("note_revise", &["id", "content"]),
    ("note_supersede", &["old_id", "content"]),
    ("rag_add", &["text", "source"]),
    (
        "update_self_model",
        &["summary", "add_goals", "complete_goals", "abandon_goals"],
    ),
    (
        "update_user_model",
        &[
            "add_traits",
            "remove_traits",
            "add_interests",
            "remove_interests",
            "relationship_dynamic",
            "note",
        ],
    ),
    ("web_search", &["query", "max_results", "fetch_content"]),
    (
        "youtube_watch",
        &["url", "focus", "start", "end", "transcript"],
    ),
];

/// The call's fields in display order: the ones [`FIELD_ORDER`] names for this
/// tool first, in that order, then everything the table does not mention in the
/// map's own (alphabetical) order — an argument stays visible whether or not the
/// table knows about it.
fn ordered_fields<'a>(
    name: &str,
    map: &'a serde_json::Map<String, Value>,
) -> Vec<(&'a String, &'a Value)> {
    let Some((_, order)) = FIELD_ORDER.iter().find(|(tool, _)| *tool == name) else {
        return map.iter().collect();
    };
    let mut fields: Vec<(&String, &Value)> = order
        .iter()
        .filter_map(|field| map.get_key_value(*field))
        .collect();
    fields.extend(map.iter().filter(|(k, _)| !order.contains(&k.as_str())));
    fields
}

/// A tool's "code field": the argument that is source text, and the language to
/// highlight it in — python's `code`, `fs_write`'s `content` (by the path's
/// extension).
fn code_field(name: &str, map: &serde_json::Map<String, Value>) -> Option<(&'static str, String)> {
    match name {
        "python_exec" => Some(("code", "python".into())),
        "fs_write" => Some(("content", ext_lang(map.get("path")))),
        _ => None,
    }
}

/// Result blocks.
fn present_result(name: &str, args: Option<&Value>, result: &str) -> Vec<ToolBlock> {
    if result.trim().is_empty() {
        return Vec::new();
    }
    match name {
        "python_exec"
        | crate::features::tools::code::CODE_BUILD_ID
        | crate::features::tools::code::CODE_RUN_ID
        | crate::features::tools::code::CODE_TEST_ID => match parse_console(result) {
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
/// stderr/exit-code sections, and the `files:` section a Wasmer run appends
/// (docs/history/sandbox-file-exchange.md §11 S9) — a result of that section alone parses too.
/// `None` — if the text doesn't look like this format (launch error messages, "(empty
/// output, success)") → show it as flat text.
fn parse_console(result: &str) -> Option<Console> {
    #[derive(PartialEq)]
    enum Sec {
        None,
        Command,
        Stdout,
        Stderr,
        Files,
    }
    let exit_labels = exit_labels();
    let mut c = Console::default();
    let mut sec = Sec::None;
    let mut cmd: Vec<&str> = Vec::new();
    let mut out: Vec<&str> = Vec::new();
    let mut err: Vec<&str> = Vec::new();
    let mut files: Vec<&str> = Vec::new();
    for line in result.lines() {
        if line == "command:" {
            sec = Sec::Command;
        } else if line == "stdout:" {
            sec = Sec::Stdout;
        } else if line == "stderr:" {
            sec = Sec::Stderr;
        } else if line == "files:" {
            sec = Sec::Files;
        } else if let Some(rest) = exit_labels.iter().find_map(|lbl| line.strip_prefix(lbl)) {
            c.exit = rest.trim().parse::<i32>().ok();
            sec = Sec::None;
        } else {
            match sec {
                Sec::Command => cmd.push(line),
                Sec::Stdout => out.push(line),
                Sec::Stderr => err.push(line),
                Sec::Files => files.push(line),
                // The separator after an exit code, when the `files:` section follows.
                Sec::None if line.trim().is_empty() => {}
                // A line outside a known section → this isn't our format.
                Sec::None => return None,
            }
        }
    }
    if cmd.is_empty() && out.is_empty() && err.is_empty() && files.is_empty() && c.exit.is_none() {
        return None;
    }
    // Sections are joined via join("\n\n") — strip the trailing empty separator lines.
    c.command = join_trim(&cmd);
    c.stdout = join_trim(&out);
    c.stderr = join_trim(&err);
    c.files = join_trim(&files);
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
    let raw = match pairs.len() {
        0 => return None,
        1 => pairs[0].1.clone(),
        _ => pairs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", "),
    };
    Some(truncate_header(&raw))
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
    // The header is one line by construction.
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

    /// The tests below describe the **compact** presentation — the collapsed
    /// card and the confirmation popup. Shadows [`super::present`] so adding the
    /// detail level cost no call-site churn; `ArgDetail::Full` has its own tests.
    fn present(name: &str, arguments: &str, result: &str) -> ToolPresentation {
        super::present(name, arguments, result, ArgDetail::Compact)
    }

    /// The text of every argument block, joined — what the reader sees under the
    /// header.
    fn arg_text(p: &ToolPresentation) -> String {
        p.args
            .iter()
            .map(|b| match b {
                ToolBlock::Code { text, .. }
                | ToolBlock::Plain(text)
                | ToolBlock::Markdown(text) => text.clone(),
                ToolBlock::Console(_) => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn full_detail_puts_the_name_alone_in_the_header_and_lists_the_arguments() {
        // Expanded, the header is just the tool's name and every argument is
        // enumerated below — including the ones the compact header could have
        // carried, so the layout doesn't depend on how long the values happen
        // to be (spec §11.3).
        let args = r#"{"url":"http://x/y","summarize":true}"#;
        let compact = present("fetch_url", args, "ok");
        assert_eq!(
            compact.header_suffix.as_deref(),
            Some("url=http://x/y, summarize=true"),
            "collapsed folds them into the header"
        );
        assert!(compact.args.is_empty());

        let full = super::present("fetch_url", args, "ok", ArgDetail::Full);
        assert_eq!(full.header_suffix, None, "the name alone");
        assert_eq!(
            arg_text(&full),
            "url: http://x/y\nsummarize: true",
            "one line per argument, in the tool's own field order"
        );
    }

    #[test]
    fn full_detail_shows_a_value_the_header_would_truncate() {
        // The header cuts at HEADER_MAX_CHARS; the listing does not. The
        // truncation needs **two** medium values whose joined form overflows —
        // any single scalar over the ceiling is `is_big` and becomes a block of
        // its own instead, so a lone long value never reaches the truncation.
        let url = "http://example.org/a/fairly/long/path/that/still/fits/on/its/own";
        let focus = "memory management modes and their pitfalls";
        let args = format!(r#"{{"url":"{url}","focus":"{focus}","summarize":true}}"#);
        assert!(
            present("fetch_url", &args, "")
                .header_suffix
                .unwrap()
                .ends_with('…'),
            "the premise: this one truncates"
        );
        let text = arg_text(&super::present("fetch_url", &args, "", ArgDetail::Full));
        assert!(text.contains(&format!("url: {url}")), "whole: {text}");
        assert!(text.contains(&format!("focus: {focus}")), "whole: {text}");
    }

    #[test]
    fn full_detail_shows_arguments_the_header_cannot_carry() {
        // Arrays and objects are not scalars, so the header drops them entirely
        // — until now the request was simply unreadable in either mode.
        let args = r#"{"temperature":0.8,"samplers":["top_k","min_p"],"nested":{"a":1}}"#;
        let compact = present("set_sampling", args, "ok");
        assert_eq!(compact.header_suffix.as_deref(), Some("0.8"));
        assert!(compact.args.is_empty());

        let full = super::present("set_sampling", args, "ok", ArgDetail::Full);
        assert_eq!(full.header_suffix, None);
        let text = arg_text(&full);
        assert!(text.contains(r#"samplers: ["top_k","min_p"]"#), "{text}");
        assert!(text.contains(r#"nested: {"a":1}"#), "{text}");
        assert!(text.contains("temperature: 0.8"), "{text}");
    }

    #[test]
    fn full_detail_labels_a_code_argument_and_keeps_its_highlighting() {
        // A value that cannot share a line with its key goes under a `key:`
        // label as its own block — so python code is still highlighted, and the
        // listing still says which argument it is.
        let full = super::present(
            "python_exec",
            r#"{"code":"print(1)","timeout":30}"#,
            "",
            ArgDetail::Full,
        );
        assert_eq!(
            full.args,
            vec![
                ToolBlock::Plain("code:".into()),
                ToolBlock::Code {
                    lang: "python".into(),
                    text: "print(1)".into()
                },
                ToolBlock::Plain("timeout: 30".into()),
            ]
        );
    }

    #[test]
    fn full_detail_labels_a_large_string_argument() {
        let long = "текст ".repeat(40);
        let args = format!(r#"{{"content":{},"tags":"a"}}"#, serde_json::json!(long));
        let full = super::present("note_save", &args, "", ArgDetail::Full);
        assert_eq!(
            full.args,
            vec![
                ToolBlock::Plain("content:".into()),
                ToolBlock::Plain(long),
                ToolBlock::Plain("tags: a".into()),
            ]
        );
    }

    #[test]
    fn full_detail_covers_arguments_that_are_not_a_json_object() {
        // A parse failure / a bare scalar: the raw text is all there is, and the
        // header cuts it at the same ceiling — so it is shown whole below.
        let long = "x".repeat(HEADER_MAX_CHARS + 40);
        assert!(present("t", &long, "").args.is_empty());
        let full = super::present("t", &long, "", ArgDetail::Full);
        assert_eq!(full.header_suffix, None);
        assert_eq!(arg_text(&full), long, "the raw arguments, whole");
        // Empty arguments produce nothing at all.
        assert!(super::present("t", "", "", ArgDetail::Full).args.is_empty());
    }

    #[test]
    fn full_detail_shows_an_empty_value_as_json() {
        // `k: ` alone reads as a rendering glitch rather than as the value.
        let full = super::present("t", r#"{"focus":"","n":1}"#, "", ArgDetail::Full);
        assert_eq!(arg_text(&full), "focus: \"\"\nn: 1");
    }

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
                command: String::new(),
                stdout: "hello\nworld".into(),
                stderr: String::new(),
                exit: None,
                files: String::new(),
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

    /// The section a Wasmer run appends is a block of its own, after an exit code too —
    /// and a quoted head's lines cannot open a section, since each sits behind `  | `.
    #[test]
    fn python_console_keeps_the_files_section() {
        let out = "stdout:\nok\n\nкод возврата: 3\n\nfiles:\nSaved to this chat's files, in /d:\n\
                   - totals.csv — 9 B, text/csv\n  | stdout:\n- a.png — 1 B, image/png — shown";
        let c = parse_console(out).unwrap();
        assert_eq!(c.stdout, "ok");
        assert_eq!(c.exit, Some(3));
        assert_eq!(
            c.files,
            "Saved to this chat's files, in /d:\n- totals.csv — 9 B, text/csv\n  | stdout:\n\
             - a.png — 1 B, image/png — shown"
        );
        let only = parse_console("files:\n- a.txt — 2 B, text/plain").unwrap();
        assert_eq!(only.files, "- a.txt — 2 B, text/plain");
        assert!(only.stdout.is_empty() && only.exit.is_none());
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
        // `note_link` is in FIELD_ORDER, so the fields keep the schema's order
        // (alphabetically `relation` would come second).
        assert_eq!(
            p.header_suffix.as_deref(),
            Some("from_id=a, to_id=b, relation=supports")
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

    #[test]
    fn field_order_follows_the_tools_own_schema() {
        // The complaint this table answers: `call_subagent` showed the request
        // above the persona it was sent to, because a `serde_json::Map` is a
        // BTreeMap and `message` sorts before `system_message`.
        let args = serde_json::json!({
            "system_message": "Ты рецензент. ".repeat(10),
            "message": "Проверь этот вывод. ".repeat(10),
        })
        .to_string();
        let full = super::present("call_subagent", &args, "", ArgDetail::Full);
        let labels: Vec<&String> = full
            .args
            .iter()
            .filter_map(|b| match b {
                ToolBlock::Plain(t) if t.ends_with(':') => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(
            labels,
            vec!["system_message:", "message:"],
            "the schema's order, not the alphabetical one"
        );
    }

    #[test]
    fn field_order_keeps_an_unlisted_argument() {
        // A field the table does not name (a provider extension, a schema that
        // grew) still shows — after the listed ones, in the map's own order.
        let args = r#"{"replace_all":true,"new_string":"b","old_string":"a","path":"src/x.rs","dry_run":true}"#;
        let full = super::present("code_edit", args, "", ArgDetail::Full);
        assert_eq!(
            arg_text(&full),
            "path: src/x.rs
old_string: a
new_string: b
replace_all: true
dry_run: true"
        );
    }

    #[test]
    fn unlisted_tool_keeps_the_alphabetical_order() {
        // Everything the table does not mention — an MCP tool above all, whose
        // name is not knowable here — keeps the map's order, as before.
        let args = r#"{"zeta":1,"alpha":2}"#;
        let full = super::present("mcp__server__whatever", args, "", ArgDetail::Full);
        assert_eq!(
            arg_text(&full),
            "alpha: 2
zeta: 1"
        );
    }

    #[test]
    fn field_order_matches_the_registry_schemas() {
        // The table addresses tools and fields by string literal; a rename or a
        // typo would silently do nothing. Every entry must name a registered
        // tool and only fields its schema declares (the order itself the schema
        // cannot tell us back — `json!` builds a BTreeMap too).
        use crate::features::tools::{ToolConfig, standard_registry};
        let reg = standard_registry(&ToolConfig::default());
        let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::En);
        for (tool, fields) in FIELD_ORDER {
            let t = reg
                .get(tool)
                .unwrap_or_else(|| panic!("{tool} not registered"));
            let schema = t.parameters(loc);
            let props = schema
                .get("properties")
                .and_then(|p| p.as_object())
                .unwrap_or_else(|| panic!("{tool}: no properties in the schema"));
            for field in *fields {
                assert!(
                    props.contains_key(*field),
                    "{tool}: no such argument {field}"
                );
            }
            assert_eq!(
                fields.len(),
                props.len(),
                "{tool}: the table lists {:?}, the schema {:?}",
                fields,
                props.keys().collect::<Vec<_>>()
            );
        }
        let mut names: Vec<&str> = FIELD_ORDER.iter().map(|(t, _)| *t).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), FIELD_ORDER.len(), "a tool listed twice");
    }
}

//! A command line as a person types it, and back: splitting into `argv`,
//! joining `argv` into one line, and spotting the shell syntax this application
//! deliberately does not have.
//!
//! It lives in `shared` because two unrelated features need the same three
//! answers and sit on opposite sides of the dependency direction: the MCP server
//! editor (`screens/settings`, which owned this code first) and the code
//! workspace's command slots (`features/tools/code.rs`). `features` cannot
//! import `screens`, so the choice was to hoist or to copy — and a copy of a
//! quoting parser is the shape the duplication gate reads as one block written
//! twice (docs/lessons.md §2).
//!
//! **No shell is involved anywhere downstream** (design fork F6,
//! docs/history/code-workspace.md §3.3): the line is split here and the program is
//! spawned directly. That buys predictable quoting on both platforms and takes
//! `cmd.exe`'s second round of argument parsing out of the picture — but it also
//! means a pipeline or a redirect is not a command this application can run, and
//! the user has to be told so in the words they typed rather than through a
//! "program not found" from the OS. [`shell_syntax`] is what makes that possible.

/// Splits a command line into `argv`. Whitespace separates; `"…"` quotes with
/// `\\`/`\"` escapes; `'…'` quotes literally.
///
/// An unterminated quote simply runs to the end of the line rather than failing:
/// the settings field this grew up in is edited character by character, and
/// refusing a half-typed value would be hostile. Round-trips with [`join`].
pub fn split(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut started = false; // distinguishes an empty quoted arg from no arg
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            '"' => {
                started = true;
                read_double_quoted(&mut chars, &mut cur);
            }
            '\'' => {
                started = true;
                read_single_quoted(&mut chars, &mut cur);
            }
            c => {
                started = true;
                cur.push(c);
            }
        }
    }
    if started {
        out.push(cur);
    }
    out
}

/// Joins `argv` back into a command line, quoting an argument that could not be
/// read back verbatim. Shell-style rather than a separator character
/// (docs/history/mcp-server-editor.md F3): it is how a person types a command
/// line, and unlike a separator it is total — any argument can be represented.
pub fn join(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            // Quote on anything that would re-parse differently: whitespace
            // (a separator), and either quote character (which would open a
            // quoted run mid-token).
            if a.is_empty()
                || a.chars()
                    .any(|c| c.is_whitespace() || c == '"' || c == '\'')
            {
                let escaped = a.replace('\\', "\\\\").replace('"', "\\\"");
                format!("\"{escaped}\"")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The shell metacharacters this application cannot honour, since it spawns the
/// program itself instead of handing the line to a shell.
///
/// `&` covers `&&` and a trailing background `&`; `|` covers `||` and a pipe.
/// Each is reported as the single character the user can see in their own line.
const SHELL_METACHARS: [char; 5] = ['|', '&', '>', '<', ';'];

/// The first shell metacharacter in `line` that sits **outside** quotes, or
/// `None` when the line is a plain `program arg arg`.
///
/// Quoted occurrences are fine and must stay fine: `grep "a|b" src` is one
/// program and three arguments, and refusing it would be refusing a correct
/// command line.
///
/// This exists because of the failure it prevents. Without it,
/// `cargo build 2>&1 | tee log.txt` is split into a program named `cargo` and
/// arguments including `|` and `tee`, which then either fails deep inside the
/// build or, worse, succeeds while quietly doing something other than what was
/// written. Naming the character and the route that works (a script) is the
/// "a message must close the door" rule (docs/lessons.md §4), and it is checked
/// **when the line is set**, not when a model first tries to run it three turns
/// later.
pub fn shell_syntax(line: &str) -> Option<char> {
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                let mut sink = String::new();
                read_double_quoted(&mut chars, &mut sink);
            }
            '\'' => {
                let mut sink = String::new();
                read_single_quoted(&mut chars, &mut sink);
            }
            c if SHELL_METACHARS.contains(&c) => return Some(c),
            _ => {}
        }
    }
    None
}

/// Reads a `"…"` run (opening quote already consumed) into `cur`, undoing the
/// `\\`/`\"` escapes [`join`] produces.
fn read_double_quoted(chars: &mut impl Iterator<Item = char>, cur: &mut String) {
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                // Only the two characters `join` escapes; any other backslash
                // stays literal (Windows paths).
                Some(n @ ('\\' | '"')) => cur.push(n),
                Some(n) => {
                    cur.push('\\');
                    cur.push(n);
                }
                None => cur.push('\\'),
            },
            c => cur.push(c),
        }
    }
}

/// Reads a `'…'` run (opening quote already consumed) into `cur`, literally.
fn read_single_quoted(chars: &mut impl Iterator<Item = char>, cur: &mut String) {
    for c in chars {
        if c == '\'' {
            break;
        }
        cur.push(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_whitespace_and_honours_both_quote_styles() {
        assert_eq!(
            split("cargo build --offline"),
            ["cargo", "build", "--offline"]
        );
        assert_eq!(
            split(r#"cmd "two words" 'and more'"#),
            ["cmd", "two words", "and more"]
        );
        assert!(split("   ").is_empty());
    }

    /// A Windows path is the argument most likely to carry backslashes, and it
    /// must survive a round trip through the field the user edits.
    #[test]
    fn a_windows_path_round_trips() {
        let args = vec![r"C:\Program Files\app\a.exe".to_string(), "-v".to_string()];
        assert_eq!(split(&join(&args)), args);
    }

    #[test]
    fn every_metacharacter_is_recognized() {
        for (line, expected) in [
            ("cargo build | tee log.txt", '|'),
            ("cargo build > out.txt", '>'),
            ("cargo build < in.txt", '<'),
            ("cargo build && cargo test", '&'),
            ("cargo build; cargo test", ';'),
            ("cargo build 2>&1", '>'),
        ] {
            assert_eq!(shell_syntax(line), Some(expected), "line: {line}");
        }
    }

    /// The half that keeps the check from being a nuisance: a metacharacter
    /// *inside* quotes is an argument, and a line made only of arguments must
    /// pass. Without this the check would refuse `grep "a|b"`, which is a
    /// perfectly runnable command.
    #[test]
    fn a_quoted_metacharacter_is_an_argument_not_a_pipeline() {
        assert_eq!(shell_syntax(r#"grep "a|b" src"#), None);
        assert_eq!(shell_syntax("grep 'a>b' src"), None);
        assert_eq!(shell_syntax("cargo build --offline"), None);
        assert_eq!(shell_syntax(""), None);
    }
}

//! Markdown — LaTeX→unicode: delimiter normalization + command converter.
//! Part of module [`super`]; split out of the markdown.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 6).

/// Sentinels for braces that must survive the final grouping strip: literal
/// `\{`/`\}` **and** the braces of unrecognized brace-commands (`\binom{n}{k}`
/// stays readable, instead of gluing into `\binomnk`).
const LBRACE: char = '\u{1}';
const RBRACE: char = '\u{2}';

/// Recursion-depth ceiling for [`apply_brace_commands`] — a stack guard
/// against pathological nesting (`\frac{\frac{…}}` thousands of levels deep).
/// Beyond it, group content is copied without further parsing.
const MAX_BRACE_DEPTH: usize = 64;

// ---------- formula-delimiter normalization ----------

/// Converts LaTeX delimiter forms to the dollar forms the parser understands:
/// `\(…\)`→`$…$`, `\[…\]`→`$$…$$`. The content of **code spans and code
/// blocks** (backtick runs of any length) is copied verbatim — `\(` inside
/// code is left alone. The parser itself strips the dollars further down
/// (math events).
pub fn normalize_delimiters(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < n {
        // A backtick code span/block: copy the content verbatim (don't touch
        // `\(` inside code). The closing run is the same length.
        if chars[i] == '`' {
            let fence = char_run(&chars, i, '`');
            let body_start = i + fence;
            match find_char_run(&chars, body_start, fence, '`') {
                Some(close_end) => {
                    out.extend(&chars[i..close_end]);
                    i = close_end;
                }
                None if fence >= 3 => {
                    // An unclosed fence is normal while streaming ` ``` `:
                    // copy to the end verbatim (can't normalize inside an
                    // unfinished block).
                    out.extend(&chars[i..]);
                    i = n;
                }
                None => {
                    // An unclosed short run (1–2) is a literal per
                    // CommonMark: copy the backticks themselves and CONTINUE
                    // normalizing (otherwise a lone ` would silence formula
                    // conversion to the end of the message).
                    out.extend(&chars[i..body_start]);
                    i = body_start;
                }
            }
            continue;
        }
        // A block fenced with tildes (`~~~`): content verbatim. A run < 3
        // (e.g. `~~strikethrough~~`) doesn't count as a fence — tildes are
        // copied the regular way.
        if chars[i] == '~' {
            let run = char_run(&chars, i, '~');
            if run >= 3 {
                let body_start = i + run;
                let end = find_tilde_fence_close(&chars, body_start).unwrap_or(n);
                out.extend(&chars[i..end]);
                i = end;
                continue;
            }
        }
        if chars[i] == '\\' && i + 1 < n {
            match chars[i + 1] {
                '(' => {
                    if let Some(close) = find_pair(&chars, i + 2, '\\', ')') {
                        out.push('$');
                        out.extend(&chars[i + 2..close]);
                        out.push('$');
                        i = close + 2;
                        continue;
                    }
                }
                '[' => {
                    if let Some(close) = find_pair(&chars, i + 2, '\\', ']') {
                        out.push_str("$$");
                        out.extend(&chars[i + 2..close]);
                        out.push_str("$$");
                        i = close + 2;
                        continue;
                    }
                }
                _ => {}
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Length of a run of identical characters `ch` starting at `start`.
pub(super) fn char_run(chars: &[char], start: usize, ch: char) -> usize {
    let mut k = start;
    while k < chars.len() && chars[k] == ch {
        k += 1;
    }
    k - start
}

/// Looks for a closing run of `ch` of exactly length `len`, starting at
/// `from`. Returns the index **past** the closing run.
pub(super) fn find_char_run(chars: &[char], from: usize, len: usize, ch: char) -> Option<usize> {
    let mut j = from;
    while j < chars.len() {
        if chars[j] == ch {
            let run = char_run(chars, j, ch);
            if run == len {
                return Some(j + run);
            }
            j += run;
        } else {
            j += 1;
        }
    }
    None
}

/// Looks for a closing tilde run of length ≥ 3, starting at `from` (`~~~`
/// fences close with a run no shorter than the opening one, but for "skip the
/// code" it's enough to find any run ≥ 3). Returns the index **past** the
/// closing run.
pub(super) fn find_tilde_fence_close(chars: &[char], from: usize) -> Option<usize> {
    let mut j = from;
    while j < chars.len() {
        if chars[j] == '~' {
            let run = char_run(chars, j, '~');
            if run >= 3 {
                return Some(j + run);
            }
            j += run;
        } else {
            j += 1;
        }
    }
    None
}

/// Looks for a pair `(a, b)` in a row starting at `from`; returns the index
/// of character `a`.
pub(super) fn find_pair(chars: &[char], from: usize, a: char, b: char) -> Option<usize> {
    let mut i = from;
    while i + 1 < chars.len() {
        if chars[i] == a && chars[i + 1] == b {
            return Some(i);
        }
        i += 1;
    }
    None
}

// ---------- formula content → unicode ----------

/// Formula mode: affects the line separator `\\` (in display — a line break,
/// in inline — "; "). Inline output must not contain line breaks (a ratatui
/// span).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MathMode {
    Inline,
    Display,
}

/// Converts the content of an **inline** formula (`$…$`) into a unicode
/// approximation: fractions `\frac{a}{b}`→`a/b`, roots `\sqrt{x}`→`√(x)`, text
/// wrappers (`\text{…}`/`\mathrm{…}`/…) → their content, commands
/// (`\alpha`→α, `\leq`→≤, …), super-/subscripts (`x^2`→x², `^{-1}`→⁻¹),
/// grouping braces are stripped. Literal `\{`/`\}` are preserved. Unknown
/// commands are left as-is. Environments `\begin{…}…\end{…}` are stripped,
/// `\\` → "; ", `&` (alignment) is removed.
pub fn latex_to_unicode(input: &str) -> String {
    latex_to_unicode_mode(input, MathMode::Inline)
}

/// Like [`latex_to_unicode`], but for a **block** formula (`$$…$$`): `\\`
/// gives a real line break, so `\begin{aligned}…\end{aligned}` lays out row
/// by row.
pub fn latex_to_unicode_display(input: &str) -> String {
    latex_to_unicode_mode(input, MathMode::Display)
}

fn latex_to_unicode_mode(input: &str, mode: MathMode) -> String {
    // Protect literal braces from grouping strip at the end.
    let protected = input.replace("\\{", "\u{1}").replace("\\}", "\u{2}");
    let without_env = strip_environments(&protected, mode);
    let with_braces = apply_brace_commands(&without_env);
    let with_commands = replace_commands(&with_braces);
    let with_scripts = replace_scripts(&with_commands);
    // Strip the remaining grouping braces and restore the literal ones.
    let stripped: String = with_scripts
        .chars()
        .filter(|&c| c != '{' && c != '}')
        .map(|c| match c {
            LBRACE => '{',
            RBRACE => '}',
            other => other,
        })
        .collect();
    // In inline mode there must be no line breaks (a ratatui span) — collapse
    // them into a space; in both modes clean up whitespace line by line
    // (stripping `&`/`\hline` leaves doubles) and drop empty edge lines.
    let stripped = match mode {
        MathMode::Inline => stripped.replace('\n', " "),
        MathMode::Display => stripped,
    };
    stripped
        .split('\n')
        .map(|l| collapse_spaces(l.trim()))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Collapses runs of spaces into one (for a readability approximation, space
/// multiplicity doesn't matter; stripping alignment `&`/`\hline` otherwise
/// leaves doubles).
fn collapse_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        if ch == ' ' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out
}

/// Strips environments `\begin{…}…\end{…}`, turns `\\` into a line separator
/// (per [`MathMode`]), removes alignment `&` and service commands (`\hline`,
/// `\label{…}`, `\notag`, …). Other commands (`\alpha`, `\frac`, …) are copied
/// unchanged — later passes parse them. Works on an already-"protected"
/// string (after `\{`→LBRACE).
pub(super) fn strip_environments(input: &str, mode: MathMode) -> String {
    let chars: Vec<char> = input.chars().collect();
    let n = chars.len();
    let sep = match mode {
        MathMode::Display => "\n",
        MathMode::Inline => "; ",
    };
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < n {
        let c = chars[i];
        // Real line breaks in the source are not formula breaks (only `\\`
        // is a break).
        if c == '\n' || c == '\r' {
            out.push(' ');
            i += 1;
            continue;
        }
        // Table/matrix alignment — removed.
        if c == '&' {
            i += 1;
            continue;
        }
        if c == '\\' {
            // `\\` (+ an optional gap `[6pt]`) — a line break; collapse
            // whitespace around the separator.
            if i + 1 < n && chars[i + 1] == '\\' {
                i += 2;
                if i < n
                    && chars[i] == '['
                    && let Some(rel) = chars[i + 1..].iter().position(|&ch| ch == ']')
                {
                    i += 1 + rel + 1;
                }
                while i < n && chars[i].is_whitespace() {
                    i += 1;
                }
                while out.ends_with(char::is_whitespace) {
                    out.pop();
                }
                out.push_str(sep);
                continue;
            }
            // `\&` — a literal ampersand (not alignment).
            if i + 1 < n && chars[i + 1] == '&' {
                out.push('&');
                i += 2;
                continue;
            }
            // A named command.
            let start = i + 1;
            let mut j = start;
            while j < n && chars[j].is_ascii_alphabetic() {
                j += 1;
            }
            let name: String = chars[start..j].iter().collect();
            match name.as_str() {
                "begin" => {
                    // `\begin{env}` (+ a column spec `{cc}` for array/tabular).
                    let mut k = j;
                    let mut env = String::new();
                    if k < n
                        && chars[k] == '{'
                        && let Some((g, after)) = read_group(&chars, k)
                    {
                        env = g;
                        k = after;
                    }
                    if matches!(env.trim(), "array" | "tabular" | "tabularx")
                        && k < n
                        && chars[k] == '{'
                        && let Some((_g, after)) = read_group(&chars, k)
                    {
                        k = after;
                    }
                    i = k;
                }
                "end" => {
                    let mut k = j;
                    if k < n
                        && chars[k] == '{'
                        && let Some((_g, after)) = read_group(&chars, k)
                    {
                        k = after;
                    }
                    i = k;
                }
                "hline" | "midrule" | "toprule" | "bottomrule" | "notag" | "nonumber" => {
                    i = j;
                }
                "label" | "cline" | "tag" | "ref" | "eqref" => {
                    // strip along with the argument group
                    let mut k = j;
                    if k < n
                        && chars[k] == '{'
                        && let Some((_g, after)) = read_group(&chars, k)
                    {
                        k = after;
                    }
                    i = k;
                }
                "" => {
                    // `\` before a non-letter (a special symbol `\,`, `\%`,
                    // …) — copy the pair, replace_commands will parse it.
                    out.push('\\');
                    if start < n {
                        out.push(chars[start]);
                        i = start + 1;
                    } else {
                        i = start;
                    }
                }
                _ => {
                    // Some other command — copy `\name` unchanged.
                    out.push('\\');
                    out.push_str(&name);
                    i = j;
                }
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Expands brace commands: `\frac{a}{b}`→`a/b`, `\sqrt{a}`→`√(a)`, text
/// wrappers → their content. Group content is processed recursively (nested
/// fractions/wrappers). Other `\name`s are copied as-is (parsed later by
/// [`replace_commands`]).
pub(super) fn apply_brace_commands(input: &str) -> String {
    apply_brace_commands_depth(input, 0)
}

/// Recursively processes a group's content (respecting the depth ceiling).
fn brace_recurse(s: &str, depth: usize) -> String {
    if depth >= MAX_BRACE_DEPTH {
        s.to_string()
    } else {
        apply_brace_commands_depth(s, depth + 1)
    }
}

fn apply_brace_commands_depth(input: &str, depth: usize) -> String {
    let chars: Vec<char> = input.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < n {
        if chars[i] != '\\' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut j = start;
        while j < n && chars[j].is_ascii_alphabetic() {
            j += 1;
        }
        let name: String = chars[start..j].iter().collect();

        // `\sqrt` with an optional index `\sqrt[3]{x}` → ∛(x). Checked before
        // the general brace parsing (the index sits between the name and the
        // group).
        if name == "sqrt" {
            let (root, after_opt) = read_optional(&chars, j);
            if after_opt < n
                && chars[after_opt] == '{'
                && let Some((a, after_a)) = read_group(&chars, after_opt)
            {
                out.push_str(&sqrt_render(root.as_deref(), &brace_recurse(&a, depth)));
                i = after_a;
                continue;
            }
            // otherwise — a "bare" \sqrt, parsed below as a regular command (→ √)
        }

        if !name.is_empty() && j < n && chars[j] == '{' {
            // Fractions: `\frac`/`\dfrac`/`\tfrac`/`\cfrac{a}{b}` → a/b.
            if is_frac_command(&name)
                && let Some((a, after_a)) = read_group(&chars, j)
                && after_a < n
                && chars[after_a] == '{'
                && let Some((b, after_b)) = read_group(&chars, after_a)
            {
                out.push_str(&brace_recurse(&a, depth));
                out.push('/');
                out.push_str(&brace_recurse(&b, depth));
                i = after_b;
                continue;
            }
            // Binomial coefficient `\binom{n}{k}` → C(n, k).
            if name == "binom"
                && let Some((a, after_a)) = read_group(&chars, j)
                && after_a < n
                && chars[after_a] == '{'
                && let Some((b, after_b)) = read_group(&chars, after_a)
            {
                out.push('C');
                out.push('(');
                out.push_str(&brace_recurse(&a, depth));
                out.push_str(", ");
                out.push_str(&brace_recurse(&b, depth));
                out.push(')');
                i = after_b;
                continue;
            }
            // `\pmod{n}` → (mod n).
            if name == "pmod"
                && let Some((a, after_a)) = read_group(&chars, j)
            {
                out.push_str("(mod ");
                out.push_str(&brace_recurse(&a, depth));
                out.push(')');
                i = after_a;
                continue;
            }
            // Overlays `\overset{a}{b}`/`\underset`/`\stackrel` → the base
            // (second) argument; the annotation is dropped (like a diacritic
            // on accents).
            if is_stack_command(&name)
                && let Some((_a, after_a)) = read_group(&chars, j)
                && after_a < n
                && chars[after_a] == '{'
                && let Some((b, after_b)) = read_group(&chars, after_a)
            {
                out.push_str(&brace_recurse(&b, depth));
                i = after_b;
                continue;
            }
            // Typefaces `\mathbb{R}`→ℝ, `\mathcal{L}`→ℒ, `\mathfrak{g}`: a
            // single letter with a BMP counterpart → the glyph; otherwise —
            // content (like a wrapper).
            if matches!(name.as_str(), "mathbb" | "mathcal" | "mathfrak")
                && let Some((a, after_a)) = read_group(&chars, j)
            {
                out.push_str(&blackboard_or_content(&name, &a, depth));
                i = after_a;
                continue;
            }
            // Text/font wrappers and accents → content.
            if is_text_command(&name)
                && let Some((a, after_a)) = read_group(&chars, j)
            {
                out.push_str(&brace_recurse(&a, depth));
                i = after_a;
                continue;
            }
            // An UNKNOWN command with a group: keep `\name` and all its
            // groups, **protecting the braces** (sentinels) — otherwise the
            // final strip would glue `\binom{n}{k}` into `\binomnk`. Group
            // content is processed recursively.
            out.push('\\');
            out.push_str(&name);
            let mut k = j;
            while k < n && chars[k] == '{' {
                let Some((g, after)) = read_group(&chars, k) else {
                    break;
                };
                out.push(LBRACE);
                out.push_str(&brace_recurse(&g, depth));
                out.push(RBRACE);
                k = after;
            }
            i = k;
            continue;
        }

        // A command with no group (or a lone '\'): copy '\', the rest is
        // picked up by replace_commands.
        out.push('\\');
        i += 1;
    }
    out
}

/// Fractions reducible to `a/b`.
fn is_frac_command(name: &str) -> bool {
    matches!(name, "frac" | "dfrac" | "tfrac" | "cfrac")
}

/// Overlay commands whose base (second) argument we take.
fn is_stack_command(name: &str) -> bool {
    matches!(name, "overset" | "underset" | "stackrel")
}

/// Reads an optional argument `[…]` starting at `from` (if present there).
/// Returns (content without brackets, the index past `]`); if there's no `[`
/// — `(None, from)`.
fn read_optional(chars: &[char], from: usize) -> (Option<String>, usize) {
    if from < chars.len()
        && chars[from] == '['
        && let Some(rel) = chars[from + 1..].iter().position(|&c| c == ']')
    {
        let inner: String = chars[from + 1..from + 1 + rel].iter().collect();
        return (Some(inner), from + 1 + rel + 1);
    }
    (None, from)
}

/// Renders a root by its optional index: `None`/`2` → `√(x)`, `3` → `∛(x)`,
/// `4` → `∜(x)`, otherwise a prefix superscript `ⁿ√(x)` (if the index maps
/// fully) or the fallback `√[n](x)`.
fn sqrt_render(root: Option<&str>, inner: &str) -> String {
    let prefix = match root {
        None | Some("2") => "√".to_string(),
        Some("3") => "∛".to_string(),
        Some("4") => "∜".to_string(),
        Some(r) => {
            let rc: Vec<char> = r.chars().collect();
            match map_script_chars(&rc, true) {
                Some(sup) => format!("{sup}√"),
                None => format!("√[{r}]"),
            }
        }
    };
    format!("{prefix}({inner})")
}

/// `\mathbb{R}`→ℝ and kin: a single letter with a BMP counterpart → the
/// glyph, otherwise the content is treated as an ordinary text wrapper
/// (`\mathbb{XY}`→`XY`). Supplementary-plane characters (`𝔸`…, lowercase)
/// aren't used — terminals support them unevenly.
fn blackboard_or_content(name: &str, content: &str, depth: usize) -> String {
    let chars: Vec<char> = content.chars().collect();
    if chars.len() == 1 {
        let mapped = match name {
            "mathbb" => double_struck(chars[0]),
            "mathcal" => script_letter(chars[0]),
            "mathfrak" => fraktur_letter(chars[0]),
            _ => None,
        };
        if let Some(g) = mapped {
            return g.to_string();
        }
    }
    brace_recurse(content, depth)
}

/// Double-struck (blackboard bold) capitals from the Letterlike Symbols block
/// (BMP).
fn double_struck(c: char) -> Option<char> {
    Some(match c {
        'C' => 'ℂ',
        'H' => 'ℍ',
        'N' => 'ℕ',
        'P' => 'ℙ',
        'Q' => 'ℚ',
        'R' => 'ℝ',
        'Z' => 'ℤ',
        _ => return None,
    })
}

/// Script (calligraphic) letters from the Letterlike Symbols block (BMP, an
/// incomplete set).
fn script_letter(c: char) -> Option<char> {
    Some(match c {
        'B' => 'ℬ',
        'E' => 'ℰ',
        'F' => 'ℱ',
        'H' => 'ℋ',
        'I' => 'ℐ',
        'L' => 'ℒ',
        'M' => 'ℳ',
        'R' => 'ℛ',
        'e' => 'ℯ',
        'g' => 'ℊ',
        'o' => 'ℴ',
        _ => return None,
    })
}

/// Fraktur (blackletter) letters from the Letterlike Symbols block (BMP, an
/// incomplete set).
fn fraktur_letter(c: char) -> Option<char> {
    Some(match c {
        'C' => 'ℭ',
        'H' => 'ℌ',
        'I' => 'ℑ',
        'R' => 'ℜ',
        'Z' => 'ℨ',
        _ => return None,
    })
}

/// Reads a balanced group `{…}`, starting at `open` (where
/// `chars[open]=='{'`). Returns (content without the outer braces, the index
/// past `}`). `None` — no matching pair.
pub(super) fn read_group(chars: &[char], open: usize) -> Option<(String, usize)> {
    let mut depth = 0usize;
    let mut buf = String::new();
    let mut i = open;
    while i < chars.len() {
        match chars[i] {
            '{' => {
                if depth > 0 {
                    buf.push('{');
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((buf, i + 1));
                }
                buf.push('}');
            }
            c => buf.push(c),
        }
        i += 1;
    }
    None
}

/// Text/font wrappers whose content only we take.
pub(super) fn is_text_command(name: &str) -> bool {
    matches!(
        name,
        "text"
            | "textbf"
            | "textit"
            | "textrm"
            | "textnormal"
            | "textsf"
            | "textsl"
            | "texttt"
            | "textup"
            | "mbox"
            | "emph"
            | "mathrm"
            | "mathbf"
            | "mathit"
            | "mathbb"
            | "mathcal"
            | "mathfrak"
            | "mathsf"
            | "mathtt"
            | "operatorname"
            | "boldsymbol"
            // accents/wrappers: show the content (diacritics are dropped)
            | "overline"
            | "underline"
            | "overbrace"
            | "underbrace"
            | "overrightarrow"
            | "overleftarrow"
            | "hat"
            | "widehat"
            | "bar"
            | "vec"
            | "tilde"
            | "widetilde"
            | "dot"
            | "ddot"
            | "mathring"
            | "breve"
            | "acute"
            | "grave"
            | "check"
    )
}

/// Operator-name functions (`\log`, `\sin`, `\lim`, …) — printed as a word.
pub(super) fn is_function_name(name: &str) -> bool {
    matches!(
        name,
        "log"
            | "ln"
            | "lg"
            | "exp"
            | "sin"
            | "cos"
            | "tan"
            | "cot"
            | "sec"
            | "csc"
            | "sinh"
            | "cosh"
            | "tanh"
            | "coth"
            | "arcsin"
            | "arccos"
            | "arctan"
            | "lim"
            | "limsup"
            | "liminf"
            | "sup"
            | "inf"
            | "min"
            | "max"
            | "gcd"
            | "det"
            | "deg"
            | "dim"
            | "ker"
            | "hom"
            | "arg"
            | "Pr"
            | "mod"
    )
}

// ---------- `\name` commands ----------

/// Replaces `\name` with unicode per the table (the longest name match).
/// `\\` → `\`; `\ ` (backslash-space) → a space; an unknown command is left
/// alone.
pub(super) fn replace_commands(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            // safe: cuts on an ASCII character boundary, or copies UTF-8 as-is
            let ch = input[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        // found '\'; read the command name (ASCII letters)
        let start = i + 1;
        let mut j = start;
        while j < bytes.len() && bytes[j].is_ascii_alphabetic() {
            j += 1;
        }
        if j == start {
            // not a letter after '\': a spacing command (`\,` `\;` `\:`
            // `\ `→space, `\!`→nothing), an escaped character (`\\`, `\_`,
            // …), or a lone '\'.
            if start < bytes.len() {
                let next = input[start..].chars().next().unwrap();
                match next {
                    ',' | ';' | ':' | ' ' => out.push(' '),
                    '!' => {}
                    other => out.push(other),
                }
                i = start + next.len_utf8();
            } else {
                out.push('\\');
                i += 1;
            }
            continue;
        }
        let name = &input[start..j];
        if let Some(sym) = command_symbol(name) {
            out.push_str(sym);
            i = j;
            // `\left.` / `\right.` — an "invisible" delimiter dot: the
            // modifier is dropped (empty string), also eat the dot so no
            // dangling "." remains.
            if (name == "left" || name == "right") && i < bytes.len() && bytes[i] == b'.' {
                i += 1;
            }
            // The space after a command is NOT consumed (unlike real LaTeX):
            // this is a readability approximation, and the user's spaces are
            // a meaningful visual separator (`\alpha + \beta` → "α + β", not
            // "α+ β").
        } else if is_function_name(name) {
            // An operator name (`\log`, `\sin`, `\lim`, …) — printed as a word.
            out.push_str(name);
            i = j;
        } else {
            // an unknown command — left as-is (with the '\')
            out.push('\\');
            out.push_str(name);
            i = j;
        }
    }
    out
}

/// LaTeX command → unicode substitution table.
pub(super) fn command_symbol(name: &str) -> Option<&'static str> {
    let s = match name {
        // lowercase Greek
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "epsilon" | "varepsilon" => "ε",
        "zeta" => "ζ",
        "eta" => "η",
        "theta" => "θ",
        "iota" => "ι",
        "kappa" => "κ",
        "lambda" => "λ",
        "mu" => "μ",
        "nu" => "ν",
        "xi" => "ξ",
        "pi" => "π",
        "rho" => "ρ",
        "sigma" => "σ",
        "tau" => "τ",
        "upsilon" => "υ",
        "phi" | "varphi" => "φ",
        "chi" => "χ",
        "psi" => "ψ",
        "omega" => "ω",
        // uppercase Greek
        "Gamma" => "Γ",
        "Delta" => "Δ",
        "Theta" => "Θ",
        "Lambda" => "Λ",
        "Xi" => "Ξ",
        "Pi" => "Π",
        "Sigma" => "Σ",
        "Phi" => "Φ",
        "Psi" => "Ψ",
        "Omega" => "Ω",
        // arrows
        "rightarrow" | "to" => "→",
        "leftarrow" | "gets" => "←",
        "leftrightarrow" => "↔",
        "longrightarrow" => "⟶",
        "longleftarrow" => "⟵",
        "Rightarrow" | "implies" => "⇒",
        "Longrightarrow" => "⟹",
        "Leftarrow" => "⇐",
        "Leftrightarrow" | "iff" => "⇔",
        "uparrow" => "↑",
        "downarrow" => "↓",
        "mapsto" => "↦",
        // operators and relations
        "leq" | "le" => "≤",
        "geq" | "ge" => "≥",
        "neq" | "ne" => "≠",
        "ll" => "≪",
        "gg" => "≫",
        "approx" => "≈",
        "equiv" => "≡",
        "sim" => "∼",
        "times" => "×",
        "div" => "÷",
        "pm" => "±",
        "mp" => "∓",
        "cdot" => "·",
        "ast" => "∗",
        "star" => "⋆",
        "bullet" => "•",
        "circ" => "∘",
        "infty" => "∞",
        "partial" => "∂",
        "nabla" => "∇",
        "sum" => "∑",
        "prod" => "∏",
        "int" => "∫",
        "oint" => "∮",
        "sqrt" => "√",
        "propto" => "∝",
        "in" => "∈",
        "notin" => "∉",
        "subset" => "⊂",
        "subseteq" => "⊆",
        "supset" => "⊃",
        "supseteq" => "⊇",
        "cup" => "∪",
        "cap" => "∩",
        "emptyset" | "varnothing" => "∅",
        "forall" => "∀",
        "exists" => "∃",
        "nexists" => "∄",
        "neg" | "lnot" => "¬",
        "land" | "wedge" => "∧",
        "lor" | "vee" => "∨",
        "oplus" => "⊕",
        "otimes" => "⊗",
        "perp" => "⊥",
        "top" => "⊤",
        "bot" => "⊥",
        "because" => "∵",
        "therefore" => "∴",
        "angle" => "∠",
        "triangle" => "△",
        "degree" => "°",
        "prime" => "′",
        "hbar" => "ℏ",
        "ell" => "ℓ",
        "Re" => "ℜ",
        "Im" => "ℑ",
        "aleph" => "ℵ",
        "wp" => "℘",
        "ldots" | "dots" => "…",
        "cdots" => "⋯",
        "vdots" => "⋮",
        "ddots" => "⋱",
        // delimiters and separators
        "langle" => "⟨",
        "rangle" => "⟩",
        "lfloor" => "⌊",
        "rfloor" => "⌋",
        "lceil" => "⌈",
        "rceil" => "⌉",
        "mid" => "|",
        "parallel" | "Vert" => "‖",
        "setminus" | "smallsetminus" => "∖",
        "backslash" => "\\",
        // relations
        "cong" => "≅",
        "simeq" => "≃",
        "asymp" => "≍",
        "doteq" => "≐",
        "vdash" => "⊢",
        "dashv" => "⊣",
        "models" | "vDash" => "⊨",
        "triangleq" => "≜",
        "coloneqq" | "coloneq" => "≔",
        "subsetneq" => "⊊",
        "supsetneq" => "⊋",
        "nsubseteq" => "⊈",
        "nsupseteq" => "⊉",
        "sqsubseteq" => "⊑",
        "sqsupseteq" => "⊒",
        "ni" | "owns" => "∋",
        // arrows (symmetric with the ones above + common ones)
        "Longleftarrow" => "⟸",
        "longleftrightarrow" => "⟷",
        "Longleftrightarrow" => "⟺",
        "hookrightarrow" => "↪",
        "hookleftarrow" => "↩",
        "twoheadrightarrow" => "↠",
        "rightsquigarrow" | "leadsto" => "⇝",
        "nearrow" => "↗",
        "searrow" => "↘",
        "nwarrow" => "↖",
        "swarrow" => "↙",
        // Greek variants
        "vartheta" => "ϑ",
        "varsigma" => "ς",
        "varrho" => "ϱ",
        "varkappa" => "ϰ",
        "varpi" => "ϖ",
        // set operators and big operators
        "sqcap" => "⊓",
        "sqcup" => "⊔",
        "uplus" => "⊎",
        "bigcup" => "⋃",
        "bigcap" => "⋂",
        "coprod" => "∐",
        "iint" => "∬",
        "iiint" => "∭",
        "odot" => "⊙",
        "ominus" => "⊖",
        "oslash" => "⊘",
        // other symbols
        "dagger" => "†",
        "ddagger" => "‡",
        "diamond" => "⋄",
        "Box" | "square" => "□",
        "blacksquare" => "■",
        "triangleleft" => "◁",
        "triangleright" => "▷",
        "checkmark" => "✓",
        // spacing commands (readability): reduce to a space
        "quad" => " ",
        "qquad" => "  ",
        "bmod" => "mod",
        // bracket-size modifiers — stripped, the delimiter bracket stays
        "left" | "right" | "big" | "Big" | "bigg" | "Bigg" | "bigl" | "bigr" | "Bigl" | "Bigr"
        | "biggl" | "biggr" => "",
        _ => return None,
    };
    Some(s)
}

// ---------- super-/subscripts ----------

/// Replaces `^x`/`^{xyz}` and `_x`/`_{xyz}` with unicode indices, if every
/// character of the group has a unicode counterpart. Otherwise leaves the
/// construct as-is.
pub(super) fn replace_scripts(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '^' || c == '_' {
            let sup = c == '^';
            if let Some((mapped, consumed)) = take_script(&chars[i + 1..], sup) {
                out.push_str(&mapped);
                i += 1 + consumed;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Tries to read a script argument after `^`/`_`: either `{...}` or a single
/// character. Returns (unicode string, how many characters were consumed) on
/// success.
pub(super) fn take_script(rest: &[char], sup: bool) -> Option<(String, usize)> {
    if rest.is_empty() {
        return None;
    }
    if rest[0] == '{' {
        // look for the closing '}'
        let close = rest.iter().position(|&c| c == '}')?;
        let inner = &rest[1..close];
        match map_script_chars(inner, sup) {
            Some(mapped) => Some((mapped, close + 1)), // including '{' and '}'
            None => {
                // The group doesn't map as a whole — show it as `^(…)`/`_(…)`,
                // keeping the grouping (otherwise `x^{q+}` would lose its
                // braces → `x^q+`).
                let marker = if sup { '^' } else { '_' };
                let inner_str: String = inner.iter().collect();
                Some((format!("{marker}({inner_str})"), close + 1))
            }
        }
    } else {
        // A single character with no group: doesn't map — leave as-is (`x^q`→`x^q`).
        let mapped = map_script_chars(&rest[0..1], sup)?;
        Some((mapped, 1))
    }
}

/// Maps all characters to unicode indices; `None` if even one has no
/// counterpart.
pub(super) fn map_script_chars(chars: &[char], sup: bool) -> Option<String> {
    if chars.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(chars.len());
    for &c in chars {
        let mapped = if sup { superscript(c) } else { subscript(c) }?;
        out.push(mapped);
    }
    Some(out)
}

pub(super) fn superscript(c: char) -> Option<char> {
    Some(match c {
        '0' => '⁰',
        '1' => '¹',
        '2' => '²',
        '3' => '³',
        '4' => '⁴',
        '5' => '⁵',
        '6' => '⁶',
        '7' => '⁷',
        '8' => '⁸',
        '9' => '⁹',
        '+' => '⁺',
        '-' => '⁻',
        '=' => '⁼',
        '(' => '⁽',
        ')' => '⁾',
        // lowercase Latin (no 'q' in Unicode)
        'a' => 'ᵃ',
        'b' => 'ᵇ',
        'c' => 'ᶜ',
        'd' => 'ᵈ',
        'e' => 'ᵉ',
        'f' => 'ᶠ',
        'g' => 'ᵍ',
        'h' => 'ʰ',
        'i' => 'ⁱ',
        'j' => 'ʲ',
        'k' => 'ᵏ',
        'l' => 'ˡ',
        'm' => 'ᵐ',
        'n' => 'ⁿ',
        'o' => 'ᵒ',
        'p' => 'ᵖ',
        'r' => 'ʳ',
        's' => 'ˢ',
        't' => 'ᵗ',
        'u' => 'ᵘ',
        'v' => 'ᵛ',
        'w' => 'ʷ',
        'x' => 'ˣ',
        'y' => 'ʸ',
        'z' => 'ᶻ',
        // uppercase Latin (not all available)
        'A' => 'ᴬ',
        'B' => 'ᴮ',
        'D' => 'ᴰ',
        'E' => 'ᴱ',
        'G' => 'ᴳ',
        'H' => 'ᴴ',
        'I' => 'ᴵ',
        'J' => 'ᴶ',
        'K' => 'ᴷ',
        'L' => 'ᴸ',
        'M' => 'ᴹ',
        'N' => 'ᴺ',
        'O' => 'ᴼ',
        'P' => 'ᴾ',
        'R' => 'ᴿ',
        'T' => 'ᵀ',
        'U' => 'ᵁ',
        'V' => 'ⱽ',
        'W' => 'ᵂ',
        _ => return None,
    })
}

pub(super) fn subscript(c: char) -> Option<char> {
    Some(match c {
        '0' => '₀',
        '1' => '₁',
        '2' => '₂',
        '3' => '₃',
        '4' => '₄',
        '5' => '₅',
        '6' => '₆',
        '7' => '₇',
        '8' => '₈',
        '9' => '₉',
        '+' => '₊',
        '-' => '₋',
        '=' => '₌',
        '(' => '₍',
        ')' => '₎',
        // lowercase Latin (Unicode covers only part of it)
        'a' => 'ₐ',
        'e' => 'ₑ',
        'h' => 'ₕ',
        'i' => 'ᵢ',
        'j' => 'ⱼ',
        'k' => 'ₖ',
        'l' => 'ₗ',
        'm' => 'ₘ',
        'n' => 'ₙ',
        'o' => 'ₒ',
        'p' => 'ₚ',
        'r' => 'ᵣ',
        's' => 'ₛ',
        't' => 'ₜ',
        'u' => 'ᵤ',
        'v' => 'ᵥ',
        'x' => 'ₓ',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::super::testkit::*;
    use super::*;

    #[test]
    fn greek_and_arrows() {
        assert_eq!(latex_to_unicode(r"\alpha + \beta"), "α + β");
        assert_eq!(latex_to_unicode(r"A \rightarrow B"), "A → B");
        assert_eq!(latex_to_unicode(r"x \leq y \times z"), "x ≤ y × z");
        assert_eq!(latex_to_unicode(r"\Omega \neq \emptyset"), "Ω ≠ ∅");
    }

    #[test]
    fn command_preserves_following_whitespace() {
        // user spaces are preserved (a readability approximation)
        assert_eq!(latex_to_unicode(r"\pi r^2"), "π r²");
        assert_eq!(latex_to_unicode(r"\alpha+\beta"), "α+β");
    }

    #[test]
    fn unknown_command_is_left_intact() {
        assert_eq!(latex_to_unicode(r"\foobar x"), r"\foobar x");
    }

    #[test]
    fn escaped_backslash_and_brace() {
        // \\ is now a line separator (in inline → "; "); a literal backslash
        // is written as \backslash (see backslash_command_is_literal).
        assert_eq!(latex_to_unicode(r"a \\ b"), "a; b");
        assert_eq!(latex_to_unicode(r"\{x\}"), "{x}");
    }

    #[test]
    fn superscripts_and_subscripts() {
        assert_eq!(latex_to_unicode("x^2"), "x²");
        assert_eq!(latex_to_unicode("H_2O"), "H₂O");
        assert_eq!(latex_to_unicode("e^{-1}"), "e⁻¹");
        assert_eq!(latex_to_unicode("a_{12}"), "a₁₂");
    }

    #[test]
    fn unmappable_script_keeps_group_as_parens() {
        // 'q' has no superscript form — a single character with no group
        // stays as-is; a group with an unmappable character is preserved as
        // `^(…)` (doesn't lose its braces).
        assert_eq!(latex_to_unicode("x^q"), "x^q");
        assert_eq!(latex_to_unicode("x^{q+}"), "x^(q+)");
    }

    #[test]
    fn letter_scripts_map_to_unicode() {
        // letter indices (common: x_i, a_n, x^T) now convert
        assert_eq!(latex_to_unicode("x_i"), "xᵢ");
        assert_eq!(latex_to_unicode("a_n"), "aₙ");
        assert_eq!(latex_to_unicode("x^T"), "xᵀ");
        assert_eq!(latex_to_unicode("x^{ab}"), "xᵃᵇ");
        assert_eq!(latex_to_unicode(r"\sum_{i=1}^{n}"), "∑ᵢ₌₁ⁿ");
    }

    #[test]
    fn mathbb_double_struck() {
        assert_eq!(latex_to_unicode(r"x \in \mathbb{R}"), "x ∈ ℝ");
        assert_eq!(latex_to_unicode(r"\mathbb{Z}"), "ℤ");
        assert_eq!(latex_to_unicode(r"\mathcal{L}"), "ℒ");
        assert_eq!(latex_to_unicode(r"\mathfrak{g}"), "g"); // 'g' isn't in the set → content
        assert_eq!(latex_to_unicode(r"\mathbb{XY}"), "XY"); // not a single letter → content
    }

    #[test]
    fn plain_text_unchanged() {
        assert_eq!(
            latex_to_unicode("обычный текст 2 + 2"),
            "обычный текст 2 + 2"
        );
    }

    #[test]
    fn frac_sqrt_and_text_wrappers() {
        assert_eq!(latex_to_unicode(r"\frac{a}{b}"), "a/b");
        assert_eq!(latex_to_unicode(r"\frac{\alpha}{2}"), "α/2");
        assert_eq!(latex_to_unicode(r"\sqrt{x+1}"), "√(x+1)");
        assert_eq!(latex_to_unicode(r"\text{скорость} = v"), "скорость = v");
        assert_eq!(latex_to_unicode(r"\mathbb{R}"), "ℝ");
    }

    #[test]
    fn spacing_commands_become_space() {
        assert_eq!(latex_to_unicode(r"a\,b"), "a b");
        assert_eq!(latex_to_unicode(r"a\;b"), "a b");
        assert_eq!(latex_to_unicode(r"a\!b"), "ab");
    }

    #[test]
    fn extended_symbols() {
        assert_eq!(latex_to_unicode(r"a \therefore b"), "a ∴ b");
        assert_eq!(latex_to_unicode(r"x \longrightarrow y"), "x ⟶ y");
        assert_eq!(latex_to_unicode(r"p \ll q"), "p ≪ q");
    }

    #[test]
    fn function_names_render_as_words() {
        assert_eq!(latex_to_unicode(r"O(n \log n)"), "O(n log n)");
        assert_eq!(latex_to_unicode(r"\sin x + \cos x"), "sin x + cos x");
        assert_eq!(latex_to_unicode(r"\lim f"), "lim f");
        assert_eq!(latex_to_unicode(r"\ln(x) \exp(y)"), "ln(x) exp(y)");
    }

    #[test]
    fn bracket_size_modifiers_stripped() {
        assert_eq!(latex_to_unicode(r"\left( x \right)"), "( x )");
        assert_eq!(latex_to_unicode(r"\bigl[ a \bigr]"), "[ a ]");
    }

    #[test]
    fn accents_show_content() {
        assert_eq!(latex_to_unicode(r"\vec{v}"), "v");
        assert_eq!(latex_to_unicode(r"\overline{AB}"), "AB");
        assert_eq!(latex_to_unicode(r"\hat{x} + \bar{y}"), "x + y");
    }

    #[test]
    fn pmod_and_bmod() {
        assert_eq!(latex_to_unicode(r"a \bmod n"), "a mod n");
        assert_eq!(latex_to_unicode(r"x \pmod{7}"), "x (mod 7)");
    }

    #[test]
    fn normalize_paren_and_bracket_delimiters() {
        assert_eq!(normalize_delimiters(r"итог \(x^2\) тут"), "итог $x^2$ тут");
        assert_eq!(normalize_delimiters(r"\[a+b\]"), "$$a+b$$");
    }

    #[test]
    fn normalize_skips_code_spans() {
        // inside a code span `\(` is left alone
        assert_eq!(normalize_delimiters(r"`\(x\)`"), r"`\(x\)`");
        assert_eq!(
            normalize_delimiters("```\n\\(x\\)\n```"),
            "```\n\\(x\\)\n```"
        );
    }

    #[test]
    fn render_converts_math_and_strips_dollars() {
        let collected = rendered_text(r"Формула: $x^2 + \alpha$");
        assert!(collected.contains("x²"));
        assert!(collected.contains('α'));
        // the delimiter dollars are stripped by the parser
        assert!(!collected.contains('$'));
    }

    #[test]
    fn render_leaves_bare_commands_outside_math() {
        // outside $…$, commands are left alone (chosen semantics)
        let collected = rendered_text(r"стрелка \rightarrow без формулы");
        assert!(collected.contains(r"\rightarrow"));
    }

    #[test]
    fn render_paren_delimiters_become_math() {
        let collected = rendered_text(r"путь \(\alpha \to \beta\) готов");
        assert!(collected.contains("α → β"));
        assert!(!collected.contains('$'));
    }

    #[test]
    fn unknown_brace_command_keeps_braces() {
        // An unrecognized command with a group doesn't glue to its argument
        // (regression: \boxed{x+1} → \boxedx+1). The content inside is still
        // converted.
        assert_eq!(latex_to_unicode(r"\boxed{x+1}"), r"\boxed{x+1}");
        assert_eq!(latex_to_unicode(r"\boxed{\alpha}"), r"\boxed{α}");
        assert_eq!(latex_to_unicode(r"\op{a}{b}"), r"\op{a}{b}");
    }

    #[test]
    fn frac_family_aliases() {
        assert_eq!(latex_to_unicode(r"\dfrac{a}{b}"), "a/b");
        assert_eq!(latex_to_unicode(r"\tfrac{1}{2}"), "1/2");
        assert_eq!(latex_to_unicode(r"\cfrac{x}{y}"), "x/y");
    }

    #[test]
    fn binom_coefficient() {
        assert_eq!(latex_to_unicode(r"\binom{n}{k}"), "C(n, k)");
        assert_eq!(latex_to_unicode(r"\binom{\alpha}{2}"), "C(α, 2)");
    }

    #[test]
    fn sqrt_with_index() {
        assert_eq!(latex_to_unicode(r"\sqrt[3]{x}"), "∛(x)");
        assert_eq!(latex_to_unicode(r"\sqrt[4]{y}"), "∜(y)");
        assert_eq!(latex_to_unicode(r"\sqrt[n]{x}"), "ⁿ√(x)");
        // an index with no superscript counterpart ('q' isn't one) —
        // fallback with square brackets
        assert_eq!(latex_to_unicode(r"\sqrt[q]{x}"), "√[q](x)");
        // a "bare" \sqrt with no index still works
        assert_eq!(latex_to_unicode(r"\sqrt{x}"), "√(x)");
    }

    #[test]
    fn left_right_dot_delimiter_eaten() {
        assert_eq!(latex_to_unicode(r"\left. x \right."), "x");
        // the modifier is stripped, the delimiter bracket stays
        assert_eq!(latex_to_unicode(r"\left( x \right)"), "( x )");
    }

    #[test]
    fn overset_underset_take_base() {
        assert_eq!(latex_to_unicode(r"\overset{def}{=}"), "=");
        assert_eq!(latex_to_unicode(r"\underset{n}{\min}"), "min");
        assert_eq!(latex_to_unicode(r"\stackrel{?}{=}"), "=");
    }

    #[test]
    fn more_text_wrappers() {
        assert_eq!(latex_to_unicode(r"\texttt{code}"), "code");
        assert_eq!(latex_to_unicode(r"\emph{важно}"), "важно");
        assert_eq!(latex_to_unicode(r"\underbrace{a+b}"), "a+b");
    }

    #[test]
    fn extended_symbol_table() {
        assert_eq!(latex_to_unicode(r"\langle x \rangle"), "⟨ x ⟩");
        assert_eq!(latex_to_unicode(r"\lfloor x \rfloor"), "⌊ x ⌋");
        assert_eq!(latex_to_unicode(r"a \cong b"), "a ≅ b");
        assert_eq!(latex_to_unicode(r"\Gamma \vdash x"), "Γ ⊢ x");
        assert_eq!(latex_to_unicode(r"A \setminus B"), "A ∖ B");
        assert_eq!(latex_to_unicode(r"x \hookrightarrow y"), "x ↪ y");
        assert_eq!(latex_to_unicode(r"\vartheta + \varpi"), "ϑ + ϖ");
        assert_eq!(latex_to_unicode(r"\bigcup_i A"), "⋃ᵢ A");
    }

    #[test]
    fn backslash_command_is_literal() {
        assert_eq!(latex_to_unicode(r"a \backslash b"), r"a \ b");
    }

    #[test]
    fn normalize_skips_tilde_fence() {
        // inside a ~~~ fence `\(` is left alone (avoids corrupting code)
        assert_eq!(
            normalize_delimiters("~~~\n\\(x\\)\n~~~"),
            "~~~\n\\(x\\)\n~~~"
        );
        // ~~strikethrough~~ (a run < 3) doesn't count as a fence — the formula
        // after it normalizes
        assert_eq!(normalize_delimiters(r"~~s~~ \(y\)"), r"~~s~~ $y$");
    }

    #[test]
    fn normalize_unclosed_short_backtick_continues() {
        // a lone ` (an unclosed short run) is a literal, the formula after it normalizes
        assert_eq!(normalize_delimiters(r"a ` b \(x\)"), r"a ` b $x$");
        // an unclosed fence ``` — verbatim to the end (normal while streaming)
        assert_eq!(normalize_delimiters("```\n\\(x\\)"), "```\n\\(x\\)");
    }

    #[test]
    fn deep_nesting_does_not_overflow_stack() {
        // pathological nesting doesn't crash the stack (the MAX_BRACE_DEPTH ceiling)
        let deep = "\\sqrt{".repeat(5000) + "x" + &"}".repeat(5000);
        let _ = latex_to_unicode(&deep);
    }

    #[test]
    fn display_environment_lays_out_rows() {
        // \begin{aligned}…\end{aligned} with \\ and & — row by row, alignment removed
        let r = latex_to_unicode_display(r"\begin{aligned} x &= y \\ z &= w \end{aligned}");
        assert_eq!(r, "x = y\nz = w");
    }

    #[test]
    fn display_cases_and_matrix() {
        assert_eq!(
            latex_to_unicode_display(r"\begin{cases} a & x>0 \\ b & x<0 \end{cases}"),
            "a x>0\nb x<0"
        );
        assert_eq!(
            latex_to_unicode_display(r"\begin{pmatrix} 1 & 2 \\ 3 & 4 \end{pmatrix}"),
            "1 2\n3 4"
        );
    }

    #[test]
    fn inline_double_backslash_is_semicolon() {
        assert_eq!(latex_to_unicode(r"a \\ b"), "a; b");
        // an environment in inline also lays out via "; "
        assert_eq!(
            latex_to_unicode(r"\begin{aligned} x &= 1 \\ y &= 2 \end{aligned}"),
            "x = 1; y = 2"
        );
    }

    #[test]
    fn environment_helpers_stripped() {
        // \label{…}, \\[4pt], \hline, \notag, \& (literal)
        assert_eq!(
            latex_to_unicode_display(r"a = b \label{eq:1} \\[4pt] c = d"),
            "a = b\nc = d"
        );
        assert_eq!(latex_to_unicode_display(r"x \hline y"), "x y");
        assert_eq!(latex_to_unicode(r"P \& Q"), "P & Q");
    }

    #[test]
    fn display_math_multiline_source_without_break() {
        // real source line breaks (with no \\) don't break the formula
        assert_eq!(latex_to_unicode_display("x = y +\nz"), "x = y + z");
    }
}

//! Markdown — code-block highlighting (syntect: syntax + theme from the
//! palette). Part of module [`super`]; split out of the markdown.rs monolith
//! (see docs/history/refactoring-god-objects.md, stage 6).

use super::*;

// ---------- writer: pulldown events → lines ----------

/// syntect's bundled syntaxes **plus** the grammars vendored in `syntaxes/`
/// (see its `SOURCES.md`), assembled into one dump at build time by
/// [`build.rs`](../../../build.rs).
///
/// The dump is uncompressed on purpose — measured at 0.55 ms to load against
/// 4.1 ms for the compressed form, for 45 KiB more in the binary; that is the
/// same trade syntect makes for its own defaults, and it keeps the first code
/// block rendered as cheap as it was before the grammars were added
/// (docs/history/vendored-syntaxes.md §2.3). Assembling the set here instead would cost
/// ~130 ms on that first block.
pub(super) static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(|| {
    syntect::dumps::from_uncompressed_data(include_bytes!(concat!(
        env!("OUT_DIR"),
        "/syntaxes.packdump"
    )))
    .expect("the syntax dump built by build.rs must load")
});

/// Resolves a code block's language label (` ```csharp `) into a syntect
/// syntax.
///
/// `SyntaxSet::find_syntax_by_token` in the default Sublime set matches a
/// label either against a file extension (`rs`, `cs`) or against a syntax's
/// **name**, case-insensitively (`Rust`, `C#`). So `rust` is found by a lucky
/// match with the name `Rust`, while common model labels like
/// `csharp`/`c++`/`golang` match neither the name (`C#`, `C++`, `Go`) nor the
/// extension — and the code stays unhighlighted. The [`canonical_lang`] table
/// maps such aliases to a token the set recognizes; on a miss we try the
/// original label (maybe it's already a valid extension/name absent from the
/// table).
pub(super) fn resolve_syntax(lang: &str) -> Option<&'static SyntaxReference> {
    if lang.is_empty() {
        return None;
    }
    let canonical = canonical_lang(lang);
    SYNTAX_SET
        .find_syntax_by_token(canonical)
        .or_else(|| SYNTAX_SET.find_syntax_by_token(lang))
}

/// Reduces a language alias to a token (extension/name) the default syntect
/// set understands. The label's case is ignored. An unmapped label is
/// returned as-is (`find_syntax_by_token` itself tries to recognize it).
///
/// Keys are typical labels Gemma/Qwen/Claude use to mark code blocks. **All
/// targets are checked against [`SYNTAX_SET`]** — mapping to a syntax that
/// isn't there is pointless. Labels that already resolve (`rust`, `python`,
/// `go`, `js`, `java`, `ruby`, `php`, `sql`, `html`, `css`, `json`, `yaml`,
/// `bash`, `c`, `c++`, `c#`/`cs`, …) aren't listed here — nor are the ones a
/// **vendored** grammar now answers by its own `file_extensions` (`zig`,
/// `toml`, `ts`, `swift`, `kt`, `dockerfile`, `scss`, …, see
/// `syntaxes/SOURCES.md`).
///
/// **An alias shadows a real grammar**, since `resolve_syntax` tries the
/// canonical token first: while `zig → rs` was in this table, the vendored Zig
/// grammar was never reached and `zig` still highlighted as Rust (measured —
/// docs/history/vendored-syntaxes.md §2.4). So an approximation must be deleted the
/// moment its language gets a grammar of its own.
pub(super) fn canonical_lang(lang: &str) -> &str {
    match lang.trim().to_ascii_lowercase().as_str() {
        // --- direct aliases: the target is in the set, but the label doesn't match it ---
        "csharp" | "cs-script" | "dotnet" => "cs", // the name "C#" ≠ "csharp"
        "cpp" | "cplusplus" | "cxx" | "cc" => "c++", // the name "C++" ≠ "cpp"
        "objc" | "objective-c" | "objectivec" | "obj-c" => "m", // the name "Objective-C"
        "objcpp" | "objc++" | "objective-c++" => "mm", // the name "Objective-C++"
        "golang" => "go",
        "rustlang" => "rs",
        "python3" | "py3" | "python2" => "py",
        "node" | "nodejs" => "js",
        "shell" | "sh" | "zsh" | "console" | "shell-session" | "shellsession" => "bash",
        "yml" | "yaml-frontmatter" | "frontmatter" => "yaml",
        "rlang" => "r",
        // --- labels a vendored grammar answers under a different spelling ---
        "docker" | "containerfile" => "dockerfile",
        "pwsh" => "ps1", // the grammar answers "powershell"/"ps1", not "pwsh"
        // HCL is Terraform's own language; its upstream grammar is an
        // `extends:` stub syntect cannot load, so the label goes to Terraform.
        "hcl" | "tfvars" => "tf",
        "proto3" => "protobuf",
        // JSON with comments: the bundled JSON grammar already highlights `//`
        // and `/* */` as comments (measured), so this is exact, not an
        // approximation — only the label is missing.
        "jsonc" | "json5" => "json",
        // --- approximations: the language isn't in the set, take a close relative ---
        // Partial highlighting from a related grammar beats gray text.
        "jsx" => "js",
        "tsx" => "ts", // TSX is TypeScript plus JSX; the TS grammar covers most
        // V on the Go grammar. No grammar is vendored because the only
        // `.sublime-syntax` for V that exists carries **no licence at all**
        // (see syntaxes/SOURCES.md). Go is the measured best of go/rust/c: V is
        // Go-inspired and shares `:=`, `import`, `struct`, the primitive type
        // names, single-quoted strings and `//` comments. Rust catches
        // `pub`/`fn`/`mut` but reads `'` as a lifetime and **mangles V's
        // default string form**, which is worse than leaving those three plain.
        "v" | "vlang" => "go",
        other => {
            // Return an unmapped label as-is; borrowed from the original
            // string, so we return a slice of `lang`, not a temporary
            // lowercase buffer.
            let _ = other;
            lang.trim()
        }
    }
}

/// Highlights one line of code `line` (may carry a trailing `\n`) into visual
/// rows. On a syntect/`ansi-to-tui` error, degrades to a flat line — **the
/// text isn't lost** (previously `Writer::text` silently dropped the line on
/// a `highlight_line` error). Shared by `Writer::text` and
/// [`super::highlight_code`].
pub(super) fn highlight_line_or_plain(
    hl: &mut HighlightLines<'static>,
    line: &str,
) -> Vec<Line<'static>> {
    let plain = || vec![Line::from(line.trim_end_matches('\n').to_string())];
    let Ok(parts) = hl.highlight_line(line, &SYNTAX_SET) else {
        return plain();
    };
    match as_24_bit_terminal_escaped(&parts, false).into_text() {
        Ok(text) => text.lines,
        Err(_) => plain(),
    }
}

/// A blank column kept along the block's right edge, so the text doesn't run
/// into the background's hard edge. Left alone on purpose: the code's own
/// indentation stays aligned with the fence markers and with the surrounding
/// prose.
pub(super) const CODE_RIGHT_PAD: usize = 1;

/// Lays an already-emitted code block out as a solid rectangle: its rows are
/// wrapped to the panel width and padded with spaces up to the block's own
/// width — the widest row plus [`CODE_RIGHT_PAD`], capped at `width`. `start`
/// is the index in `lines` of the block's opening fence.
///
/// Without this the block's background ([`super::code_style`] — `REVERSED`,
/// so a space paints a solid cell) follows the ragged right edge of the text,
/// and the block reads as a stack of bars of differing length instead of one
/// panel. The width is the **block's own**, not the panel's — the same rule
/// tables follow: laid out to their content, capped by the panel.
///
/// Rows are wrapped **here** rather than left to the feed: a row longer than
/// the panel would be split later and its tail would stay ragged inside an
/// otherwise rectangular block. The feed's re-wrap then becomes a no-op (every
/// row is ≤ `width`, like a table's). They are wrapped to `width -
/// CODE_RIGHT_PAD` so that the blank column exists even for a block whose text
/// fills the panel — otherwise the cap would eat it exactly where the edge is
/// tightest.
///
/// Only the unhighlighted path calls this: a syntect-highlighted block carries
/// no background at all (the pipeline only transfers foreground color, see
/// [`build_code_theme`]), so there would be no rectangle to square off.
pub(super) fn pad_code_block(lines: &mut Vec<Line<'static>>, start: usize, width: usize) {
    let text_w = width.saturating_sub(CODE_RIGHT_PAD);
    let rows: Vec<Line<'static>> = lines
        .split_off(start)
        .into_iter()
        .flat_map(|line| wrap::wrap_line(&line, text_w))
        // Trailing spaces carry no meaning in a code block (leading ones —
        // indentation — do), and `wrap_ranges` "spills" a word-boundary space
        // past the row's edge; both would inflate the measured width and defeat
        // the padding. Same reasoning as `trim_row_trailing_ws` in a table cell.
        .map(trim_row_trailing_ws)
        .collect();
    let block = rows
        .iter()
        .map(|l| cell_width(&l.spans) + CODE_RIGHT_PAD)
        .max()
        .unwrap_or(0)
        .min(width);
    lines.extend(rows.into_iter().map(|mut row| {
        let pad = block.saturating_sub(cell_width(&row.spans));
        if pad > 0 {
            // A raw span: the background comes from the line style, which the
            // padding picks up on its own — and, unlike the fence spans, it
            // stays free of their `DIM`, so the rectangle's top and bottom
            // edges are the same shade as its body.
            row.spans.push(Span::raw(" ".repeat(pad)));
        }
        row
    }));
}

/// Builds a syntect code-highlighting theme from the semantic [`Palette`],
/// mapping syntax scopes to theme roles: keywords → `accent`, strings →
/// `success`, numbers/constants → `warning`, functions → `user`, types →
/// `assistant`, tags → `accent`. "Default" text and comments are set to an
/// absolute gray, light on a dark background and dark on light
/// (`palette.dark`) — so highlighting tracks the app's theme rather than
/// living in its "own palette" (ADR 0003).
///
/// The render pipeline (`as_24_bit_terminal_escaped(.., false)`) only carries
/// **foreground** color, so background/bold/italic aren't set in the theme.
pub(super) fn build_code_theme(palette: &Palette) -> Theme {
    // Grays with no adaptable ANSI counterpart — chosen by background lightness.
    let (default_fg, comment) = if palette.dark {
        (gray(212), gray(128))
    } else {
        (gray(40), gray(110))
    };

    let settings = ThemeSettings {
        foreground: Some(default_fg),
        ..Default::default()
    };

    // A list (scope selector → role color). The most specific selector wins
    // (syntect picks by "match strength"), the vector's order doesn't matter.
    let scopes = vec![
        scope_item("comment", comment),
        scope_item(
            "keyword, storage, keyword.operator, keyword.control",
            to_syn(palette.accent),
        ),
        scope_item(
            "string, string.quoted, string.regexp",
            to_syn(palette.success),
        ),
        scope_item(
            "constant.numeric, constant.language, constant.character, constant.character.escape",
            to_syn(palette.warning),
        ),
        scope_item(
            "entity.name.function, support.function, meta.function-call",
            to_syn(palette.user),
        ),
        scope_item(
            "entity.name.type, entity.name.class, support.type, support.class, entity.other.inherited-class",
            to_syn(palette.assistant),
        ),
        scope_item(
            "entity.name.tag, punctuation.definition.tag",
            to_syn(palette.accent),
        ),
    ];

    Theme {
        name: Some("mindfork".to_string()),
        author: None,
        settings,
        scopes,
    }
}

/// An opaque shade of gray `v` across all channels.
pub(super) fn gray(v: u8) -> SynColor {
    SynColor {
        r: v,
        g: v,
        b: v,
        a: 255,
    }
}

/// One theme item: scope selector(s) → foreground color.
pub(super) fn scope_item(selector: &str, color: SynColor) -> ThemeItem {
    ThemeItem {
        scope: ScopeSelectors::from_str(selector).unwrap_or_default(),
        style: StyleModifier {
            foreground: Some(color),
            background: None,
            font_style: None,
        },
    }
}

/// Converts a ratatui color into a syntect RGB color. Named ANSI colors
/// (`Auto`/`Dark` use the named, terminal-adaptable palette) are converted to
/// standard RGB (the Campbell palette — Windows Terminal's default): code
/// highlighting emits a 24-bit color regardless, so there's no other way.
/// `Rgb` is copied as-is.
pub(super) fn to_syn(color: Color) -> SynColor {
    let (r, g, b) = match color {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (12, 12, 12),
        Color::Red => (197, 15, 31),
        Color::Green => (19, 161, 14),
        Color::Yellow => (193, 156, 0),
        Color::Blue => (0, 55, 218),
        Color::Magenta => (136, 23, 152),
        Color::Cyan => (58, 150, 221),
        Color::Gray => (204, 204, 204),
        Color::DarkGray => (118, 118, 118),
        Color::LightRed => (231, 72, 86),
        Color::LightGreen => (22, 198, 12),
        Color::LightYellow => (249, 241, 165),
        Color::LightBlue => (59, 120, 255),
        Color::LightMagenta => (180, 0, 158),
        Color::LightCyan => (97, 214, 214),
        Color::White => (242, 242, 242),
        Color::Indexed(_) | Color::Reset => (204, 204, 204),
    };
    SynColor { r, g, b, a: 255 }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::*;
    use super::*;
    use crate::shared::config::Theme;

    /// Language labels models tag code blocks with must resolve to a syntax —
    /// otherwise the block stays unhighlighted (there was a bug with
    /// ` ```csharp `: the token matched neither the name `C#` nor the
    /// extension `cs`).
    #[test]
    fn language_aliases_resolve_to_syntax() {
        for (label, expect_name) in [
            ("rust", "Rust"),
            ("csharp", "C#"),
            ("c#", "C#"),
            ("CSharp", "C#"),
            ("cs", "C#"),
            ("cpp", "C++"),
            ("c++", "C++"),
            ("golang", "Go"),
            ("objc", "Objective-C"),
            ("objective-c++", "Objective-C++"),
            ("python3", "Python"),
            ("nodejs", "JavaScript"),
            ("shell", "Bourne Again Shell (bash)"),
            ("yml", "YAML"),
            // labels a vendored grammar answers under a different spelling
            ("docker", "Dockerfile"),
            ("pwsh", "PowerShell"),
            ("hcl", "Terraform"),
            ("proto3", "Protocol Buffer"),
            ("jsonc", "JSON"),
            ("json5", "JSON"),
            // approximations: the language isn't in the set → a close grammar
            ("jsx", "JavaScript"),
            ("tsx", "TypeScript"),
            ("v", "Go"),
            ("vlang", "Go"),
        ] {
            let syntax = resolve_syntax(label)
                .unwrap_or_else(|| panic!("label {label:?} doesn't resolve to a syntax"));
            assert_eq!(syntax.name, expect_name, "label {label:?}");
        }
    }

    /// Every vendored grammar (`syntaxes/`, see its `SOURCES.md`) is reachable
    /// by the label a model would write, **and by its own name** — no alias
    /// table entry needed. A grammar that stopped resolving would otherwise sit
    /// in the binary doing nothing, which is exactly the failure the vendoring
    /// exists to end.
    #[test]
    fn vendored_grammars_resolve_by_their_own_label() {
        for (label, expect_name) in [
            ("zig", "Zig"),
            ("Zig", "Zig"),
            ("typescript", "TypeScript"),
            ("ts", "TypeScript"),
            ("toml", "TOML"),
            ("dockerfile", "Dockerfile"),
            ("powershell", "PowerShell"),
            ("ps1", "PowerShell"),
            ("swift", "Swift"),
            ("kotlin", "Kotlin"),
            ("kt", "Kotlin"),
            ("scss", "SCSS"),
            ("sass", "Sass"),
            ("graphql", "GraphQL"),
            ("terraform", "Terraform"),
            ("tf", "Terraform"),
            ("elixir", "Elixir"),
            ("ex", "Elixir"),
            ("solidity", "Solidity"),
            ("julia", "Julia"),
            ("jl", "Julia"),
            ("nix", "Nix"),
            ("dart", "Dart"),
            ("protobuf", "Protocol Buffer"),
            ("proto", "Protocol Buffer"),
            ("cmake", "CMake"),
            ("nginx", "nginx"),
            // Vue's grammar calls itself "Vue Component"; the label still
            // reaches it through the file extension.
            ("vue", "Vue Component"),
            ("svelte", "Svelte"),
            ("nim", "Nim"),
        ] {
            let syntax = resolve_syntax(label)
                .unwrap_or_else(|| panic!("label {label:?} doesn't resolve to a syntax"));
            assert_eq!(syntax.name, expect_name, "label {label:?}");
        }
    }

    /// **Every** syntax in the set can actually highlight, not merely load.
    ///
    /// The two are different questions, and the gap is where a vendored grammar
    /// can hurt: Vue and Svelte embed other languages by scope
    /// (`source.js`/`text.html.basic`/…), and a reference syntect cannot
    /// resolve at link time only shows up when something is parsed through it.
    /// A panic there would kill the app — the panic hook restores the terminal
    /// and exits, and we deliberately do not `catch_unwind` (see the mermaid
    /// module). An `Err` is fine: `highlight_line_or_plain` degrades to flat
    /// text.
    #[test]
    fn every_syntax_can_highlight_without_panicking() {
        // Deliberately mixed: markup, script, style, strings and comments, so
        // an embedded-scope grammar actually reaches its embedded contexts.
        const SNIPPET: &str = "<div class=\"a\">{{ x }}</div>\n\
                               <script>const a = 1; // note\n</script>\n\
                               <style>.a { color: red; }</style>\n\
                               fn main() { let s = \"текст\"; }\n";
        let theme = code_theme(&Palette::for_theme(Theme::Dark));
        for syntax in SYNTAX_SET.syntaxes() {
            let mut hl = HighlightLines::new(syntax, theme);
            for line in LinesWithEndings::from(SNIPPET) {
                // Errors are acceptable (the renderer falls back to plain
                // text); a panic is not, and is what this test exists to catch.
                let _ = hl.highlight_line(line, &SYNTAX_SET);
            }
        }
    }

    /// The dump `build.rs` embeds really is the bundled set **plus** the
    /// vendored grammars — a guard against the build step silently degrading to
    /// syntect's defaults, which would leave every new label unhighlighted
    /// again, quietly.
    #[test]
    fn dump_carries_the_vendored_grammars() {
        let vendored = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/syntaxes"))
            .expect("the syntaxes/ directory")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "sublime-syntax"))
            .count();
        assert!(
            vendored >= 22,
            "expected the vendored grammars, got {vendored}"
        );
        assert_eq!(
            SYNTAX_SET.syntaxes().len(),
            75 + vendored,
            "the dump should carry syntect's 75 bundled syntaxes plus every vendored one"
        );
    }

    /// An empty/unknown label doesn't panic and doesn't resolve.
    #[test]
    fn empty_and_unknown_language_do_not_resolve() {
        assert!(resolve_syntax("").is_none());
        assert!(resolve_syntax("совсем-не-язык-42").is_none());
    }

    #[test]
    fn code_highlight_is_colored() {
        // Highlighting sets foreground colors (not plain text).
        let colors = fg_colors(CODE_MD, &Palette::for_theme(Theme::Dark));
        assert!(
            colors.iter().any(|c| matches!(c, Color::Rgb(..))),
            "expected RGB code-highlight colors"
        );
    }

    #[test]
    fn code_highlight_follows_theme() {
        // The same code highlighting in the dark and light theme gives
        // different colors — so highlighting tracks the theme, not living in
        // its "own palette".
        let dark = fg_colors(CODE_MD, &Palette::for_theme(Theme::Dark));
        let light = fg_colors(CODE_MD, &Palette::for_theme(Theme::Light));
        assert_ne!(dark, light, "code highlighting doesn't depend on the theme");
    }

    #[test]
    fn highlight_code_has_no_fences_and_is_colored() {
        // Helper for tool cards: highlighting with no enclosing ```, with RGB colors.
        let lines = highlight_code("fn main() {}", "rust", &Palette::for_theme(Theme::Dark));
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("fn main"), "code content: {joined}");
        assert!(
            !joined.contains("```"),
            "there should be no fences: {joined}"
        );
        assert!(
            lines
                .iter()
                .flat_map(|l| l.spans.iter())
                .any(|s| matches!(s.style.fg, Some(Color::Rgb(..)))),
            "expected RGB highlighting"
        );
    }

    #[test]
    fn highlight_code_unknown_lang_falls_back_to_plain() {
        // An unrecognized language → unhighlighted lines (no panic, text intact).
        let lines = highlight_code("a\nb", "нет-такого-языка", &Palette::default());
        assert_eq!(lines.len(), 2);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert_eq!(joined, "ab");
    }

    /// An unhighlighted (no language) fenced block: the content starts on the
    /// line under the opening `​```​`, not glued onto it (regression: the
    /// first line used to get appended to the fence line, `i==0` +
    /// `needs_newline==false`).
    #[test]
    fn plain_code_block_content_not_glued_to_fence() {
        let md = "```\nX_ij = 1, тест\nE = 2/(j-i+1)\n```";
        // Rows are padded to the block's rectangle (see `pad_code_block`), so
        // compare the text, not the trailing background.
        let lines: Vec<String> = block_rows(md, 80)
            .iter()
            .map(|l| l.trim_end().to_string())
            .collect();
        // The opening fence — on its own line, with no content.
        assert_eq!(lines[0], "```", "content glued to the fence: {lines:?}");
        assert_eq!(lines[1], "X_ij = 1, тест");
        assert_eq!(lines[2], "E = 2/(j-i+1)");
        assert_eq!(lines[3], "```");
    }

    /// Text of each rendered row.
    fn block_rows(md: &str, width: usize) -> Vec<String> {
        render(md, width, &Palette::default())
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    /// Display width of each rendered row.
    fn block_widths(md: &str, width: usize) -> Vec<usize> {
        render(md, width, &Palette::default())
            .lines
            .iter()
            .map(|l| cell_width(&l.spans))
            .collect()
    }

    /// The block's background is a solid rectangle: every row — fences
    /// included — is padded to one width, and that width is the block's own
    /// (the widest line), not the panel's. Before this the background followed
    /// the ragged right edge of the text.
    #[test]
    fn plain_code_block_is_a_solid_rectangle() {
        let md = "```text\nкороткая\nсамая длинная строка блока\nx\n```";
        let widths = block_widths(md, 80);
        let expected =
            wrap::display_width(&"самая длинная строка блока".chars().collect::<Vec<_>>())
                + CODE_RIGHT_PAD;
        assert!(
            widths.iter().all(|&w| w == expected),
            "rows are not one width ({expected} expected): {widths:?}"
        );
        // The rectangle is sized to the content, not stretched across the panel
        // (the rule tables follow).
        assert!(expected < 80, "the block should not fill the panel");
        // …and it really is a background, not just padding: the block's rows
        // carry the reverse-video line style that paints those spaces.
        for line in render(md, 80, &Palette::default()).lines {
            assert!(
                line.style.add_modifier.contains(Modifier::REVERSED),
                "a block row lost its background: {line:?}"
            );
        }
    }

    /// A blank line inside the block is a full row of the rectangle, not a gap
    /// in it.
    #[test]
    fn blank_line_inside_a_code_block_is_filled() {
        let widths = block_widths("```\naaaa bbbb\n\ncccc\n```", 80);
        let expected = 9 + CODE_RIGHT_PAD;
        assert!(
            widths.iter().all(|&w| w == expected),
            "a blank row broke the rectangle: {widths:?}"
        );
    }

    /// The rectangle keeps a blank column along its right edge, so the text
    /// doesn't run into the background's hard edge. Checked at a comfortable
    /// panel and at the awkward one — a panel exactly as wide as the block's
    /// longest line, where the width cap would otherwise eat that very column
    /// (the line wraps instead, and the column survives).
    #[test]
    fn rectangle_keeps_a_blank_column_on_the_right() {
        let longest = "самая длинная строка блока";
        let natural = wrap::display_width(&longest.chars().collect::<Vec<_>>());
        for w in [80usize, natural] {
            let rows = block_rows(&format!("```text\nx\n{longest}\n```"), w);
            for row in &rows {
                assert!(
                    row.ends_with(' '),
                    "the right edge has no blank column at panel {w}: {rows:?}"
                );
            }
        }
        // Exactly one column, not a margin: the rectangle stays sized to its
        // content.
        let rows = block_rows(&format!("```text\nx\n{longest}\n```"), 80);
        let widest = rows
            .iter()
            .find(|r| r.contains(longest))
            .expect("the longest line");
        assert_eq!(
            widest.chars().rev().take_while(|c| *c == ' ').count(),
            CODE_RIGHT_PAD,
            "expected exactly one blank column: {widest:?}"
        );
    }

    /// A line longer than the panel is wrapped **by the renderer**, so the
    /// rectangle stays square instead of leaving a ragged tail row for the
    /// feed's re-wrap to produce; and no row ever exceeds the panel.
    #[test]
    fn long_code_line_wraps_into_the_rectangle() {
        let md = format!("```\n{}\n```", "слово ".repeat(30));
        for w in [20usize, 32, 40, 60] {
            let widths = block_widths(&md, w);
            let first = widths[0];
            assert!(
                first <= w && widths.iter().all(|&x| x == first),
                "at panel {w} the rectangle came out ragged: {widths:?}"
            );
            // The wrap really happened — a 180-column line did not survive whole.
            assert!(widths.len() > 3, "the long line did not wrap: {widths:?}");
        }
    }

    /// The fence's `DIM` must not reach the padding, or the rectangle's top and
    /// bottom edges would be a different shade from its body.
    #[test]
    fn rectangle_padding_is_not_dimmed() {
        let md = "```text\nсамая длинная строка блока\n```";
        let fence = render(md, 80, &Palette::default()).lines.remove(0);
        let text = &fence.spans[0];
        let pad = fence.spans.last().expect("the fence row is padded");
        assert!(text.content.starts_with("```"));
        assert!(
            text.style.add_modifier.contains(Modifier::DIM),
            "the fence text should stay dim: {text:?}"
        );
        assert!(
            pad.content.trim().is_empty() && !pad.style.add_modifier.contains(Modifier::DIM),
            "the padding must carry the plain background: {pad:?}"
        );
    }

    /// A ` ```zig ` block takes the highlighted path instead of dropping into
    /// the unhighlighted rectangle — the user-visible symptom that started the
    /// track: Zig is absent from syntect's bundled set, so the token resolved
    /// to nothing and the block came out as flat text on a reverse-video
    /// background. It is now the vendored Zig grammar that answers, not an
    /// approximation (`vendored_grammars_resolve_by_their_own_label` pins
    /// which).
    #[test]
    fn zig_block_is_highlighted() {
        let md = "```zig\nconst memory = try allocator.alloc(u8, 1024);\n```";
        let lines = render(md, 80, &Palette::for_theme(Theme::Dark)).lines;
        assert!(
            lines
                .iter()
                .flat_map(|l| l.spans.iter())
                .any(|s| matches!(s.style.fg, Some(Color::Rgb(..)))),
            "the zig block is not highlighted: {lines:?}"
        );
        assert!(
            !lines
                .iter()
                .any(|l| l.style.add_modifier.contains(Modifier::REVERSED)),
            "the zig block went down the unhighlighted path: {lines:?}"
        );
    }

    /// A **highlighted** block is deliberately left alone: syntect only carries
    /// foreground color (see `build_code_theme`), so such a block has no
    /// background — there is no rectangle to square off, and padding would be
    /// invisible weight.
    #[test]
    fn highlighted_block_is_left_ragged() {
        let widths = block_widths("```rust\nfn main() {\n    let x = 1;\n}\n```", 80);
        assert!(
            widths
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                > 1,
            "a highlighted block should keep its natural row widths: {widths:?}"
        );
    }
}

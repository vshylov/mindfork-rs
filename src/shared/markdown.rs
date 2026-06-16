//! Рендер markdown в `ratatui::Text` + lёгкая unicode-аппроксимация LaTeX.
//! См. spec §11.4 и docs/decisions/0003-own-markdown-renderer.md.
//!
//! Markdown парсим напрямую через `pulldown-cmark` собственным «писателем»
//! (`Writer`): markdown → `Text`. Это даёт темизацию через [`Palette`] (а не
//! захардкоженные цвета), а позже — нативные таблицы и math-события. Подсветка
//! блоков кода — `syntect` + `ansi-to-tui`. Полный LaTeX и рендер в изображение
//! сознательно НЕ реализуются.

use std::sync::LazyLock;

use ansi_to_tui::IntoText;
use pulldown_cmark::{CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::{LinesWithEndings, as_24_bit_terminal_escaped};

use crate::shared::theme::Palette;

/// Рендерит markdown-строку в владеющий [`Text`] (готовый к показу/кэшированию).
///
/// `width` — ширина панели в колонках (используется для раскладки таблиц).
/// `palette` задаёт цвета (заголовки/ссылки/код/цитаты) под текущую тему.
/// Перед парсингом применяется [`latex_to_unicode`] — стрелки и простые формулы
/// становятся читаемыми. Возвращается `'static`-`Text` (содержимое скопировано),
/// поэтому результат можно хранить в состоянии UI.
pub fn render(input: &str, width: usize, palette: &Palette) -> Text<'static> {
    // Ширина пригодится для раскладки таблиц (следующий этап); сейчас не нужна.
    let _ = width;
    let approximated = latex_to_unicode(input);
    let mut parse_opts = Options::empty();
    parse_opts.insert(Options::ENABLE_STRIKETHROUGH);
    parse_opts.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(&approximated, parse_opts);
    let mut writer = Writer::new(*palette);
    writer.run(parser);
    Text::from(writer.lines)
}

// ---------- стили из палитры ----------

/// Стиль заголовка уровня `level` (1 — крупнейший).
fn heading_style(level: u8, palette: &Palette) -> Style {
    let base = Style::new().fg(palette.accent).add_modifier(Modifier::BOLD);
    match level {
        1 => base.add_modifier(Modifier::UNDERLINED),
        2 => base,
        _ => base.add_modifier(Modifier::ITALIC),
    }
}

/// Стиль инлайн-кода и нераскрашенного блока кода — реверс (тема-независим).
fn code_style() -> Style {
    Style::new().add_modifier(Modifier::REVERSED)
}

/// Стиль ссылки.
fn link_style(palette: &Palette) -> Style {
    Style::new()
        .fg(palette.accent)
        .add_modifier(Modifier::UNDERLINED)
}

/// Стиль цитаты (тема-независим).
fn blockquote_style() -> Style {
    Style::new().add_modifier(Modifier::DIM | Modifier::ITALIC)
}

// ---------- writer: pulldown events → строки ----------

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

/// Накопитель строк: разворачивает поток событий pulldown-cmark в `Vec<Line>`.
struct Writer {
    palette: Palette,
    lines: Vec<Line<'static>>,
    /// Стек инлайн-стилей (вершина — текущий).
    inline_styles: Vec<Style>,
    /// Префиксы строк (для цитат: `>`), применяются в [`Writer::push_line`].
    line_prefixes: Vec<Span<'static>>,
    /// Стек стилей строк (цитаты/блок кода).
    line_styles: Vec<Style>,
    /// Стек индексов списков (`None` — маркированный, `Some` — нумерованный).
    list_indices: Vec<Option<u64>>,
    /// Накопленный URL ссылки (добавляется при закрытии тега).
    link: Option<String>,
    /// Подсветчик активного блока кода.
    code_highlighter: Option<HighlightLines<'static>>,
    /// Нужен ли пустой разделитель перед следующим блоком.
    needs_newline: bool,
}

impl Writer {
    fn new(palette: Palette) -> Self {
        Self {
            palette,
            lines: Vec::new(),
            inline_styles: Vec::new(),
            line_prefixes: Vec::new(),
            line_styles: Vec::new(),
            list_indices: Vec::new(),
            link: None,
            code_highlighter: None,
            needs_newline: false,
        }
    }

    fn run<'a, I: Iterator<Item = Event<'a>>>(&mut self, iter: I) {
        for event in iter {
            self.handle_event(event);
        }
    }

    fn handle_event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.text(text),
            Event::Code(code) => self.code(code),
            Event::SoftBreak => self.push_span(Span::raw(" ")),
            Event::HardBreak => self.push_line(Line::default()),
            Event::Rule => self.rule(),
            Event::TaskListMarker(checked) => self.task_list_marker(checked),
            // HTML, сноски, math (math включается на следующем этапе) — игнорируем.
            _ => {}
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.start_paragraph(),
            Tag::Heading { level, .. } => self.start_heading(level),
            Tag::BlockQuote(_) => self.start_blockquote(),
            Tag::CodeBlock(kind) => self.start_codeblock(kind),
            Tag::List(start_index) => self.start_list(start_index),
            Tag::Item => self.start_item(),
            Tag::Emphasis => self.push_inline_style(Style::new().italic()),
            Tag::Strong => self.push_inline_style(Style::new().bold()),
            Tag::Strikethrough => self.push_inline_style(Style::new().crossed_out()),
            Tag::Link { dest_url, .. } => self.link = Some(dest_url.into_string()),
            _ => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.needs_newline = true,
            TagEnd::Heading(_) => self.needs_newline = true,
            TagEnd::BlockQuote(_) => self.end_blockquote(),
            TagEnd::CodeBlock => self.end_codeblock(),
            TagEnd::List(_) => self.end_list(),
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.inline_styles.pop();
            }
            TagEnd::Link => self.end_link(),
            _ => {}
        }
    }

    fn start_paragraph(&mut self) {
        if self.needs_newline {
            self.push_line(Line::default());
        }
        self.push_line(Line::default());
        self.needs_newline = false;
    }

    fn start_heading(&mut self, level: HeadingLevel) {
        if self.needs_newline {
            self.push_line(Line::default());
        }
        let lvl = heading_number(level);
        let style = heading_style(lvl, &self.palette);
        let prefix = format!("{} ", "#".repeat(lvl as usize));
        self.push_line(Line::styled(prefix, style));
        self.needs_newline = false;
    }

    fn start_blockquote(&mut self) {
        if self.needs_newline {
            self.push_line(Line::default());
            self.needs_newline = false;
        }
        self.line_prefixes.push(Span::from("> "));
        self.line_styles.push(blockquote_style());
    }

    fn end_blockquote(&mut self) {
        self.line_prefixes.pop();
        self.line_styles.pop();
        self.needs_newline = true;
    }

    fn start_list(&mut self, index: Option<u64>) {
        if self.list_indices.is_empty() && self.needs_newline {
            self.push_line(Line::default());
        }
        self.list_indices.push(index);
    }

    fn end_list(&mut self) {
        self.list_indices.pop();
        self.needs_newline = true;
    }

    fn start_item(&mut self) {
        self.push_line(Line::default());
        let width = self.list_indices.len() * 4 - 3;
        if let Some(last_index) = self.list_indices.last_mut() {
            let span = match last_index {
                None => Span::from(" ".repeat(width - 1) + "- "),
                Some(index) => {
                    *index += 1;
                    Span::styled(
                        format!("{:width$}. ", *index - 1),
                        Style::new().fg(self.palette.accent),
                    )
                }
            };
            self.push_span(span);
        }
        self.needs_newline = false;
    }

    fn task_list_marker(&mut self, checked: bool) {
        let marker = if checked { 'x' } else { ' ' };
        if let Some(line) = self.lines.last_mut() {
            if let Some(first) = line.spans.first_mut() {
                let content = first.content.to_mut();
                if content.ends_with("- ") {
                    let len = content.len();
                    content.truncate(len - 2);
                    content.push_str(&format!("- [{marker}] "));
                    return;
                }
            }
            line.spans.insert(1, Span::from(format!("[{marker}] ")));
        }
    }

    fn rule(&mut self) {
        if self.needs_newline {
            self.push_line(Line::default());
        }
        self.push_line(Line::from("───").add_modifier(Modifier::DIM));
        self.needs_newline = true;
    }

    fn start_codeblock(&mut self, kind: CodeBlockKind<'_>) {
        if !self.lines.is_empty() {
            self.push_line(Line::default());
        }
        let lang = match kind {
            CodeBlockKind::Fenced(ref lang) => lang.as_ref(),
            CodeBlockKind::Indented => "",
        };
        if let Some(syntax) = SYNTAX_SET.find_syntax_by_token(lang) {
            let theme = &THEME_SET.themes["base16-ocean.dark"];
            self.code_highlighter = Some(HighlightLines::new(syntax, theme));
        } else {
            self.line_styles.push(code_style());
        }
        self.push_line(Line::from(format!("```{lang}")).add_modifier(Modifier::DIM));
        self.needs_newline = false;
    }

    fn end_codeblock(&mut self) {
        self.push_line(Line::from("```").add_modifier(Modifier::DIM));
        self.needs_newline = true;
        if self.code_highlighter.take().is_none() {
            self.line_styles.pop();
        }
    }

    fn text(&mut self, text: CowStr<'_>) {
        if let Some(highlighter) = &mut self.code_highlighter {
            let highlighted: Text = LinesWithEndings::from(&text)
                .filter_map(|line| highlighter.highlight_line(line, &SYNTAX_SET).ok())
                .filter_map(|parts| as_24_bit_terminal_escaped(&parts, false).into_text().ok())
                .flatten()
                .collect();
            for line in highlighted.lines {
                self.lines.push(line);
            }
            self.needs_newline = false;
            return;
        }
        let style = self.current_style();
        for (i, line) in text.lines().enumerate() {
            if self.needs_newline {
                self.push_line(Line::default());
                self.needs_newline = false;
            }
            if i > 0 {
                self.push_line(Line::default());
            }
            self.push_span(Span::styled(line.to_string(), style));
        }
    }

    fn code(&mut self, code: CowStr<'_>) {
        self.push_span(Span::styled(code.into_string(), code_style()));
    }

    fn end_link(&mut self) {
        if let Some(url) = self.link.take() {
            self.push_span(Span::from(" ("));
            self.push_span(Span::styled(url, link_style(&self.palette)));
            self.push_span(Span::from(")"));
        }
    }

    fn current_style(&self) -> Style {
        self.inline_styles.last().copied().unwrap_or_default()
    }

    fn push_inline_style(&mut self, style: Style) {
        let merged = self.current_style().patch(style);
        self.inline_styles.push(merged);
    }

    fn push_line(&mut self, line: Line<'static>) {
        let style = self.line_styles.last().copied().unwrap_or_default();
        let mut line = line.patch_style(style);
        // Префиксы строк (цитаты) — в начало, в обратном порядке стека.
        for prefix in self.line_prefixes.iter().rev().cloned() {
            line.spans.insert(0, prefix);
        }
        self.lines.push(line);
    }

    fn push_span(&mut self, span: Span<'static>) {
        if let Some(line) = self.lines.last_mut() {
            line.spans.push(span);
        } else {
            self.push_line(Line::from(vec![span]));
        }
    }
}

fn heading_number(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Заменяет распространённые LaTeX-конструкции их unicode-аппроксимациями:
/// команды (`\alpha`→α, `\rightarrow`→→, `\leq`→≤, …) и верхние/нижние индексы
/// (`x^2`→x², `H_2`→H₂, `^{-1}`→⁻¹). Неизвестные команды/символы остаются как есть.
pub fn latex_to_unicode(input: &str) -> String {
    let with_commands = replace_commands(input);
    replace_scripts(&with_commands)
}

// ---------- команды `\name` ----------

/// Заменяет `\name` на unicode по таблице (наибольшее совпадение по имени).
/// `\\` → `\`; `\ ` (бэкслеш-пробел) → пробел; неизвестная команда не трогается.
fn replace_commands(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            // безопасно: режем по границе ASCII-символа либо копируем UTF-8 как есть
            let ch = input[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        // нашли '\'; читаем имя команды (ASCII-буквы)
        let start = i + 1;
        let mut j = start;
        while j < bytes.len() && bytes[j].is_ascii_alphabetic() {
            j += 1;
        }
        if j == start {
            // не буква после '\': экранированный символ (\\, \{, \_ …) или одиночный '\'
            if start < bytes.len() {
                let next = input[start..].chars().next().unwrap();
                out.push(next);
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
            // Пробел после команды НЕ съедаем (в отличие от настоящего LaTeX):
            // это аппроксимация для чтения, и пользовательские пробелы — значимый
            // визуальный разделитель (`\alpha + \beta` → «α + β», не «α+ β»).
        } else {
            // неизвестная команда — оставляем как есть (вместе с '\')
            out.push('\\');
            out.push_str(name);
            i = j;
        }
    }
    out
}

/// Таблица подстановок LaTeX-команд → unicode.
fn command_symbol(name: &str) -> Option<&'static str> {
    let s = match name {
        // строчные греческие
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
        // прописные греческие
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
        // стрелки
        "rightarrow" | "to" => "→",
        "leftarrow" | "gets" => "←",
        "leftrightarrow" => "↔",
        "Rightarrow" | "implies" => "⇒",
        "Leftarrow" => "⇐",
        "Leftrightarrow" | "iff" => "⇔",
        "uparrow" => "↑",
        "downarrow" => "↓",
        "mapsto" => "↦",
        // операторы и отношения
        "leq" | "le" => "≤",
        "geq" | "ge" => "≥",
        "neq" | "ne" => "≠",
        "approx" => "≈",
        "equiv" => "≡",
        "times" => "×",
        "div" => "÷",
        "pm" => "±",
        "mp" => "∓",
        "cdot" => "·",
        "ast" => "∗",
        "star" => "⋆",
        "infty" => "∞",
        "partial" => "∂",
        "nabla" => "∇",
        "sum" => "∑",
        "prod" => "∏",
        "int" => "∫",
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
        "neg" | "lnot" => "¬",
        "land" | "wedge" => "∧",
        "lor" | "vee" => "∨",
        "oplus" => "⊕",
        "otimes" => "⊗",
        "perp" => "⊥",
        "angle" => "∠",
        "degree" => "°",
        "ldots" | "dots" => "…",
        "cdots" => "⋯",
        // буквы-множества
        "mathbb" => "", // без аргумента ничего не делаем (упрощение)
        _ => return None,
    };
    if s.is_empty() { None } else { Some(s) }
}

// ---------- верхние/нижние индексы ----------

/// Заменяет `^x`/`^{xyz}` и `_x`/`_{xyz}` на unicode-индексы, если все символы
/// группы имеют unicode-аналог. Иначе оставляет конструкцию как есть.
fn replace_scripts(input: &str) -> String {
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

/// Пытается прочитать аргумент индекса после `^`/`_`: либо `{...}`, либо один
/// символ. Возвращает (unicode-строка, сколько символов израсходовано) при успехе.
fn take_script(rest: &[char], sup: bool) -> Option<(String, usize)> {
    if rest.is_empty() {
        return None;
    }
    if rest[0] == '{' {
        // ищем закрывающую '}'
        let close = rest.iter().position(|&c| c == '}')?;
        let inner = &rest[1..close];
        let mapped = map_script_chars(inner, sup)?;
        Some((mapped, close + 1)) // включая '{' и '}'
    } else {
        let mapped = map_script_chars(&rest[0..1], sup)?;
        Some((mapped, 1))
    }
}

/// Маппит все символы в unicode-индексы; `None`, если хоть один не имеет аналога.
fn map_script_chars(chars: &[char], sup: bool) -> Option<String> {
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

fn superscript(c: char) -> Option<char> {
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
        'n' => 'ⁿ',
        'i' => 'ⁱ',
        _ => return None,
    })
}

fn subscript(c: char) -> Option<char> {
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
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Собирает весь текст рендера в одну строку (для проверок содержимого):
    /// спаны одной строки склеиваются без разделителя, строки — через `\n`.
    fn rendered_text(input: &str) -> String {
        let text = render(input, 80, &Palette::default());
        text.lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn greek_and_arrows() {
        assert_eq!(latex_to_unicode(r"\alpha + \beta"), "α + β");
        assert_eq!(latex_to_unicode(r"A \rightarrow B"), "A → B");
        assert_eq!(latex_to_unicode(r"x \leq y \times z"), "x ≤ y × z");
        assert_eq!(latex_to_unicode(r"\Omega \neq \emptyset"), "Ω ≠ ∅");
    }

    #[test]
    fn command_preserves_following_whitespace() {
        // пробелы пользователя сохраняются (аппроксимация для чтения)
        assert_eq!(latex_to_unicode(r"\pi r^2"), "π r²");
        assert_eq!(latex_to_unicode(r"\alpha+\beta"), "α+β");
    }

    #[test]
    fn unknown_command_is_left_intact() {
        assert_eq!(latex_to_unicode(r"\foobar x"), r"\foobar x");
    }

    #[test]
    fn escaped_backslash_and_brace() {
        assert_eq!(latex_to_unicode(r"a \\ b"), r"a \ b");
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
    fn unmappable_script_is_left_intact() {
        // 'q' нет в верхних индексах — конструкция остаётся как есть
        assert_eq!(latex_to_unicode("x^q"), "x^q");
        assert_eq!(latex_to_unicode("x^{ab}"), "x^{ab}");
    }

    #[test]
    fn plain_text_unchanged() {
        assert_eq!(
            latex_to_unicode("обычный текст 2 + 2"),
            "обычный текст 2 + 2"
        );
    }

    #[test]
    fn render_produces_owned_text() {
        let text = render("# Заголовок\n\nабзац с `кодом`.", 80, &Palette::default());
        assert!(!text.lines.is_empty());
        let collected = rendered_text("# Заголовок\n\nабзац с `кодом`.");
        assert!(collected.contains("Заголовок"));
    }

    #[test]
    fn render_applies_latex_then_markdown() {
        let collected = rendered_text(r"Формула: $x^2 + \alpha$");
        assert!(collected.contains("x²"));
        assert!(collected.contains('α'));
    }

    #[test]
    fn render_headings_lists_quotes() {
        let collected = rendered_text("## Заголовок\n\n- пункт раз\n- пункт два\n\n> цитата");
        assert!(collected.contains("## Заголовок"));
        assert!(collected.contains("- пункт раз"));
        assert!(collected.contains("> цитата"));
    }
}

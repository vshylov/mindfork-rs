//! Рендер markdown в `ratatui::Text` + лёгкая unicode-аппроксимация LaTeX.
//! См. spec §11.4 и docs/decisions/0001-ui-crates-ratatui-030.md.
//!
//! Markdown парсит `tui-markdown` (markdown → `Text`, подсветка кода). Поверх —
//! наша подстановка простых LaTeX-конструкций в unicode (полный LaTeX и рендер в
//! изображение сознательно НЕ реализуются).

use ratatui::text::{Line, Span, Text};

/// Рендерит markdown-строку в владеющий [`Text`] (готовый к показу/кэшированию).
///
/// Перед парсингом применяется [`latex_to_unicode`] — стрелки и простые формулы
/// становятся читаемыми. Возвращается `'static`-`Text` (содержимое скопировано),
/// поэтому результат можно хранить в состоянии UI.
pub fn render(input: &str) -> Text<'static> {
    let approximated = latex_to_unicode(input);
    own_text(tui_markdown::from_str(&approximated))
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

// ---------- владеющая копия Text ----------

fn own_text(text: Text<'_>) -> Text<'static> {
    let lines: Vec<Line<'static>> = text.lines.into_iter().map(own_line).collect();
    let mut owned = Text::from(lines);
    owned.style = text.style;
    owned.alignment = text.alignment;
    owned
}

fn own_line(line: Line<'_>) -> Line<'static> {
    let spans: Vec<Span<'static>> = line
        .spans
        .into_iter()
        .map(|s| Span::styled(s.content.into_owned(), s.style))
        .collect();
    let mut owned = Line::from(spans);
    owned.style = line.style;
    owned.alignment = line.alignment;
    owned
}

#[cfg(test)]
mod tests {
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
        let text = render("# Заголовок\n\nабзац с `кодом`.");
        assert!(!text.lines.is_empty());
        // владеющий Text живёт независимо от входной строки
        let collected: String = text
            .lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(collected.contains("Заголовок"));
    }

    #[test]
    fn render_applies_latex_then_markdown() {
        let text = render(r"Формула: $x^2 + \alpha$");
        let collected: String = text
            .lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(collected.contains("x²"));
        assert!(collected.contains('α'));
    }
}

//! Markdown — LaTeX→unicode: нормализация разделителей + конвертер команд. Часть модуля [`super`]; разбито из монолита
//! markdown.rs (см. docs/refactoring-god-objects.md, этап 6).

// ---------- нормализация разделителей формул ----------

/// Приводит формы разделителей LaTeX к долларовым, понятным парсеру:
/// `\(…\)`→`$…$`, `\[…\]`→`$$…$$`. Содержимое **код-спанов и блоков кода**
/// (последовательности `` ` `` любой длины) копируется дословно — внутри кода
/// `\(` не трогаем. Сами доллары далее снимает парсер (math-события).
pub fn normalize_delimiters(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < n {
        // Код-спан/блок: копируем от открывающего ряда бэктиков до закрывающего
        // ряда той же длины дословно.
        if chars[i] == '`' {
            let fence = backtick_run(&chars, i);
            let body_start = i + fence;
            let end = match find_backtick_run(&chars, body_start, fence) {
                Some(close_end) => close_end,
                None => n,
            };
            out.extend(&chars[i..end]);
            i = end;
            continue;
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

/// Длина ряда бэктиков, начинающегося с `start`.
pub(super) fn backtick_run(chars: &[char], start: usize) -> usize {
    let mut k = start;
    while k < chars.len() && chars[k] == '`' {
        k += 1;
    }
    k - start
}

/// Ищет закрывающий ряд бэктиков ровно длины `len`, начиная с `from`.
/// Возвращает индекс **за** закрывающим рядом.
pub(super) fn find_backtick_run(chars: &[char], from: usize, len: usize) -> Option<usize> {
    let mut j = from;
    while j < chars.len() {
        if chars[j] == '`' {
            let run = backtick_run(chars, j);
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

/// Ищет с позиции `from` пару `(a, b)` подряд; возвращает индекс символа `a`.
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

// ---------- содержимое формулы → unicode ----------

/// Преобразует содержимое формулы (без разделителей) в unicode-аппроксимацию:
/// дроби `\frac{a}{b}`→`a/b`, корни `\sqrt{x}`→`√(x)`, текстовые обёртки
/// (`\text{…}`/`\mathrm{…}`/…) → содержимое, команды (`\alpha`→α, `\leq`→≤, …),
/// верхние/нижние индексы (`x^2`→x², `^{-1}`→⁻¹), скобки-группировки снимаются.
/// Литеральные `\{`/`\}` сохраняются. Неизвестные команды остаются как есть.
pub fn latex_to_unicode(input: &str) -> String {
    // Защищаем литеральные скобки от снятия группировки в конце.
    const LBRACE: char = '\u{1}';
    const RBRACE: char = '\u{2}';
    let protected = input.replace("\\{", "\u{1}").replace("\\}", "\u{2}");
    let with_braces = apply_brace_commands(&protected);
    let with_commands = replace_commands(&with_braces);
    let with_scripts = replace_scripts(&with_commands);
    // Снимаем оставшиеся группирующие скобки и восстанавливаем литеральные.
    let stripped: String = with_scripts
        .chars()
        .filter(|&c| c != '{' && c != '}')
        .map(|c| match c {
            LBRACE => '{',
            RBRACE => '}',
            other => other,
        })
        .collect();
    stripped.trim().to_string()
}

/// Раскрывает brace-команды: `\frac{a}{b}`→`a/b`, `\sqrt{a}`→`√(a)`,
/// текстовые обёртки → содержимое. Содержимое групп обрабатывается рекурсивно
/// (вложенные дроби/обёртки). Прочие `\name` копируются как есть (их разберёт
/// [`replace_commands`]).
pub(super) fn apply_brace_commands(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < n {
        if chars[i] == '\\' {
            let start = i + 1;
            let mut j = start;
            while j < n && chars[j].is_ascii_alphabetic() {
                j += 1;
            }
            let name: String = chars[start..j].iter().collect();
            if !name.is_empty() && j < n && chars[j] == '{' {
                if name == "frac"
                    && let Some((a, after_a)) = read_group(&chars, j)
                    && after_a < n
                    && chars[after_a] == '{'
                    && let Some((b, after_b)) = read_group(&chars, after_a)
                {
                    out.push_str(&apply_brace_commands(&a));
                    out.push('/');
                    out.push_str(&apply_brace_commands(&b));
                    i = after_b;
                    continue;
                } else if name == "sqrt"
                    && let Some((a, after_a)) = read_group(&chars, j)
                {
                    out.push('√');
                    out.push('(');
                    out.push_str(&apply_brace_commands(&a));
                    out.push(')');
                    i = after_a;
                    continue;
                } else if name == "pmod"
                    && let Some((a, after_a)) = read_group(&chars, j)
                {
                    out.push_str("(mod ");
                    out.push_str(&apply_brace_commands(&a));
                    out.push(')');
                    i = after_a;
                    continue;
                } else if is_text_command(&name)
                    && let Some((a, after_a)) = read_group(&chars, j)
                {
                    out.push_str(&apply_brace_commands(&a));
                    i = after_a;
                    continue;
                }
            }
            // Не обрабатываемая здесь команда: копируем только '\', остальное —
            // обычными символами (их подхватит replace_commands).
            out.push('\\');
            i += 1;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Читает сбалансированную группу `{…}`, начиная с `open` (где `chars[open]=='{'`).
/// Возвращает (содержимое без внешних скобок, индекс за `}`). `None` — нет пары.
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

/// Текстовые/шрифтовые обёртки, у которых берём только содержимое.
pub(super) fn is_text_command(name: &str) -> bool {
    matches!(
        name,
        "text"
            | "textbf"
            | "textit"
            | "textrm"
            | "mathrm"
            | "mathbf"
            | "mathit"
            | "mathbb"
            | "mathcal"
            | "mathsf"
            | "mathtt"
            | "operatorname"
            | "boldsymbol"
            // акценты/обёртки: показываем содержимое (диакритику опускаем)
            | "overline"
            | "underline"
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

/// Операторные имена (`\log`, `\sin`, `\lim`, …) — печатаются словом без `\`.
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

// ---------- команды `\name` ----------

/// Заменяет `\name` на unicode по таблице (наибольшее совпадение по имени).
/// `\\` → `\`; `\ ` (бэкслеш-пробел) → пробел; неизвестная команда не трогается.
pub(super) fn replace_commands(input: &str) -> String {
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
            // не буква после '\': spacing-команда (`\,` `\;` `\:` `\ `→пробел,
            // `\!`→ничего), экранированный символ (`\\`, `\_`, …) или одиночный '\'.
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
            // Пробел после команды НЕ съедаем (в отличие от настоящего LaTeX):
            // это аппроксимация для чтения, и пользовательские пробелы — значимый
            // визуальный разделитель (`\alpha + \beta` → «α + β», не «α+ β»).
        } else if is_function_name(name) {
            // Операторное имя (`\log`, `\sin`, `\lim`, …) — печатаем словом.
            out.push_str(name);
            i = j;
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
pub(super) fn command_symbol(name: &str) -> Option<&'static str> {
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
        "longrightarrow" => "⟶",
        "longleftarrow" => "⟵",
        "Rightarrow" | "implies" => "⇒",
        "Longrightarrow" => "⟹",
        "Leftarrow" => "⇐",
        "Leftrightarrow" | "iff" => "⇔",
        "uparrow" => "↑",
        "downarrow" => "↓",
        "mapsto" => "↦",
        // операторы и отношения
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
        // пробельные команды (читаемость): сводим к пробелу
        "quad" => " ",
        "qquad" => "  ",
        "bmod" => "mod",
        // модификаторы размера скобок — снимаем, скобку-делимитер оставляем
        "left" | "right" | "big" | "Big" | "bigg" | "Bigg" | "bigl" | "bigr" | "Bigl" | "Bigr"
        | "biggl" | "biggr" => "",
        _ => return None,
    };
    Some(s)
}

// ---------- верхние/нижние индексы ----------

/// Заменяет `^x`/`^{xyz}` и `_x`/`_{xyz}` на unicode-индексы, если все символы
/// группы имеют unicode-аналог. Иначе оставляет конструкцию как есть.
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

/// Пытается прочитать аргумент индекса после `^`/`_`: либо `{...}`, либо один
/// символ. Возвращает (unicode-строка, сколько символов израсходовано) при успехе.
pub(super) fn take_script(rest: &[char], sup: bool) -> Option<(String, usize)> {
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
        'n' => 'ⁿ',
        'i' => 'ⁱ',
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
        _ => return None,
    })
}

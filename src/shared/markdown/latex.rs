//! Markdown — LaTeX→unicode: нормализация разделителей + конвертер команд. Часть модуля [`super`]; разбито из монолита
//! markdown.rs (см. docs/history/refactoring-god-objects.md, этап 6).

/// Сентинелы для скобок, которые должны пережить финальный срез группировки:
/// литеральные `\{`/`\}` **и** скобки нераспознанных brace-команд (`\binom{n}{k}`
/// остаётся читаемым, а не склеивается в `\binomnk`).
const LBRACE: char = '\u{1}';
const RBRACE: char = '\u{2}';

/// Потолок глубины рекурсии [`apply_brace_commands`] — защита стека от
/// патологической вложенности (`\frac{\frac{…}}` на тысячи уровней). Глубже —
/// содержимое группы копируется без дальнейшего разбора.
const MAX_BRACE_DEPTH: usize = 64;

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
        // Код-спан/блок на бэктиках: содержимое копируем дословно (внутри кода `\(`
        // не трогаем). Закрывающий ряд — той же длины.
        if chars[i] == '`' {
            let fence = char_run(&chars, i, '`');
            let body_start = i + fence;
            match find_char_run(&chars, body_start, fence, '`') {
                Some(close_end) => {
                    out.extend(&chars[i..close_end]);
                    i = close_end;
                }
                None if fence >= 3 => {
                    // Незакрытый забор — норма при стриминге ` ``` `: до конца дословно
                    // (внутри незавершённого блока нормализовать нельзя).
                    out.extend(&chars[i..]);
                    i = n;
                }
                None => {
                    // Незакрытый короткий ряд (1–2) по CommonMark — литерал: копируем
                    // сами бэктики и ПРОДОЛЖАЕМ нормализацию (иначе одинокий ` глушил бы
                    // конверсию формул до конца сообщения).
                    out.extend(&chars[i..body_start]);
                    i = body_start;
                }
            }
            continue;
        }
        // Огороженный блок на тильдах (`~~~`): содержимое дословно. Ряд < 3 (напр.
        // `~~зачёркивание~~`) забором не считается — тильды копируются обычным путём.
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

/// Длина ряда одинаковых символов `ch`, начинающегося с `start`.
pub(super) fn char_run(chars: &[char], start: usize, ch: char) -> usize {
    let mut k = start;
    while k < chars.len() && chars[k] == ch {
        k += 1;
    }
    k - start
}

/// Ищет закрывающий ряд `ch` ровно длины `len`, начиная с `from`.
/// Возвращает индекс **за** закрывающим рядом.
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

/// Ищет закрывающий ряд тильд длины ≥ 3, начиная с `from` (заборы `~~~`
/// закрываются рядом не короче открывающего, но для «пропустить код» достаточно
/// найти любой ряд ≥ 3). Возвращает индекс **за** закрывающим рядом.
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
    apply_brace_commands_depth(input, 0)
}

/// Рекурсивно обрабатывает содержимое группы (с учётом потолка глубины).
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

        // `\sqrt` с опциональным индексом `\sqrt[3]{x}` → ∛(x). Проверяем до общего
        // brace-разбора (индекс идёт между именем и группой).
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
            // иначе — «голый» \sqrt, разберётся ниже как обычная команда (→ √)
        }

        if !name.is_empty() && j < n && chars[j] == '{' {
            // Дроби: `\frac`/`\dfrac`/`\tfrac`/`\cfrac{a}{b}` → a/b.
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
            // Биномиальный коэффициент `\binom{n}{k}` → C(n, k).
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
            // Наложения `\overset{a}{b}`/`\underset`/`\stackrel` → базовый (второй)
            // аргумент; аннотация опускается (как диакритика у акцентов).
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
            // Начертания `\mathbb{R}`→ℝ, `\mathcal{L}`→ℒ, `\mathfrak{g}`: одиночная
            // буква с BMP-аналогом → глиф; иначе — содержимое (как обёртка).
            if matches!(name.as_str(), "mathbb" | "mathcal" | "mathfrak")
                && let Some((a, after_a)) = read_group(&chars, j)
            {
                out.push_str(&blackboard_or_content(&name, &a, depth));
                i = after_a;
                continue;
            }
            // Текстовые/шрифтовые обёртки и акценты → содержимое.
            if is_text_command(&name)
                && let Some((a, after_a)) = read_group(&chars, j)
            {
                out.push_str(&brace_recurse(&a, depth));
                i = after_a;
                continue;
            }
            // НЕИЗВЕСТНАЯ команда с группой: сохраняем `\name` и все её группы,
            // **защищая скобки** (сентинелы) — иначе финальный срез склеил бы
            // `\binom{n}{k}` в `\binomnk`. Содержимое групп обрабатывается рекурсивно.
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

        // Команда без группы (или одиночный '\'): копируем '\', остальное подхватит
        // replace_commands.
        out.push('\\');
        i += 1;
    }
    out
}

/// Дроби, сводимые к `a/b`.
fn is_frac_command(name: &str) -> bool {
    matches!(name, "frac" | "dfrac" | "tfrac" | "cfrac")
}

/// Команды-наложения, у которых берём базовый (второй) аргумент.
fn is_stack_command(name: &str) -> bool {
    matches!(name, "overset" | "underset" | "stackrel")
}

/// Читает опциональный аргумент `[…]` начиная с `from` (если он там есть).
/// Возвращает (содержимое без скобок, индекс за `]`); если `[` нет — `(None, from)`.
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

/// Рендер корня по опциональному индексу: `None`/`2` → `√(x)`, `3` → `∛(x)`,
/// `4` → `∜(x)`, иначе — префикс-superscript `ⁿ√(x)` (если индекс целиком мапится)
/// либо фолбэк `√[n](x)`.
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

/// `\mathbb{R}`→ℝ и родственные начертания: одиночная буква с BMP-аналогом → глиф,
/// иначе содержимое обрабатывается как обычная текст-обёртка (`\mathbb{XY}`→`XY`).
/// Supplementary-plane (`𝔸`…, строчные) не берём — терминалы поддерживают неровно.
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

/// Double-struck (blackboard bold) заглавные из блока Letterlike Symbols (BMP).
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

/// Script (каллиграфические) буквы из блока Letterlike Symbols (BMP, набор неполный).
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

/// Fraktur (готические) буквы из блока Letterlike Symbols (BMP, набор неполный).
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
            // акценты/обёртки: показываем содержимое (диакритику опускаем)
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
            // `\left.` / `\right.` — «невидимый» делимитер-точка: модификатор снят
            // (пустая строка), съедаем и точку, чтобы не осталась висячая «.».
            if (name == "left" || name == "right") && i < bytes.len() && bytes[i] == b'.' {
                i += 1;
            }
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
        // делимитеры и разделители
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
        // отношения
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
        // стрелки (симметрия к уже имеющимся + частые)
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
        // греческие варианты
        "vartheta" => "ϑ",
        "varsigma" => "ς",
        "varrho" => "ϱ",
        "varkappa" => "ϰ",
        "varpi" => "ϖ",
        // операторы над множествами и большие операторы
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
        // прочие символы
        "dagger" => "†",
        "ddagger" => "‡",
        "diamond" => "⋄",
        "Box" | "square" => "□",
        "blacksquare" => "■",
        "triangleleft" => "◁",
        "triangleright" => "▷",
        "checkmark" => "✓",
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
        match map_script_chars(inner, sup) {
            Some(mapped) => Some((mapped, close + 1)), // включая '{' и '}'
            None => {
                // Группа не мапится целиком — показываем как `^(…)`/`_(…)`, сохраняя
                // группировку (иначе `x^{q+}` терял бы скобки → `x^q+`).
                let marker = if sup { '^' } else { '_' };
                let inner_str: String = inner.iter().collect();
                Some((format!("{marker}({inner_str})"), close + 1))
            }
        }
    } else {
        // Одиночный символ без группы: не мапится — оставляем как есть (`x^q`→`x^q`).
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
        // строчные латинские (нет 'q' в Unicode)
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
        // заглавные латинские (доступны не все)
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
        // строчные латинские (Unicode покрывает лишь часть)
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
    fn unmappable_script_keeps_group_as_parens() {
        // 'q' нет в верхних индексах — одиночный символ без группы остаётся как есть;
        // группа с несмапливаемым символом сохраняется как `^(…)` (не теряет скобки).
        assert_eq!(latex_to_unicode("x^q"), "x^q");
        assert_eq!(latex_to_unicode("x^{q+}"), "x^(q+)");
    }

    #[test]
    fn letter_scripts_map_to_unicode() {
        // буквенные индексы (частые: x_i, a_n, x^T) теперь конвертируются
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
        assert_eq!(latex_to_unicode(r"\mathfrak{g}"), "g"); // 'g' нет в наборе → содержимое
        assert_eq!(latex_to_unicode(r"\mathbb{XY}"), "XY"); // не одна буква → содержимое
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
        // внутри код-спана `\(` не трогаем
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
        // доллары-разделители сняты парсером
        assert!(!collected.contains('$'));
    }

    #[test]
    fn render_leaves_bare_commands_outside_math() {
        // вне $…$ команды не трогаем (выбранная семантика)
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
        // Нераспознанная команда с группой не склеивается с аргументом (регрессия:
        // \boxed{x+1} → \boxedx+1). Содержимое внутри — конвертируется.
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
        // индекс без superscript-аналога ('q' нет) — фолбэк с квадратными скобками
        assert_eq!(latex_to_unicode(r"\sqrt[q]{x}"), "√[q](x)");
        // «голый» \sqrt без индекса по-прежнему работает
        assert_eq!(latex_to_unicode(r"\sqrt{x}"), "√(x)");
    }

    #[test]
    fn left_right_dot_delimiter_eaten() {
        assert_eq!(latex_to_unicode(r"\left. x \right."), "x");
        // модификатор снят, скобка-делимитер остаётся
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
        // внутри ~~~-забора `\(` не трогаем (порча кода)
        assert_eq!(
            normalize_delimiters("~~~\n\\(x\\)\n~~~"),
            "~~~\n\\(x\\)\n~~~"
        );
        // ~~зачёркивание~~ (ряд < 3) забором не считается — формула после нормализуется
        assert_eq!(normalize_delimiters(r"~~s~~ \(y\)"), r"~~s~~ $y$");
    }

    #[test]
    fn normalize_unclosed_short_backtick_continues() {
        // одинокий ` (незакрытый короткий ряд) — литерал, формула после нормализуется
        assert_eq!(normalize_delimiters(r"a ` b \(x\)"), r"a ` b $x$");
        // незакрытый забор ``` — до конца дословно (норма при стриминге)
        assert_eq!(normalize_delimiters("```\n\\(x\\)"), "```\n\\(x\\)");
    }

    #[test]
    fn deep_nesting_does_not_overflow_stack() {
        // патологическая вложенность не роняет стек (потолок MAX_BRACE_DEPTH)
        let deep = "\\sqrt{".repeat(5000) + "x" + &"}".repeat(5000);
        let _ = latex_to_unicode(&deep);
    }
}

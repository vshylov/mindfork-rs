//! Тесты markdown-рендерера. См. mod.rs.

use super::*;

/// Собирает весь текст рендера в одну строку (для проверок содержимого):
/// спаны одной строки склеиваются без разделителя, строки — через `\n`.
fn rendered_text(input: &str) -> String {
    let text = render(input, 80, &Palette::default());
    text.lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Метки языков, которыми модели помечают код-блоки, должны резолвиться в
/// синтаксис — иначе блок остаётся без подсветки (был баг с ` ```csharp `:
/// токен не совпадал ни с именем `C#`, ни с расширением `cs`).
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
        // приближения: языка нет в наборе → близкий грамматик
        ("typescript", "JavaScript"),
        ("kotlin", "Java"),
    ] {
        let syntax = resolve_syntax(label)
            .unwrap_or_else(|| panic!("метка {label:?} не резолвится в синтаксис"));
        assert_eq!(syntax.name, expect_name, "метка {label:?}");
    }
}

/// Пустая/неизвестная метка не паникует и не резолвится.
#[test]
fn empty_and_unknown_language_do_not_resolve() {
    assert!(resolve_syntax("").is_none());
    assert!(resolve_syntax("совсем-не-язык-42").is_none());
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
fn unmappable_script_strips_group_braces() {
    // 'q' нет в верхних индексах — индекс не применяется; группирующие скобки
    // снимаются (это содержимое формулы, скобки — синтаксис группировки).
    assert_eq!(latex_to_unicode("x^q"), "x^q");
    assert_eq!(latex_to_unicode("x^{ab}"), "x^ab");
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
    assert_eq!(latex_to_unicode(r"\mathbb{R}"), "R");
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
fn render_produces_owned_text() {
    let text = render("# Заголовок\n\nабзац с `кодом`.", 80, &Palette::default());
    assert!(!text.lines.is_empty());
    let collected = rendered_text("# Заголовок\n\nабзац с `кодом`.");
    assert!(collected.contains("Заголовок"));
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

/// Максимальная ширина (в колонках) среди строк рендера.
fn max_line_width(input: &str, width: usize) -> usize {
    let text = render(input, width, &Palette::default());
    text.lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| wrap::display_width(&s.content.chars().collect::<Vec<_>>()))
                .sum::<usize>()
        })
        .max()
        .unwrap_or(0)
}

const TABLE_MD: &str = "\
| Алгоритм | Время | Память |
| :--- | :--- | :--- |
| QuickSort | O(n log n) | O(log n) |
| MergeSort | O(n log n) | O(n) |";

#[test]
fn table_wraps_cell_content() {
    // длинная ячейка переносится в несколько рядов, не вылезая за ширину
    let long = "\
| A | Особенности |
| :--- | :--- |
| x | Самая высокая скорость на практике сортировки |";
    let w = 40;
    assert!(max_line_width(long, w) <= w);
    // несколько строк тела → перенос произошёл
    let lines = render(long, w, &Palette::default()).lines.len();
    assert!(lines >= 6, "ожидался перенос ячейки в несколько рядов");
}

#[test]
fn table_renders_box_and_content() {
    let collected = rendered_text(TABLE_MD);
    assert!(collected.contains('┌') && collected.contains('┼') && collected.contains('└'));
    assert!(collected.contains("Алгоритм"));
    assert!(collected.contains("QuickSort"));
    assert!(collected.contains("MergeSort"));
}

#[test]
fn table_fits_panel_width() {
    // при достаточной ширине таблица не превышает её
    for w in [40usize, 60, 80, 120] {
        let max = max_line_width(TABLE_MD, w);
        assert!(max <= w, "ширина {max} превысила панель {w}");
    }
}

/// Таблица с переносом ячеек (как на скриншоте) не должна превышать ширину
/// **ни при каком** размере панели — иначе повторный перенос в `message_feed`
/// разорвал бы рамку. Регрессия на «пролитый» пробел на границе слова.
#[test]
fn wrapping_table_never_exceeds_any_width() {
    const WIDE: &str = "\
| Подход | Как работает | Минус |
| :--- | :--- | :--- |
| Стандартный Transformer Chain-of-Thought (o1) | Фиксированный проход Input → Output. Модель пишет рассуждения текстом в скрытый чат | Одинаковые затраты ресурсов на всё. Дорого по токенам, медленно, ограничено длиной текста. |
| Ваша идея (Recurrent ACT) | Итерации в скрытом пространстве (latent space) | Сложность в обучении (нужны новые методы градиентного спуска). |";
    for w in 30usize..=140 {
        let max = max_line_width(WIDE, w);
        assert!(max <= w, "при ширине {w} строка таблицы вышла на {max}");
    }
}

#[test]
fn wide_table_is_clipped_to_width() {
    // узкая панель: таблица обрезается, но не вылезает за край
    let narrow = 24;
    let max = max_line_width(TABLE_MD, narrow);
    assert!(
        max <= narrow,
        "ширина {max} превысила узкую панель {narrow}"
    );
    let collected = rendered_text_w(TABLE_MD, narrow);
    assert!(collected.contains('…'), "ожидался маркер обрезки");
}

/// Как [`rendered_text`], но с заданной шириной.
fn rendered_text_w(input: &str, width: usize) -> String {
    render(input, width, &Palette::default())
        .lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

use crate::shared::config::Theme;

const CODE_MD: &str = "```rust\nfn main() {\n    let s = \"hi\";\n    // c\n}\n```";

/// Собирает множество цветов переднего плана спанов рендера.
fn fg_colors(input: &str, palette: &Palette) -> Vec<Color> {
    render(input, 80, palette)
        .lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .filter_map(|s| s.style.fg)
        .collect()
}

#[test]
fn code_highlight_is_colored() {
    // Подсветка проставляет цвета переднего плана (не голый текст).
    let colors = fg_colors(CODE_MD, &Palette::for_theme(Theme::Dark));
    assert!(
        colors.iter().any(|c| matches!(c, Color::Rgb(..))),
        "ожидались RGB-цвета подсветки кода"
    );
}

#[test]
fn code_highlight_follows_theme() {
    // Та же подсветка кода в тёмной и светлой теме даёт разные цвета —
    // значит, подсветка согласована с темой, а не живёт «своей палитрой».
    let dark = fg_colors(CODE_MD, &Palette::for_theme(Theme::Dark));
    let light = fg_colors(CODE_MD, &Palette::for_theme(Theme::Light));
    assert_ne!(dark, light, "подсветка кода не зависит от темы");
}

/// Неподсвеченный (без языка) fenced-блок: содержимое начинается на строке под
/// открывающим `​```​`, а не приклеивается к нему (регрессия: первая строка
/// дописывалась в строку заборчика, `i==0` + `needs_newline==false`).
#[test]
fn plain_code_block_content_not_glued_to_fence() {
    let md = "```\nX_ij = 1, тест\nE = 2/(j-i+1)\n```";
    let lines: Vec<String> = render(md, 80, &Palette::default())
        .lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    // Открывающий заборчик — на своей строке, без содержимого.
    assert_eq!(
        lines[0], "```",
        "содержимое приклеилось к заборчику: {lines:?}"
    );
    assert_eq!(lines[1], "X_ij = 1, тест");
    assert_eq!(lines[2], "E = 2/(j-i+1)");
    assert_eq!(lines[3], "```");
}

#[test]
fn render_headings_lists_quotes() {
    let collected = rendered_text("## Заголовок\n\n- пункт раз\n- пункт два\n\n> цитата");
    assert!(collected.contains("## Заголовок"));
    assert!(collected.contains("- пункт раз"));
    assert!(collected.contains("> цитата"));
}

/// «Рыхлый» (loose) нумерованный список — элементы разделены пустой строкой,
/// поэтому pulldown-cmark оборачивает содержимое в `Paragraph`. Номер и текст
/// должны остаться на **одной** строке (`1. текст`), а не разъехаться (регрессия:
/// `start_paragraph` безусловно добавлял новую строку после маркера).
#[test]
fn loose_ordered_list_keeps_number_with_text() {
    let md = "1. **Первый.** Текст первого пункта.\n\n\
                  2. **Второй.** Текст второго пункта.\n\n\
                  3. **Третий.** Текст третьего пункта.";
    let collected = rendered_text(md);
    // Номер приклеен к своему тексту на одной строке ленты.
    assert!(
        collected.contains("1. Первый."),
        "номер оторвался от текста:\n{collected}"
    );
    assert!(collected.contains("2. Второй."));
    assert!(collected.contains("3. Третий."));
    // Пустой строки между маркером и его текстом быть не должно.
    assert!(
        !collected.contains("1. \n"),
        "после маркера образовался перенос:\n{collected}"
    );
}

/// Многоабзацный элемент «рыхлого» списка: первый абзац — на строке маркера,
/// последующие — на своих строках (маркер не дублируется).
#[test]
fn loose_list_item_second_paragraph_on_own_line() {
    let md = "1. Первый абзац.\n\n   Второй абзац того же пункта.\n\n2. Другой пункт.";
    let collected = rendered_text(md);
    assert!(collected.contains("1. Первый абзац."));
    assert!(collected.contains("Второй абзац того же пункта."));
    assert!(collected.contains("2. Другой пункт."));
}

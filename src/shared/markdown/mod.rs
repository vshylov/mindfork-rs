//! Рендер markdown в `ratatui::Text` + лёгкая unicode-аппроксимация LaTeX.
//! См. spec §11.4 и docs/decisions/0003-own-markdown-renderer.md.
//!
//! Markdown парсим напрямую через `pulldown-cmark` собственным «писателем»
//! (`Writer`): markdown → `Text`. Это даёт темизацию через [`Palette`] (а не
//! захардкоженные цвета), а позже — нативные таблицы и math-события. Подсветка
//! блоков кода — `syntect` + `ansi-to-tui`. Полный LaTeX и рендер в изображение
//! сознательно НЕ реализуются.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{LazyLock, Mutex};

use ansi_to_tui::IntoText;
use pulldown_cmark::{
    Alignment, CodeBlockKind, CowStr, Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd,
};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};

use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color as SynColor, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::{LinesWithEndings, as_24_bit_terminal_escaped};

use crate::shared::theme::Palette;
use crate::shared::wrap;

/// Поведенческие флаги рендера markdown (ширина и палитра — отдельными
/// аргументами, как раньше). Прецедент — `RenderOpts` у
/// [`crate::widgets::input_box`]: опции структурой вместо роста позиционных bool.
/// `Default` — «чистый» CommonMark-рендер (все флаги выключены); нужное включает
/// вызывающий (лента прокидывает флаги из настроек интерфейса).
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderOpts {
    /// Трактовать «мягкий» перенос (одиночный `\n` в исходнике) как реальный
    /// перенос строки (GFM-стиль, как комментарии на GitHub). `false` — стандарт
    /// CommonMark: одиночный перевод строки схлопывается в пробел. Нужно для
    /// **сообщений пользователя**: текст, набранный с `Shift+Enter`, должен
    /// показываться построчно, а не сливаться в один абзац. В ячейках таблиц
    /// перенос в любом случае остаётся пробелом (раскладку строк делает таблица).
    pub soft_break_as_newline: bool,
    /// Горизонтальные разделители (`├───┼───┤`) между строками тела таблиц —
    /// «сеточный» вид. `false` — компактный: разделитель только под заголовком.
    /// Управляется настройкой `interface.table_row_separators` (см. spec §11.4).
    pub table_row_separators: bool,
}

/// Рендерит markdown-строку в владеющий [`Text`] (готовый к показу/кэшированию)
/// с дефолтными флагами ([`RenderOpts::default`]).
///
/// `width` — ширина панели в колонках (используется для раскладки таблиц).
/// `palette` задаёт цвета (заголовки/ссылки/код/цитаты) под текущую тему.
///
/// LaTeX обрабатывается **только внутри математических разделителей**
/// (`$…$`/`$$…$$`; формы `\(…\)`/`\[…\]` приводятся к ним в [`normalize_delimiters`]).
/// Содержимое math-событий парсера проходит через [`latex_to_unicode`] (стрелки,
/// дроби, индексы, символы), а сами разделители снимает парсер. «Голые» команды
/// вне разделителей (`\alpha` без `$`) НЕ трогаются. Возвращается `'static`-`Text`.
// Продакшн-пути (лента) передают флаги явно через `render_with`; фасад с дефолтами
// используют тесты модуля — оставлен как публичная поверхность (см. ADR 0003).
#[allow(dead_code)]
pub fn render(input: &str, width: usize, palette: &Palette) -> Text<'static> {
    render_with(input, width, palette, RenderOpts::default())
}

/// Как [`render`], но с явными поведенческими флагами (см. [`RenderOpts`]).
pub fn render_with(
    input: &str,
    width: usize,
    palette: &Palette,
    opts: RenderOpts,
) -> Text<'static> {
    let normalized = normalize_delimiters(input);
    let mut parse_opts = Options::empty();
    parse_opts.insert(Options::ENABLE_STRIKETHROUGH);
    parse_opts.insert(Options::ENABLE_TASKLISTS);
    parse_opts.insert(Options::ENABLE_MATH);
    parse_opts.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(&normalized, parse_opts);
    let mut writer = Writer::new(*palette, width);
    writer.soft_break_as_newline = opts.soft_break_as_newline;
    writer.table_row_separators = opts.table_row_separators;
    writer.run(parser);
    Text::from(writer.lines)
}

/// Подсвечивает блок кода `code` на языке `lang` **без ограждающих ` ``` `** и без
/// переноса по ширине — для встраивания в tool-карточки ленты (см.
/// [`crate::widgets::message_feed`]). Возвращает по одной строке на строку
/// исходника, окрашенную по [`build_code_theme`] (той же теме, что fenced-блоки
/// markdown). Если язык не распознан ([`resolve_syntax`] промахнулся) — строки без
/// подсветки, цветом `palette.text` (не реверс — для нестрого-кодовых аргументов
/// читабельнее). Перенос длинных строк делает вызывающий.
pub fn highlight_code(code: &str, lang: &str, palette: &Palette) -> Vec<Line<'static>> {
    let Some(syntax) = resolve_syntax(lang) else {
        return code
            .split('\n')
            .map(|l| Line::from(Span::styled(l.to_string(), Style::new().fg(palette.text))))
            .collect();
    };
    let theme = code_theme(palette);
    let mut hl = HighlightLines::new(syntax, theme);
    let mut out: Vec<Line<'static>> = Vec::new();
    for line in LinesWithEndings::from(code) {
        out.extend(highlight_line_or_plain(&mut hl, line));
    }
    // Подсветка часто добавляет хвостовую пустую строку — снимаем, чтобы не давать
    // лишний пустой ряд под блоком в карточке.
    while out
        .last()
        .is_some_and(|l| l.spans.iter().all(|s| s.content.trim().is_empty()))
    {
        out.pop();
    }
    out
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

/// Стиль нераскрашенного блока кода — реверс (тема-независим).
fn code_style() -> Style {
    Style::new().add_modifier(Modifier::REVERSED)
}

/// Стиль инлайн-кода (`такой текст`) — тихий «чип» как в дизайн-макете: мягкий
/// приглушённый текст на фоне «клавиши», а не резкий реверс (REVERSED был слишком
/// заметным). Согласован с темой через [`Palette`].
fn inline_code_style(palette: &Palette) -> Style {
    Style::new().fg(palette.keycap_fg).bg(palette.keycap_bg)
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

/// Кэш syntect-тем подсветки кода, **построенных из [`Palette`]** (см.
/// [`build_code_theme`]). Раньше тема была захардкожена (`base16-ocean.dark`) и не
/// согласовывалась с dark/light/auto — ADR 0003 отмечал это как задел.
///
/// Ключ — палитра (различных всего три: auto/dark/light), поэтому утечка
/// `Box::leak` ограничена и оправдана: `HighlightLines<'static>` требует темы со
/// `'static`-временем жизни, а число тем конечно и живёт весь процесс. Альтернатива
/// (тема на стеке `render` + lifetime у `Writer`) усложнила бы тип ради экономии,
/// которой нет.
static CODE_THEMES: LazyLock<Mutex<HashMap<Palette, &'static Theme>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Возвращает (строя при первом обращении и кэшируя) syntect-тему подсветки кода
/// для данной палитры.
fn code_theme(palette: &Palette) -> &'static Theme {
    let mut cache = CODE_THEMES.lock().expect("CODE_THEMES poisoned");
    cache
        .entry(*palette)
        .or_insert_with(|| Box::leak(Box::new(build_code_theme(palette))))
}

// ---------- подмодули (разбор god-object: docs/history/refactoring-god-objects.md, этап 6) ----------

mod code;
mod latex;
mod table;
mod writer;

// Внутренняя проводка: Writer (writer) + подсветка (code) + таблицы (table) +
// LaTeX (latex) видны друг другу и mod.rs через реэкспорт (внешняя поверхность —
// только render/render_with, определены здесь).
use self::{code::*, latex::*, table::*, writer::*};

/// Общие тест-хелперы рендера, используемые тестами нескольких подмодулей
/// (code/latex/table/writer). См. docs/history/refactoring-god-objects.md.
#[cfg(test)]
pub(super) mod testkit {
    use super::*;

    /// Собирает весь текст рендера в одну строку (для проверок содержимого):
    /// спаны одной строки склеиваются без разделителя, строки — через `\n`.
    pub(super) fn rendered_text(input: &str) -> String {
        rendered_text_w(input, 80)
    }

    /// Как [`rendered_text`], но с заданной шириной.
    pub(super) fn rendered_text_w(input: &str, width: usize) -> String {
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

    /// Максимальная ширина (в колонках) среди строк рендера.
    pub(super) fn max_line_width(input: &str, width: usize) -> usize {
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

    /// Собирает множество цветов переднего плана спанов рендера.
    pub(super) fn fg_colors(input: &str, palette: &Palette) -> Vec<Color> {
        render(input, 80, palette)
            .lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter_map(|s| s.style.fg)
            .collect()
    }

    pub(super) const TABLE_MD: &str = "\
| Алгоритм | Время | Память |
| :--- | :--- | :--- |
| QuickSort | O(n log n) | O(log n) |
| MergeSort | O(n log n) | O(n) |";

    pub(super) const CODE_MD: &str = "```rust\nfn main() {\n    let s = \"hi\";\n    // c\n}\n```";
}

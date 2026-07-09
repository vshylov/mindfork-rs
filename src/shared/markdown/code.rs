//! Markdown — подсветка блоков кода (syntect: синтаксис + тема из палитры). Часть модуля [`super`]; разбито из монолита
//! markdown.rs (см. docs/history/refactoring-god-objects.md, этап 6).

use super::*;

// ---------- writer: pulldown events → строки ----------

pub(super) static SYNTAX_SET: LazyLock<SyntaxSet> =
    LazyLock::new(SyntaxSet::load_defaults_newlines);

/// Резолвит метку языка код-блока (` ```csharp `) в синтаксис syntect.
///
/// `SyntaxSet::find_syntax_by_token` в дефолтном наборе Sublime сопоставляет метку
/// либо расширению файла (`rs`, `cs`), либо **имени** синтаксиса регистронезависимо
/// (`Rust`, `C#`). Поэтому `rust` находится по счастливому совпадению с именем
/// `Rust`, а распространённые метки моделей вроде `csharp`/`c++`/`golang` не
/// совпадают ни с именем (`C#`, `C++`, `Go`), ни с расширением — и код остаётся без
/// подсветки. Таблица [`canonical_lang`] приводит такие алиасы к токену, который
/// набор распознаёт; при промахе пробуем исходную метку (вдруг это уже валидное
/// расширение/имя, которого нет в таблице).
pub(super) fn resolve_syntax(lang: &str) -> Option<&'static SyntaxReference> {
    if lang.is_empty() {
        return None;
    }
    let canonical = canonical_lang(lang);
    SYNTAX_SET
        .find_syntax_by_token(canonical)
        .or_else(|| SYNTAX_SET.find_syntax_by_token(lang))
}

/// Сводит алиас языка к токену (расширению/имени), понятному дефолтному набору
/// syntect. Регистр метки игнорируется. Немаппированная метка возвращается как есть
/// (её пробует распознать сам `find_syntax_by_token`).
///
/// Ключи — типичные метки, которыми Gemma/Qwen/Claude размечают код-блоки. **Все
/// цели сверены с дефолтным бандлом** (`SyntaxSet::load_defaults_newlines`) — набор
/// узкий (нет TypeScript/Kotlin/PowerShell/Dockerfile/TOML/Swift/…), поэтому мапить
/// в несуществующий синтаксис бессмысленно. Уже резолвящиеся метки (`rust`,
/// `python`, `go`, `js`, `java`, `ruby`, `php`, `sql`, `html`, `css`, `json`,
/// `yaml`, `bash`, `c`, `c++`, `c#`/`cs`, …) в таблицу не вносим.
pub(super) fn canonical_lang(lang: &str) -> &str {
    match lang.trim().to_ascii_lowercase().as_str() {
        // --- прямые алиасы: цель есть в наборе, но метка с ней не совпадает ---
        "csharp" | "cs-script" | "dotnet" => "cs", // имя "C#" ≠ "csharp"
        "cpp" | "cplusplus" | "cxx" | "cc" => "c++", // имя "C++" ≠ "cpp"
        "objc" | "objective-c" | "objectivec" | "obj-c" => "m", // имя "Objective-C"
        "objcpp" | "objc++" | "objective-c++" => "mm", // имя "Objective-C++"
        "golang" => "go",
        "rustlang" => "rs",
        "python3" | "py3" | "python2" => "py",
        "node" | "nodejs" => "js",
        "shell" | "sh" | "zsh" | "console" | "shell-session" | "shellsession" => "bash",
        "yml" | "yaml-frontmatter" | "frontmatter" => "yaml",
        "rlang" => "r",
        // --- приближения: языка нет в наборе, берём близкий по синтаксису ---
        // Лучше частичная подсветка родственным грамматиком, чем серый текст.
        "typescript" | "ts" | "tsx" | "mts" | "cts" | "jsx" => "js", // база JS
        "kotlin" | "kt" | "kts" => "java",
        other => {
            // Немаппированную метку возвращаем как есть; заимствование из исходной
            // строки, поэтому отдаём срез `lang`, а не временный lowercase-буфер.
            let _ = other;
            lang.trim()
        }
    }
}

/// Строит syntect-тему подсветки кода из семантической [`Palette`], сопоставляя
/// синтаксические scope'ы ролям темы: ключевые слова → `accent`, строки →
/// `success`, числа/константы → `warning`, функции → `user`, типы → `assistant`,
/// теги → `accent`. Текст «по умолчанию» и комментарии задаются абсолютным серым,
/// светлым на тёмном фоне и тёмным на светлом (`palette.dark`) — так подсветка
/// согласована с темой приложения, а не живёт «своей палитрой» (ADR 0003).
///
/// Пайплайн рендера (`as_24_bit_terminal_escaped(.., false)`) переносит **только
/// цвет переднего плана**, поэтому фон/жирность/курсив в теме не задаём.
pub(super) fn build_code_theme(palette: &Palette) -> Theme {
    // Серые, у которых нет адаптируемого ANSI-аналога — выбираем по светлоте фона.
    let (default_fg, comment) = if palette.dark {
        (gray(212), gray(128))
    } else {
        (gray(40), gray(110))
    };

    let settings = ThemeSettings {
        foreground: Some(default_fg),
        ..Default::default()
    };

    // Список (scope-селектор → цвет роли). Самый специфичный селектор побеждает
    // (syntect выбирает по «силе совпадения»), порядок в векторе не важен.
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

/// Непрозрачный оттенок серого `v` по всем каналам.
pub(super) fn gray(v: u8) -> SynColor {
    SynColor {
        r: v,
        g: v,
        b: v,
        a: 255,
    }
}

/// Один элемент темы: scope-селектор(ы) → цвет переднего плана.
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

/// Переводит цвет ratatui в RGB-цвет syntect. Именованные ANSI-цвета (у `Auto`/
/// `Dark` палитра именованная, адаптируемая терминалом) приводятся к стандартным
/// RGB (палитра Campbell — дефолт Windows Terminal): подсветка кода всё равно
/// эмитит 24-битный цвет, так что иначе нельзя. `Rgb` копируется как есть.
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

    #[test]
    fn highlight_code_has_no_fences_and_is_colored() {
        // Хелпер для tool-карточек: подсветка без ограждающих ```, с RGB-цветами.
        let lines = highlight_code("fn main() {}", "rust", &Palette::for_theme(Theme::Dark));
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("fn main"), "содержимое кода: {joined}");
        assert!(!joined.contains("```"), "не должно быть заборов: {joined}");
        assert!(
            lines
                .iter()
                .flat_map(|l| l.spans.iter())
                .any(|s| matches!(s.style.fg, Some(Color::Rgb(..)))),
            "ожидалась RGB-подсветка"
        );
    }

    #[test]
    fn highlight_code_unknown_lang_falls_back_to_plain() {
        // Нераспознанный язык → строки без подсветки (без паники, текст цел).
        let lines = highlight_code("a\nb", "нет-такого-языка", &Palette::default());
        assert_eq!(lines.len(), 2);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert_eq!(joined, "ab");
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
}

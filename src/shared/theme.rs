//! Палитра тем оформления TUI (spec §11.6). Семантические цветовые роли,
//! разрешаемые из [`Theme`]. Виджеты берут цвета из палитры, а не хардкодят
//! `.cyan()`/`.green()`/… — это даёт читаемость на светлом/тёмном фоне.
//!
//! Палитра и хелперы реализуют редизайн TUI (`docs/redesign/`): спокойные рамки
//! со скруглёнными бордерами, цветные рейлы ролей сообщений, статус-«пилюли» и
//! тихая строка хоткеев с «клавишными» лейблами. Точные оттенки тёмной темы —
//! из дизайн-макета (oklch → sRGB); `Auto` использует именованные ANSI-цвета,
//! подстраивающиеся под тему терминала, `Light` — затемнённые варианты.
//!
//! Атрибуты-модификаторы (`dim`/`bold`/`reversed`/`underlined`) тема-независимы
//! (адаптируются терминалом) и остаются в виджетах как есть — палитра задаёт
//! только сами цвета.
//!
//! Помимо цветов палитра несёт **набор глифов** ([`GlyphSet`], метод
//! [`Palette::glyphs`]): в режиме совместимости со старыми терминалами
//! (`config.interface.terminal_compat`, spec §11.6) декоративные эмодзи и редкие
//! символы Юникода заменяются на безопасные, а скруглённые рамки — на прямые.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders};

use crate::shared::config::Theme;
use crate::shared::wrap;

/// Набор глифов интерфейса. [`UNICODE_GLYPHS`] — вид редизайна (эмодзи и
/// декоративные символы, требуют современный терминал/шрифт с фолбэком:
/// Windows Terminal и т.п.); [`COMPAT_GLYPHS`] — режим совместимости со старыми
/// эмуляторами (conhost Windows 10 и др.), где эмодзи и редкие глифы рисуются
/// квадратами-«тофу».
///
/// Ориентир совместимого набора — **WGL4** (базовый репертуар шрифтов Windows:
/// Consolas/Lucida Console его покрывают) плюс ASCII: поэтому здесь допустимы
/// `●`/`○`/`►`/`▼`/`♦`/`√`/`×`/`≡`/`»`, а также box-drawing (`─│└┼`), блоки
/// (`▌█░`) и стрелки (`←↑↓→`) — они остаются и в других местах UI (рейлы,
/// таблицы markdown, скроллбар) без замены. Вне набора — эмодзи (`⚒`, `✻`, `➕`),
/// «редкие» символы (`✦❯▸▾◆▤⌨⚙⌕▏✕⚠✓✗⟳`), Брайль-спиннер (`⠋⠙…`) и
/// арк-сегменты скруглённых рамок (`╭╮╰╯`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphSet {
    /// Тип рамки панелей: скруглённая / прямая (арк-сегменты `╭╮╰╯` есть не во
    /// всех консольных шрифтах).
    pub border: BorderType,
    /// Иконка ассистента («✦») — заголовок реплики, титулы панелей.
    pub assistant_icon: &'static str,
    /// Иконка пользователя («❯») — заголовок реплики.
    pub user_icon: &'static str,
    /// Колонка приглашения поля ввода («❯ »; ширина 2 колонки в обоих наборах,
    /// см. `input_box::PROMPT_W`).
    pub prompt: &'static str,
    /// Префикс первого ряда tool-карточки («⚒  »: эмодзи рисуется шириной 2,
    /// поэтому за ним два пробела — см. `message_feed::push_tool`).
    pub tool_head: &'static str,
    /// Отступ продолжений tool-карточки — той же **счётной** ширины, что
    /// [`Self::tool_head`] (выравнивание переносов).
    pub tool_cont: &'static str,
    /// Маркер свёрнутого блока («▸») — пилюля «мыслей», титул «Секции».
    pub collapsed: &'static str,
    /// Маркер развёрнутого блока («▾») — блок «мыслей».
    pub expanded: &'static str,
    /// Маркер титула панели/секции («◆») — лента чата, секция настроек.
    pub title_marker: &'static str,
    /// Иконка панели списка чатов («▤»).
    pub chats_icon: &'static str,
    /// Иконка панели настроек («⚙  », ширина-2-эмодзи → два пробела).
    pub settings_icon: &'static str,
    /// Иконка попапа помощи по клавишам («⌨  », ширина-2-эмодзи → два пробела).
    pub help_icon: &'static str,
    /// Подтверждение/успех («✓»).
    pub ok: &'static str,
    /// Отказ/брошенная цель («✗»).
    pub failed: &'static str,
    /// Предупреждение/ошибка («⚠»).
    pub warn: &'static str,
    /// Статус «подключение…» в чипе сервера («◐»; готовность — `●`, он в WGL4
    /// и не заменяется).
    pub status_connecting: &'static str,
    /// Статус «нет связи/не настроен» в чипе сервера («✕»).
    pub status_off: &'static str,
    /// Индикатор активной генерации в статус-баре («⟳»).
    pub busy: &'static str,
    /// Индикатор фоновой задачи в статус-баре («✻»); ширина 1 колонка — от неё
    /// зависит раскладка сетки хоткеев.
    pub background: &'static str,
    /// Иконка строки поиска («⌕»).
    pub search: &'static str,
    /// Псевдокурсор строки поиска («▏»).
    pub caret: &'static str,
    /// Пункт «добавить в словарь» в подсказках орфографии («➕»).
    pub add: &'static str,
    /// Кадры спиннера фоновых операций (RAG-индексация, имперсонация).
    pub spinner: &'static [char],
}

/// Глифы редизайна (по умолчанию): эмодзи и декоративные символы Юникода.
pub static UNICODE_GLYPHS: GlyphSet = GlyphSet {
    border: BorderType::Rounded,
    assistant_icon: "✦",
    user_icon: "❯",
    prompt: "❯ ",
    tool_head: "⚒  ",
    tool_cont: "   ",
    collapsed: "▸",
    expanded: "▾",
    title_marker: "◆",
    chats_icon: "▤",
    settings_icon: "⚙  ",
    help_icon: "⌨  ",
    ok: "✓",
    failed: "✗",
    warn: "⚠",
    status_connecting: "◐",
    status_off: "✕",
    busy: "⟳",
    background: "✻",
    search: "⌕",
    caret: "▏",
    add: "➕",
    spinner: &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'],
};

/// Глифы режима совместимости: только WGL4/ASCII (см. док-коммент [`GlyphSet`]).
pub static COMPAT_GLYPHS: GlyphSet = GlyphSet {
    border: BorderType::Plain,
    assistant_icon: "*",
    user_icon: ">",
    prompt: "> ",
    tool_head: "# ",
    tool_cont: "  ",
    collapsed: "►",
    expanded: "▼",
    title_marker: "♦",
    chats_icon: "≡",
    settings_icon: "# ",
    help_icon: "# ",
    ok: "√",
    failed: "×",
    warn: "!",
    status_connecting: "○",
    status_off: "×",
    busy: "»",
    background: "*",
    search: "?",
    caret: "│",
    add: "+",
    spinner: &['|', '/', '-', '\\'],
};

/// Семантические цвета интерфейса. `Copy` — дёшево передавать в render по значению.
/// `Hash` — палитра служит ключом кэша (напр. построенной syntect-темы подсветки
/// кода в `shared/markdown.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Palette {
    /// Рейл/акцент реплики пользователя (синий).
    pub user: Color,
    /// Рейл/акцент реплики ассистента (зелёный).
    pub assistant: Color,
    /// Tool-блоки (имя/аргументы инструмента; янтарный).
    pub tool: Color,
    /// Успех/готовность (статус «готов», маркер активного чата).
    pub success: Color,
    /// Предупреждение (подключение/не настроено, индикатор правки).
    pub warning: Color,
    /// Ошибка (нет связи, орфографическая ошибка).
    pub error: Color,
    /// Акцент (индикатор генерации/токенов).
    pub accent: Color,
    /// «Мягкий» (светлее/менее насыщенный) вариант цвета пользователя — для
    /// текста-заголовка реплики (рейл — насыщенный `user`).
    pub user_soft: Color,
    /// «Мягкий» вариант цвета ассистента (текст заголовка реплики).
    pub assistant_soft: Color,
    /// «Мягкий» вариант цвета инструмента (имя в карточке tool-колла).
    pub tool_soft: Color,
    /// Основной цвет текста.
    pub text: Color,
    /// Приглушённый текст (вторичные подписи, описания хоткеев).
    pub muted: Color,
    /// Цвет обычной рамки панели.
    pub border: Color,
    /// Цвет рамки сфокусированной панели (ввод, активный список).
    pub border_focus: Color,
    /// Текст «клавиши» в строке хоткеев.
    pub keycap_fg: Color,
    /// Фон «клавиши» в строке хоткеев.
    pub keycap_bg: Color,
    /// Текст «опасной» клавиши (напр. `Del` удаления) на фоне `keycap_bg`. Отдельно
    /// от `error`: «клавиша» приглушённая и тёмная, поэтому красный для неё берётся
    /// **ярче** обычного `error`, чтобы читаться и не сливаться с тёмным фоном пилюли.
    pub keycap_danger: Color,
    /// Тёмный ли фон темы. Нужно там, где цвет приходится задавать абсолютным RGB
    /// (нет именованного ANSI, адаптируемого терминалом) — напр. серый «по умолчанию»
    /// и цвет комментариев в подсветке кода: на тёмном фоне светлый, на светлом —
    /// тёмный. `Auto` считаем тёмным (типичный терминал тёмный; так было до тем).
    pub dark: bool,
    /// Режим совместимости со старым терминалом (`config.interface.terminal_compat`):
    /// глифы берутся из [`COMPAT_GLYPHS`] (см. [`Palette::glyphs`]), а затемнение
    /// фона попапов идёт цветом вместо `DIM` (`shared/ui.rs::dim_background`).
    /// Не цвет, но живёт в палитре (как `dark`): она уже протянута во все render.
    pub compat: bool,
}

impl Palette {
    /// Палитра для выбранной темы.
    pub fn for_theme(theme: Theme) -> Self {
        match theme {
            Theme::Auto => Self::auto(),
            Theme::Dark => Self::dark(),
            Theme::Light => Self::light(),
        }
    }

    /// «Авто» — именованные ANSI-цвета: их оттенки задаёт сам терминал, поэтому
    /// палитра подстраивается под тему терминала (поведение до редизайна).
    /// Структурные цвета (рамки/приглушённый/клавиши) — нейтральные ANSI-серые.
    fn auto() -> Self {
        Self {
            user: Color::Cyan,
            assistant: Color::Green,
            tool: Color::Yellow,
            success: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,
            accent: Color::Magenta,
            user_soft: Color::LightCyan,
            assistant_soft: Color::LightGreen,
            tool_soft: Color::LightYellow,
            text: Color::Reset,
            muted: Color::DarkGray,
            border: Color::DarkGray,
            border_focus: Color::Gray,
            // «Клавиши» — тихие тёмные пилюли (приглушённый текст на фоне чуть выше
            // типичного тёмного фона терминала), чтобы не перетягивать внимание.
            // Named ANSI не имеет шага темнее `DarkGray`, поэтому здесь абсолютный RGB
            // (Auto и так считаем тёмной темой — флаг `dark: true`).
            keycap_fg: Color::Rgb(138, 144, 152),
            keycap_bg: Color::Rgb(36, 39, 45),
            keycap_danger: Color::Rgb(226, 110, 98),
            dark: true,
            compat: false,
        }
    }

    /// Тёмная тема — точные оттенки дизайн-макета (oklch → sRGB). Спокойная,
    /// «терминальная»: глубокий фон у терминала, мягкие рамки, насыщенные рейлы.
    fn dark() -> Self {
        Self {
            user: Color::Rgb(121, 169, 219),           // blue
            assistant: Color::Rgb(111, 192, 130),      // green
            tool: Color::Rgb(220, 175, 97),            // amber
            success: Color::Rgb(111, 192, 130),        // green
            warning: Color::Rgb(220, 175, 97),         // amber
            error: Color::Rgb(223, 105, 92),           // red
            accent: Color::Rgb(220, 175, 97),          // amber (генерация/токены)
            user_soft: Color::Rgb(144, 188, 233),      // blueSoft
            assistant_soft: Color::Rgb(164, 209, 172), // greenSoft
            tool_soft: Color::Rgb(230, 201, 154),      // amberSoft
            text: Color::Rgb(201, 204, 210),           // #c9ccd2
            muted: Color::Rgb(126, 132, 139),          // #7e848b
            border: Color::Rgb(54, 58, 66),            // чуть ярче макетного #24272e для видимости
            border_focus: Color::Rgb(110, 117, 128),   // #6e7580 (фокус)
            keycap_fg: Color::Rgb(132, 138, 146),      // тише прежнего #9aa0a7
            keycap_bg: Color::Rgb(33, 36, 42),         // темнее прежнего #2a2d34
            keycap_danger: Color::Rgb(232, 116, 104),  // ярче error для читаемости на пилюле
            dark: true,
            compat: false,
        }
    }

    /// Светлая тема — затемнённые цвета (жёлтый/яркие на белом нечитаемы).
    fn light() -> Self {
        Self {
            user: Color::Blue,
            assistant: Color::Rgb(0, 128, 0),
            tool: Color::Rgb(160, 100, 0),
            success: Color::Rgb(0, 128, 0),
            warning: Color::Rgb(160, 100, 0),
            error: Color::Rgb(180, 0, 0),
            accent: Color::Rgb(140, 0, 140),
            user_soft: Color::Rgb(40, 80, 170),
            assistant_soft: Color::Rgb(0, 110, 0),
            tool_soft: Color::Rgb(150, 95, 0),
            text: Color::Rgb(30, 32, 36),
            muted: Color::Rgb(110, 116, 124),
            border: Color::Rgb(190, 193, 198),
            border_focus: Color::Rgb(120, 124, 130),
            keycap_fg: Color::Rgb(74, 78, 84), // мягче чёрного — «клавиши» не кричат
            keycap_bg: Color::Rgb(222, 224, 228),
            keycap_danger: Color::Rgb(178, 34, 34),
            dark: false,
            compat: false,
        }
    }

    /// Та же палитра с выставленным режимом совместимости (builder-стиль; удобно
    /// на месте: `Palette::for_theme(t).with_compat(flag)`).
    pub fn with_compat(mut self, compat: bool) -> Self {
        self.compat = compat;
        self
    }

    /// Активный набор глифов: юникодный по умолчанию, безопасный — в режиме
    /// совместимости со старым терминалом. См. [`GlyphSet`].
    pub fn glyphs(&self) -> &'static GlyphSet {
        if self.compat {
            &COMPAT_GLYPHS
        } else {
            &UNICODE_GLYPHS
        }
    }

    // ---- Удобные конструкторы стилей (foreground по роли) ----
    pub fn success_style(&self) -> Style {
        Style::new().fg(self.success)
    }
    pub fn accent_style(&self) -> Style {
        Style::new().fg(self.accent)
    }
    /// Стиль приглушённого текста (вторичные подписи/описания).
    pub fn muted_style(&self) -> Style {
        Style::new().fg(self.muted)
    }

    /// Стиль рамки панели (`focused` → выделенная рамка).
    pub fn border_style(&self, focused: bool) -> Style {
        Style::new().fg(if focused {
            self.border_focus
        } else {
            self.border
        })
    }

    /// Скруглённая панель (`Block`) с титулом и опциональным фокусом — единый
    /// «фрейм» редизайна. Титул рисуется на верхней линии, рамка — цветом палитры;
    /// в режиме совместимости рамка прямая (см. [`GlyphSet::border`]).
    pub fn panel(&self, title: impl Into<String>, focused: bool) -> Block<'static> {
        Block::default()
            .borders(Borders::ALL)
            .border_type(self.glyphs().border)
            .border_style(self.border_style(focused))
            .title(Span::styled(
                format!(" {} ", title.into()),
                Style::new().fg(self.text),
            ))
    }

    /// «Клавиша» хоткея — лейбл на приглушённом фоне (как пилюля в макете).
    pub fn keycap(&self, label: impl Into<String>) -> Span<'static> {
        Span::styled(
            format!(" {} ", label.into()),
            Style::new().fg(self.keycap_fg).bg(self.keycap_bg),
        )
    }

    /// Подсказка-хоткей: «клавиша» + приглушённое описание. Возвращает спаны для
    /// вставки в строку (с ведущим пробелом-отступом перед клавишей).
    pub fn hint(&self, key: &str, desc: &str) -> Vec<Span<'static>> {
        vec![
            self.keycap(key),
            Span::styled(format!(" {desc}"), self.muted_style()),
        ]
    }

    /// Как `hint`, но **значение после двоеточия** выделено цветом `color`, а подпись
    /// (само двоеточие включительно) остаётся приглушённой — для подсветки значения
    /// активного режима (напр. «прокрутка» в «мышь: прокрутка» цветом `accent`, как
    /// заголовки markdown в ленте). Разбиение по `:`, а не по пробелу, корректно для
    /// многословных значений (важно при будущей локализации). Без двоеточия — всё
    /// описание выделяется целиком.
    pub fn hint_highlight_value(&self, key: &str, desc: &str, color: Color) -> Vec<Span<'static>> {
        let mut spans = vec![self.keycap(key)];
        match desc.split_once(':') {
            Some((label, value)) => {
                spans.push(Span::styled(format!(" {label}: "), self.muted_style()));
                spans.push(Span::styled(
                    value.trim().to_string(),
                    Style::new().fg(color),
                ));
            }
            None => spans.push(Span::styled(format!(" {desc}"), Style::new().fg(color))),
        }
        spans
    }

    /// Раскладывает подсказки-хоткеи в аккуратную сетку под ширину `width`:
    /// «клавиша» + приглушённое описание, столбцы совпадают по вертикали. Число
    /// столбцов подбирается максимальным из влезающих в ширину (→ минимум строк);
    /// при переполнении одной строки хоткеи переносятся на следующие. `danger`
    /// помечает «опасную» клавишу (напр. удаление) красным. Используется в
    /// статус-баре экрана чата и в оверлее списка чатов — для одинакового вида.
    pub fn hotkey_grid(&self, items: &[(&str, &str, bool)], width: usize) -> Vec<Line<'static>> {
        let n = items.len();
        if n == 0 {
            return Vec::new();
        }
        // Ширина ячейки = «клавиша» (символы + 2 на отступы) + пробел + описание.
        let cell_w: Vec<usize> = items
            .iter()
            .map(|(key, desc, _)| keycap_width(key) + 1 + str_width(desc))
            .collect();
        const GAP: usize = 3; // зазор между столбцами

        // Ширины столбцов при `cols` колонках (row-major раскладка).
        let col_widths = |cols: usize| -> Vec<usize> {
            let mut w = vec![0usize; cols];
            for (i, cw) in cell_w.iter().enumerate() {
                w[i % cols] = w[i % cols].max(*cw);
            }
            w
        };
        // Подбираем максимум столбцов, влезающих в ширину (→ минимум строк).
        let mut cols = 1;
        for c in (1..=n).rev() {
            let total: usize = col_widths(c).iter().sum::<usize>() + GAP * c.saturating_sub(1);
            if total <= width {
                cols = c;
                break;
            }
        }
        let widths = col_widths(cols);

        // Раскладываем по строкам; каждую ячейку добиваем до ширины столбца, чтобы
        // столбцы совпадали по вертикали.
        let mut lines: Vec<Line<'static>> = Vec::new();
        for row in items.chunks(cols) {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (c, (key, desc, danger)) in row.iter().enumerate() {
                let cap = if *danger {
                    Span::styled(
                        format!(" {key} "),
                        Style::new().fg(self.keycap_danger).bg(self.keycap_bg),
                    )
                } else {
                    self.keycap(*key)
                };
                spans.push(cap);
                spans.push(Span::styled(format!(" {desc}"), self.muted_style()));
                let used = keycap_width(key) + 1 + str_width(desc);
                let pad = widths[c].saturating_sub(used) + if c + 1 < cols { GAP } else { 0 };
                if pad > 0 {
                    spans.push(Span::raw(" ".repeat(pad)));
                }
            }
            lines.push(Line::from(spans));
        }
        lines
    }
}

/// Видимая ширина строки в колонках терминала.
fn str_width(s: &str) -> usize {
    wrap::display_width(&s.chars().collect::<Vec<_>>())
}

/// Ширина «клавиши» в колонках: символы лейбла + 2 (отступы вокруг, как в
/// [`Palette::keycap`], который форматирует `" {label} "`).
fn keycap_width(label: &str) -> usize {
    str_width(label) + 2
}

impl Default for Palette {
    fn default() -> Self {
        Self::auto()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_uses_named_ansi_colors() {
        let p = Palette::for_theme(Theme::Auto);
        assert_eq!(p.user, Color::Cyan);
        assert_eq!(p.error, Color::Red);
    }

    #[test]
    fn dark_and_light_differ_from_auto() {
        let auto = Palette::for_theme(Theme::Auto);
        assert_ne!(Palette::for_theme(Theme::Dark), auto);
        assert_ne!(Palette::for_theme(Theme::Light), auto);
    }

    #[test]
    fn default_is_auto() {
        assert_eq!(Palette::default(), Palette::for_theme(Theme::Auto));
    }

    #[test]
    fn dark_flag_follows_theme() {
        // Auto и Dark считаем тёмными, Light — светлой (для абсолютных RGB-цветов
        // подсветки кода, у которых нет адаптируемого ANSI-аналога).
        assert!(Palette::for_theme(Theme::Auto).dark);
        assert!(Palette::for_theme(Theme::Dark).dark);
        assert!(!Palette::for_theme(Theme::Light).dark);
    }

    #[test]
    fn keycap_and_hint_carry_label() {
        let p = Palette::default();
        let cap = p.keycap("Ctrl+N");
        assert!(cap.content.contains("Ctrl+N"));
        let hint = p.hint("Enter", "отправить");
        let joined: String = hint.iter().map(|s| s.content.as_ref()).collect();
        assert!(joined.contains("Enter") && joined.contains("отправить"));
    }

    #[test]
    fn glyphs_follow_compat_flag() {
        // По умолчанию — юникодный набор и скруглённые рамки; в режиме
        // совместимости — безопасный набор и прямые рамки.
        let p = Palette::default();
        assert!(!p.compat);
        assert_eq!(p.glyphs(), &UNICODE_GLYPHS);
        assert_eq!(p.glyphs().border, BorderType::Rounded);
        let c = p.with_compat(true);
        assert_eq!(c.glyphs(), &COMPAT_GLYPHS);
        assert_eq!(c.glyphs().border, BorderType::Plain);
    }

    #[test]
    fn compat_glyphs_avoid_rare_symbols() {
        // В компат-набор не должны просочиться заменяемые эмодзи/редкие символы
        // (они и есть причина режима: старый терминал рисует их «тофу»).
        let banned: Vec<char> = "✦❯⚒▸▾◆▤⚙⌨✓✗⚠◐✕⟳✻⌕▏➕".chars().collect();
        let g = &COMPAT_GLYPHS;
        let all = [
            g.assistant_icon,
            g.user_icon,
            g.prompt,
            g.tool_head,
            g.tool_cont,
            g.collapsed,
            g.expanded,
            g.title_marker,
            g.chats_icon,
            g.settings_icon,
            g.help_icon,
            g.ok,
            g.failed,
            g.warn,
            g.status_connecting,
            g.status_off,
            g.busy,
            g.background,
            g.search,
            g.caret,
            g.add,
        ];
        for s in all {
            for ch in s.chars() {
                assert!(
                    !banned.contains(&ch),
                    "редкий символ в компат-наборе: {ch:?}"
                );
            }
        }
        // Спиннер — чистый ASCII (Брайль-кадры старые консоли не рисуют).
        assert!(g.spinner.iter().all(|c| c.is_ascii()), "{:?}", g.spinner);
    }

    #[test]
    fn tool_head_and_cont_widths_match_in_both_sets() {
        // Продолжения tool-карточки выравниваются под первый ряд — счётные ширины
        // префиксов должны совпадать в обоих наборах (см. message_feed::push_tool).
        for g in [&UNICODE_GLYPHS, &COMPAT_GLYPHS] {
            assert_eq!(str_width(g.tool_head), str_width(g.tool_cont));
        }
        // Колонка приглашения ввода — всегда 2 колонки (input_box::PROMPT_W).
        assert_eq!(str_width(UNICODE_GLYPHS.prompt), 2);
        assert_eq!(str_width(COMPAT_GLYPHS.prompt), 2);
    }
}

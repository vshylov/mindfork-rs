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

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders};

use crate::shared::config::Theme;
use crate::shared::wrap;

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
    /// «фрейм» редизайна. Титул рисуется на верхней линии, рамка — цветом палитры.
    pub fn panel(&self, title: impl Into<String>, focused: bool) -> Block<'static> {
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
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

    /// Как `hint`, но **последнее слово** описания выделено цветом `color`, а начало
    /// остаётся приглушённым — для подсветки значения активного режима (напр. слова
    /// «прокрутка» в «мышь: прокрутка» цветом `accent`, как заголовки markdown в ленте).
    pub fn hint_highlight_word(&self, key: &str, desc: &str, color: Color) -> Vec<Span<'static>> {
        let mut spans = vec![self.keycap(key)];
        match desc.rsplit_once(' ') {
            Some((head, word)) => {
                spans.push(Span::styled(format!(" {head} "), self.muted_style()));
                spans.push(Span::styled(word.to_string(), Style::new().fg(color)));
            }
            None => spans.push(Span::styled(format!(" {desc}"), Style::new().fg(color))),
        }
        spans
    }

    /// Статус-«пилюля»: точка-индикатор `●` + подпись, оба цветом `color`
    /// (роль статуса). Используется в статус-баре.
    pub fn pill(&self, label: &str, color: Color) -> Vec<Span<'static>> {
        vec![
            Span::styled("●", Style::new().fg(color).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" {label}"), Style::new().fg(color)),
        ]
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
}

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
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders};

use crate::shared::config::Theme;

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
            keycap_fg: Color::White,
            keycap_bg: Color::DarkGray,
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
            keycap_fg: Color::Rgb(154, 160, 167),      // #9aa0a7
            keycap_bg: Color::Rgb(42, 45, 52),         // #2a2d34
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
            keycap_fg: Color::Rgb(40, 42, 46),
            keycap_bg: Color::Rgb(222, 224, 228),
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

    /// Статус-«пилюля»: точка-индикатор `●` + подпись, оба цветом `color`
    /// (роль статуса). Используется в статус-баре.
    pub fn pill(&self, label: &str, color: Color) -> Vec<Span<'static>> {
        vec![
            Span::styled("●", Style::new().fg(color).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" {label}"), Style::new().fg(color)),
        ]
    }
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

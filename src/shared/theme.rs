//! Палитра тем оформления TUI (spec §11.6). Семантические цветовые роли,
//! разрешаемые из [`Theme`]. Виджеты берут цвета из палитры, а не хардкодят
//! `.cyan()`/`.green()`/… — это даёт читаемость на светлом/тёмном фоне.
//!
//! Атрибуты-модификаторы (`dim`/`bold`/`reversed`/`underlined`) тема-независимы
//! (адаптируются терминалом) и остаются в виджетах как есть — палитра задаёт
//! только сами цвета.

use ratatui::style::{Color, Style};

use crate::shared::config::Theme;

/// Семантические цвета интерфейса. `Copy` — дёшево передавать в render по значению.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Заголовок реплики пользователя.
    pub user: Color,
    /// Заголовок реплики ассистента.
    pub assistant: Color,
    /// Tool-блоки (имя/аргументы инструмента).
    pub tool: Color,
    /// Успех/готовность (статус «готов», маркер активного чата).
    pub success: Color,
    /// Предупреждение (подключение/не настроено, индикатор правки).
    pub warning: Color,
    /// Ошибка (нет связи, орфографическая ошибка).
    pub error: Color,
    /// Акцент (индикатор генерации).
    pub accent: Color,
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
    /// палитра подстраивается под тему терминала (поведение до M9).
    fn auto() -> Self {
        Self {
            user: Color::Cyan,
            assistant: Color::Green,
            tool: Color::Yellow,
            success: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,
            accent: Color::Magenta,
        }
    }

    /// Тёмная тема — яркие варианты (читаемы на тёмном фоне).
    fn dark() -> Self {
        Self {
            user: Color::LightCyan,
            assistant: Color::LightGreen,
            tool: Color::LightYellow,
            success: Color::LightGreen,
            warning: Color::LightYellow,
            error: Color::LightRed,
            accent: Color::LightMagenta,
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
        }
    }

    // Удобные конструкторы стилей (foreground по роли) для статус-бара.
    pub fn success_style(&self) -> Style {
        Style::new().fg(self.success)
    }
    pub fn warning_style(&self) -> Style {
        Style::new().fg(self.warning)
    }
    pub fn error_style(&self) -> Style {
        Style::new().fg(self.error)
    }
    pub fn accent_style(&self) -> Style {
        Style::new().fg(self.accent)
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
}

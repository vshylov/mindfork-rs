//! Статус-бар: состояние сервера, индикатор генерации, подсказки по клавишам.
//! См. spec §11.1. Модель/токены/контекст/профиль появятся позже (M5/M8).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};

use crate::shared::server::ServerStatus;
use crate::shared::theme::Palette;

/// Рисует строку статуса в `area`. `mouse_scroll` — включён ли захват мыши для
/// прокрутки колесом (иначе работает нативное выделение текста). См. spec §11.3.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    status: &ServerStatus,
    generating: bool,
    mouse_scroll: bool,
    palette: &Palette,
) {
    frame.render_widget(
        Line::from(spans(status, generating, mouse_scroll, palette)),
        area,
    );
}

/// Собирает спаны статус-строки (вынесено для тестируемости).
fn spans(
    status: &ServerStatus,
    generating: bool,
    mouse_scroll: bool,
    palette: &Palette,
) -> Vec<Span<'static>> {
    let (label, style) = match status {
        ServerStatus::NotConfigured => ("сервер не настроен".to_string(), palette.warning_style()),
        ServerStatus::Connecting => ("подключение…".to_string(), palette.warning_style()),
        ServerStatus::Ready => ("готов".to_string(), palette.success_style()),
        ServerStatus::Disconnected(why) => (format!("нет связи: {why}"), palette.error_style()),
    };
    let mut spans = vec![Span::from("сервер: "), Span::styled(label, style)];
    if generating {
        spans.push(Span::from("  •  ").dim());
        spans.push(Span::styled("генерация…", palette.accent_style()));
    }
    // Текущий режим мыши + клавиша тумблера (`Ctrl+W`), чтобы он был заметен.
    spans.push(Span::from("  •  ").dim());
    if mouse_scroll {
        spans.push(Span::styled(
            "мышь: прокрутка (Ctrl+W)",
            palette.accent_style(),
        ));
    } else {
        spans.push(Span::from("мышь: выделение (Ctrl+W)").dim());
    }
    spans.push(
        Span::from("  •  F1 справка · Esc чаты · Ctrl+N новый · Ctrl+P настройки · Ctrl+C выход")
            .dim(),
    );
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(status: &ServerStatus, generating: bool) -> String {
        spans(status, generating, false, &Palette::default())
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn shows_server_state() {
        assert!(text(&ServerStatus::Ready, false).contains("готов"));
        assert!(text(&ServerStatus::NotConfigured, false).contains("не настроен"));
        assert!(text(&ServerStatus::Disconnected("boom".into()), false).contains("boom"));
    }

    #[test]
    fn generating_indicator_toggles() {
        assert!(text(&ServerStatus::Ready, true).contains("генерация"));
        assert!(!text(&ServerStatus::Ready, false).contains("генерация"));
    }

    #[test]
    fn shows_mouse_mode() {
        let mode = |scroll| -> String {
            spans(&ServerStatus::Ready, false, scroll, &Palette::default())
                .iter()
                .map(|s| s.content.as_ref())
                .collect()
        };
        assert!(mode(true).contains("мышь: прокрутка"));
        assert!(mode(false).contains("мышь: выделение"));
    }

    #[test]
    fn render_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut term = Terminal::new(TestBackend::new(60, 1)).unwrap();
        term.draw(|f| {
            render(
                f,
                f.area(),
                &ServerStatus::Ready,
                true,
                true,
                &Palette::default(),
            )
        })
        .unwrap();
    }
}

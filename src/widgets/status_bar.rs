//! Статус-бар: состояние сервера, индикатор генерации, счётчик токенов
//! ответа/контекста, подсказки по клавишам. См. spec §11.1. Модель/профиль —
//! позже.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::shared::server::ServerStatus;
use crate::shared::theme::Palette;

/// Рисует строку статуса в `area`. `tokens` — токенов ответа (live), `context` —
/// токенов переписки (промпта; `None` — неизвестно), `context_exact` — точное ли
/// это число из `usage` сервера (иначе оценка, помечается `~`). `mouse_scroll` —
/// включён ли захват мыши для прокрутки колесом (иначе нативное выделение текста).
/// См. spec §11.1, §11.3.
#[allow(clippy::too_many_arguments)]
pub fn render(
    frame: &mut Frame,
    area: Rect,
    status: &ServerStatus,
    generating: bool,
    tokens: u64,
    context: Option<u64>,
    context_exact: bool,
    mouse_scroll: bool,
    palette: &Palette,
) {
    frame.render_widget(
        Line::from(spans(
            status,
            generating,
            tokens,
            context,
            context_exact,
            mouse_scroll,
            palette,
        )),
        area,
    );
}

/// Собирает спаны статус-строки (вынесено для тестируемости).
#[allow(clippy::too_many_arguments)]
fn spans(
    status: &ServerStatus,
    generating: bool,
    tokens: u64,
    context: Option<u64>,
    context_exact: bool,
    mouse_scroll: bool,
    palette: &Palette,
) -> Vec<Span<'static>> {
    // Разделитель-черта между группами (тихий, цветом рамки).
    let sep = || Span::styled("  │  ", Style::new().fg(palette.border));
    let muted = palette.muted_style();

    // Статус сервера — «пилюлей» (точка-индикатор + подпись цветом статуса).
    let (label, color) = match status {
        ServerStatus::NotConfigured => ("сервер не настроен".to_string(), palette.warning),
        ServerStatus::Connecting => ("сервер: подключение…".to_string(), palette.warning),
        ServerStatus::Ready => ("сервер: готов".to_string(), palette.success),
        ServerStatus::Disconnected(why) => (format!("сервер: нет связи: {why}"), palette.error),
    };
    let mut spans = palette.pill(&label, color);

    if generating {
        spans.push(sep());
        spans.push(Span::styled("⟳ генерация…", palette.accent_style()));
    }
    // Суммарный счётчик токенов (переписка + ответ): ярко во время генерации (растёт
    // live), приглушённо после (итог последнего хода). Помечается `~`, пока переписка
    // (промпт) — клиентская оценка, т.е. до прихода точного числа из `usage` сервера.
    if tokens > 0 || context.is_some() {
        spans.push(sep());
        let total = context.unwrap_or(0) + tokens;
        let approx = if context.is_some() && !context_exact {
            "~"
        } else {
            ""
        };
        let label = format!("токены: {approx}{total}");
        if generating {
            spans.push(Span::styled(label, palette.accent_style()));
        } else {
            spans.push(Span::styled(label, muted));
        }
    }
    // Текущий режим мыши + клавиша тумблера (`Ctrl+W`).
    spans.push(sep());
    spans.push(palette.keycap("Ctrl+W"));
    if mouse_scroll {
        spans.push(Span::styled(" мышь: прокрутка", palette.accent_style()));
    } else {
        spans.push(Span::styled(" мышь: выделение", muted));
    }
    // Тихая строка хоткеев — «клавиши» + приглушённые описания.
    spans.push(sep());
    for (key, desc) in [
        ("F1", "справка"),
        ("Esc", "чаты"),
        ("Ctrl+N", "новый"),
        ("Ctrl+P", "настройки"),
        ("Ctrl+C", "выход"),
    ] {
        spans.extend(palette.hint(key, desc));
        spans.push(Span::raw("   "));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(status: &ServerStatus, generating: bool) -> String {
        spans(
            status,
            generating,
            0,
            None,
            false,
            false,
            &Palette::default(),
        )
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
            spans(
                &ServerStatus::Ready,
                false,
                0,
                None,
                false,
                scroll,
                &Palette::default(),
            )
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
        };
        assert!(mode(true).contains("мышь: прокрутка"));
        assert!(mode(false).contains("мышь: выделение"));
    }

    #[test]
    fn token_counter_shows_summed_total() {
        let line = |tokens, context, exact| -> String {
            spans(
                &ServerStatus::Ready,
                true,
                tokens,
                context,
                exact,
                false,
                &Palette::default(),
            )
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
        };
        // Нет токенов и нет контекста — счётчик скрыт.
        assert!(!line(0, None, false).contains("токены"));
        // Только ответ (переписка неизвестна) — сумма = ответ, без `~`.
        assert!(line(42, None, false).contains("токены: 42"));
        // Сумма переписки и ответа; пока переписка — оценка, помечается `~`.
        assert!(line(42, Some(1000), false).contains("токены: ~1042"));
        // Точное число из usage — без `~`.
        assert!(line(42, Some(1000), true).contains("токены: 1042"));
        // Видно сразу при старте: переписка есть, ответ ещё 0 → сумма = переписка.
        assert!(line(0, Some(1000), false).contains("токены: ~1000"));
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
                123,
                Some(456),
                true,
                true,
                &Palette::default(),
            )
        })
        .unwrap();
    }
}

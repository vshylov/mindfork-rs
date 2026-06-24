//! Статус-бар: состояние сервера, индикатор генерации, счётчик токенов
//! ответа/контекста, подсказки по клавишам. См. spec §11.1. Модель/профиль —
//! позже.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::shared::server::ServerStatus;
use crate::shared::theme::Palette;
use crate::shared::wrap;

/// Постоянные хоткеи статус-бара (после тумблера мыши `Ctrl+W`, чьё описание
/// зависит от режима). «Клавиша» + описание; опасных нет.
const HOTKEYS: [(&str, &str); 5] = [
    ("F1", "справка"),
    ("Esc", "чаты"),
    ("Ctrl+N", "новый"),
    ("Ctrl+P", "настройки"),
    ("Ctrl+C", "выход"),
];

/// Зазор после хоткея, когда вся строка влезает целиком (как было).
const INLINE_GAP: &str = "   ";

/// Рисует статус-строку в `area`. `tokens` — токенов ответа (live), `context` —
/// токенов переписки (промпта; `None` — неизвестно), `context_exact` — точное ли
/// это число из `usage` сервера (иначе оценка, помечается `~`). `mouse_scroll` —
/// включён ли захват мыши для прокрутки колесом (иначе нативное выделение текста).
/// Когда хоткеи не помещаются по ширине, они переносятся на следующие строки
/// аккуратной сеткой (как в оверлее списка чатов); высоту под это отводит вызывающий
/// через [`height`]. См. spec §11.1, §11.3.
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
    let lines = lines(
        area.width as usize,
        status,
        generating,
        tokens,
        context,
        context_exact,
        mouse_scroll,
        palette,
    );
    frame.render_widget(Paragraph::new(lines), area);
}

/// Сколько строк займёт статус-бар при ширине `width` — вызывающий отводит под него
/// ровно эту высоту (минимум 1). Хоткеи переносятся, когда не помещаются.
#[allow(clippy::too_many_arguments)]
pub fn height(
    width: usize,
    status: &ServerStatus,
    generating: bool,
    tokens: u64,
    context: Option<u64>,
    context_exact: bool,
    mouse_scroll: bool,
    palette: &Palette,
) -> u16 {
    lines(
        width,
        status,
        generating,
        tokens,
        context,
        context_exact,
        mouse_scroll,
        palette,
    )
    .len()
    .clamp(1, u16::MAX as usize) as u16
}

/// Собирает строки статус-бара. Первая часть — «состояние» (пилюля статуса,
/// индикатор генерации, счётчик токенов). Дальше — хоткеи, начиная с тумблера мыши
/// `Ctrl+W` (его описание — текущий режим). Если состояние и все хоткеи влезают в
/// одну строку — рисуем одной строкой (как раньше); иначе «состояние» остаётся
/// первой строкой, а все хоткеи переносятся единой сеткой ниже (как в списке чатов),
/// так что все «клавиши» выровнены по столбцам.
#[allow(clippy::too_many_arguments)]
fn lines(
    width: usize,
    status: &ServerStatus,
    generating: bool,
    tokens: u64,
    context: Option<u64>,
    context_exact: bool,
    mouse_scroll: bool,
    palette: &Palette,
) -> Vec<Line<'static>> {
    let sep = || Span::styled("  │  ", Style::new().fg(palette.border));
    let muted = palette.muted_style();

    // --- «состояние» (статус сервера, генерация, счётчик токенов) ---
    let (label, color) = match status {
        ServerStatus::NotConfigured => ("сервер не настроен".to_string(), palette.warning),
        ServerStatus::Connecting => ("сервер: подключение…".to_string(), palette.warning),
        ServerStatus::Ready => ("сервер: готов".to_string(), palette.success),
        ServerStatus::Disconnected(why) => (format!("сервер: нет связи: {why}"), palette.error),
    };
    let mut state = palette.pill(&label, color);

    if generating {
        state.push(sep());
        state.push(Span::styled("⟳ генерация…", palette.accent_style()));
    }
    // Суммарный счётчик токенов (переписка + ответ): ярко во время генерации (растёт
    // live), приглушённо после (итог последнего хода). Помечается `~`, пока переписка
    // (промпт) — клиентская оценка, т.е. до прихода точного числа из `usage` сервера.
    if tokens > 0 || context.is_some() {
        state.push(sep());
        let total = context.unwrap_or(0) + tokens;
        let approx = if context.is_some() && !context_exact {
            "~"
        } else {
            ""
        };
        let label = format!("токены: {approx}{total}");
        if generating {
            state.push(Span::styled(label, palette.accent_style()));
        } else {
            state.push(Span::styled(label, muted));
        }
    }

    // Хоткеи: тумблер мыши `Ctrl+W` (описание = текущий режим) + постоянные. Все
    // «клавиши» однородны и выравниваются вместе, без отдельной orphan-клавиши.
    let mouse_desc = if mouse_scroll {
        "мышь: прокрутка"
    } else {
        "мышь: выделение"
    };
    let mut hotkeys: Vec<(&str, &str, bool)> = vec![("Ctrl+W", mouse_desc, false)];
    hotkeys.extend(HOTKEYS.iter().map(|(k, d)| (*k, *d, false)));

    // Ширина «состояния» + разделитель + все хоткеи в одну строку.
    let state_w = spans_width(&state);
    let inline_hotkeys_w: usize = hotkeys
        .iter()
        .map(|(key, desc, _)| str_w(key) + 2 + 1 + str_w(desc) + str_w(INLINE_GAP))
        .sum();
    if state_w + str_w("  │  ") + inline_hotkeys_w <= width {
        // Всё влезает — одна строка (как до переноса).
        let mut spans = state;
        spans.push(sep());
        for (key, desc, _) in &hotkeys {
            spans.extend(palette.hint(key, desc));
            spans.push(Span::raw(INLINE_GAP));
        }
        return vec![Line::from(spans)];
    }

    // Не влезает — «состояние» первой строкой, все хоткеи единой сеткой ниже.
    let mut out = vec![Line::from(state)];
    out.extend(palette.hotkey_grid(&hotkeys, width));
    out
}

/// Видимая ширина строки в колонках терминала.
fn str_w(s: &str) -> usize {
    wrap::display_width(&s.chars().collect::<Vec<_>>())
}

/// Суммарная видимая ширина набора спанов.
fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| str_w(&s.content)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Весь текст статус-бара одной строкой (широкая ширина → одна строка).
    fn flat(
        status: &ServerStatus,
        generating: bool,
        tokens: u64,
        context: Option<u64>,
        context_exact: bool,
        mouse_scroll: bool,
    ) -> String {
        lines(
            200,
            status,
            generating,
            tokens,
            context,
            context_exact,
            mouse_scroll,
            &Palette::default(),
        )
        .iter()
        .flat_map(|l| l.spans.iter())
        .map(|s| s.content.as_ref())
        .collect()
    }

    fn text(status: &ServerStatus, generating: bool) -> String {
        flat(status, generating, 0, None, false, false)
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
        let mode = |scroll| flat(&ServerStatus::Ready, false, 0, None, false, scroll);
        assert!(mode(true).contains("мышь: прокрутка"));
        assert!(mode(false).contains("мышь: выделение"));
    }

    #[test]
    fn token_counter_shows_summed_total() {
        let line = |tokens, context, exact| {
            flat(&ServerStatus::Ready, true, tokens, context, exact, false)
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
    fn hotkeys_fit_on_one_line_when_wide() {
        let n = lines(
            200,
            &ServerStatus::Ready,
            false,
            0,
            None,
            false,
            false,
            &Palette::default(),
        )
        .len();
        assert_eq!(n, 1);
    }

    #[test]
    fn hotkeys_wrap_to_grid_when_narrow() {
        // Узкая ширина → хоткеи не помещаются и переносятся на следующие строки.
        let h = height(
            40,
            &ServerStatus::Ready,
            false,
            0,
            None,
            false,
            false,
            &Palette::default(),
        );
        assert!(h > 1, "ожидался перенос хоткеев, высота = {h}");
        // Все хоткеи присутствуют, несмотря на перенос (включая тумблер мыши).
        let flat = flat(&ServerStatus::Ready, false, 0, None, false, false);
        assert!(flat.contains("Ctrl+W") && flat.contains("мышь: выделение"));
        for (_, desc) in HOTKEYS {
            assert!(flat.contains(desc));
        }
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

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

/// Зазор между столбцами хоткеев и между пилюлей статуса и сеткой хоткеев.
const GAP: usize = 3;

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

/// Собирает строки статус-бара. Слева на **верхней** строке — «состояние» (пилюля
/// статуса, индикатор генерации, счётчик токенов), прижатое к левому краю. Справа —
/// хоткеи (начиная с тумблера мыши `Ctrl+W`, чьё описание = текущий режим), выложенные
/// аккуратной сеткой и **прижатые к правому краю**: когда всё влезает — одна строка
/// (пилюля слева, хоткеи справа); когда нет — хоткеи переносятся ВНИЗ сеткой, столбцы
/// которой совпадают по вертикали, а неполная (перенесённая) строка прижата вправо —
/// её клавиши встают ровно под столбцами строки выше, не привлекая внимание к
/// левой/средней части окна. Пилюля статуса делит верхнюю строку с сеткой. См.
/// spec §11.1, §11.3.
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
    let state_w = spans_width(&state);

    // Хоткеи: тумблер мыши `Ctrl+W` (описание = текущий режим) + постоянные.
    let mouse_desc = if mouse_scroll {
        "мышь: прокрутка"
    } else {
        "мышь: выделение"
    };
    let mut hotkeys: Vec<(&str, &str)> = vec![("Ctrl+W", mouse_desc)];
    hotkeys.extend(HOTKEYS.iter().copied());
    let n = hotkeys.len();
    // В режиме «прокрутка» выделяем описание тумблера мыши (индекс 0) цветом
    // `accent` — тем же, которым подсвечиваются заголовки markdown в ленте.
    let accent_idx = mouse_scroll.then_some(0usize);

    // Ширина каждой ячейки: «клавиша» (+2 на отступы) + пробел + описание.
    let cell_w: Vec<usize> = hotkeys
        .iter()
        .map(|(key, desc)| str_w(key) + 2 + 1 + str_w(desc))
        .collect();

    // Подбираем максимум столбцов (→ минимум строк), при которых пилюля статуса
    // делит верхнюю строку с прижатой вправо сеткой (`state + GAP + блок ≤ ширины`).
    let mut cols = 0;
    for c in (1..=n).rev() {
        if state_w + GAP + grid_layout(&cell_w, c).2 <= width {
            cols = c;
            break;
        }
    }
    if cols > 0 {
        return right_grid(
            &hotkeys,
            &cell_w,
            cols,
            width,
            Some(state),
            accent_idx,
            palette,
        );
    }

    // Слишком узко даже под один столбец рядом с пилюлей — «состояние» отдельной
    // верхней строкой, а хоткеи прижатой вправо сеткой под ним (без наложения).
    cols = (1..=n)
        .rev()
        .find(|&c| grid_layout(&cell_w, c).2 <= width)
        .unwrap_or(1);
    let mut out = vec![Line::from(state)];
    out.extend(right_grid(
        &hotkeys, &cell_w, cols, width, None, accent_idx, palette,
    ));
    out
}

/// Раскладка хоткеев по сетке `cols` столбцов: ячейки заполняются по строкам
/// слева-направо/сверху-вниз, но **неполная нижняя строка прижата вправо** (её ячейки
/// занимают крайние правые столбцы, под полными строками). Возвращает карту
/// `grid[row][col] = Some(индекс ячейки)`, ширины столбцов (максимум по ячейкам столбца)
/// и общую ширину блока (сумма столбцов + зазоры). Так перенесённые клавиши встают
/// ровно под столбцами строки выше (как в окне списка чатов), а не «плавающей» группой.
fn grid_layout(cell_w: &[usize], cols: usize) -> (Vec<Vec<Option<usize>>>, Vec<usize>, usize) {
    let n = cell_w.len();
    let rows = n.div_ceil(cols);
    let full = (rows - 1) * cols; // ячеек в полных строках
    let empty_lead = cols - (n - full); // пустых столбцов в начале нижней строки

    let mut grid = vec![vec![None; cols]; rows];
    for i in 0..n {
        let (r, c) = if i < full {
            (i / cols, i % cols)
        } else {
            (rows - 1, empty_lead + (i - full))
        };
        grid[r][c] = Some(i);
    }

    let mut colw = vec![0usize; cols];
    for row in &grid {
        for (c, cell) in row.iter().enumerate() {
            if let Some(i) = cell {
                colw[c] = colw[c].max(cell_w[*i]);
            }
        }
    }
    let block_w = colw.iter().sum::<usize>() + GAP * cols.saturating_sub(1);
    (grid, colw, block_w)
}

/// Рисует хоткеи сеткой, прижатой к правому краю `width` (столбцы выровнены по
/// вертикали; неполная нижняя строка — под крайними правыми столбцами). Если задан
/// `state`, пилюля статуса вставляется в левый край **верхней** строки.
fn right_grid(
    hotkeys: &[(&str, &str)],
    cell_w: &[usize],
    cols: usize,
    width: usize,
    mut state: Option<Vec<Span<'static>>>,
    accent_idx: Option<usize>,
    palette: &Palette,
) -> Vec<Line<'static>> {
    let (grid, colw, block_w) = grid_layout(cell_w, cols);
    let lead = width.saturating_sub(block_w);

    let mut out: Vec<Line<'static>> = Vec::new();
    for (r, row) in grid.iter().enumerate() {
        let mut spans: Vec<Span<'static>> = Vec::new();
        // Левое поле: на верхней строке — пилюля статуса, остаток (и прочие строки) —
        // пробелы (сетка прижата вправо).
        if r == 0
            && let Some(state) = state.take()
        {
            let state_w = spans_width(&state);
            spans.extend(state);
            if lead > state_w {
                spans.push(Span::raw(" ".repeat(lead - state_w)));
            }
        } else if lead > 0 {
            spans.push(Span::raw(" ".repeat(lead)));
        }
        // Столбцы: ячейка добивается до ширины столбца (+ зазор, кроме последнего),
        // пустой столбец — целиком пробелами (так столбцы совпадают по вертикали).
        for (c, cell) in row.iter().enumerate() {
            let gap = if c + 1 < cols { GAP } else { 0 };
            match cell {
                Some(i) => {
                    let (key, desc) = hotkeys[*i];
                    if accent_idx == Some(*i) {
                        spans.extend(palette.hint_highlight_word(key, desc, palette.accent));
                    } else {
                        spans.extend(palette.hint(key, desc));
                    }
                    let pad = colw[c].saturating_sub(cell_w[*i]) + gap;
                    if pad > 0 {
                        spans.push(Span::raw(" ".repeat(pad)));
                    }
                }
                None => spans.push(Span::raw(" ".repeat(colw[c] + gap))),
            }
        }
        out.push(Line::from(spans));
    }
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
    fn scroll_mode_highlights_only_word() {
        let palette = Palette::default();
        let span_fg = |scroll, needle: &str| {
            lines(
                200,
                &ServerStatus::Ready,
                false,
                0,
                None,
                false,
                scroll,
                &palette,
            )
            .iter()
            .flat_map(|l| l.spans.clone())
            .find(|s| s.content.contains(needle))
            .and_then(|s| s.style.fg)
        };
        // В режиме прокрутки выделено только слово «прокрутка» (цветом `accent`, как
        // заголовки markdown), а «мышь:» остаётся приглушённым — это отдельные спаны.
        assert_eq!(span_fg(true, "прокрутка"), Some(palette.accent));
        assert_eq!(span_fg(true, "мышь:"), Some(palette.muted));
        // В режиме выделения — всё описание приглушённое.
        assert_eq!(span_fg(false, "мышь: выделение"), Some(palette.muted));
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

    /// Рендерит статус-бар на ширине `w` и возвращает строки буфера как текст.
    fn rows(w: u16) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let p = Palette::default();
        let h = height(
            w as usize,
            &ServerStatus::Ready,
            false,
            0,
            None,
            false,
            false,
            &p,
        );
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            render(
                f,
                f.area(),
                &ServerStatus::Ready,
                false,
                0,
                None,
                false,
                false,
                &p,
            )
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn pill_left_hotkeys_right_on_one_line() {
        // Широко — одна строка: пилюля прижата влево, хоткеи — вправо.
        let r = rows(160);
        assert_eq!(r.len(), 1);
        let line = &r[0];
        assert!(line.trim_start().starts_with("●"), "пилюля слева: {line:?}");
        assert!(
            line.trim_end().ends_with("выход"),
            "последний хоткей прижат вправо: {line:?}"
        );
        // Между пилюлей и хоткеями — заметный зазор (они не слиплись).
        assert!(
            line.contains("готов   "),
            "есть зазор после пилюли: {line:?}"
        );
    }

    #[test]
    fn wrapped_grid_is_right_aligned_with_pill_on_top_line() {
        // Узко — хоткеи переносятся аккуратной сеткой, прижатой вправо; пилюля
        // статуса делит ВЕРХНЮЮ строку с сеткой, перенос — ровно под столбцом выше.
        let r = rows(120);
        assert_eq!(r.len(), 2, "ожидался перенос на 2 строки: {r:?}");
        // Пилюля статуса — на верхней строке слева.
        assert!(
            r[0].trim_start().starts_with("●"),
            "пилюля сверху слева: {:?}",
            r[0]
        );
        // Перенесённый хоткей — на нижней строке, прижат вправо (левая часть пустая).
        assert!(
            r[1].trim_start().starts_with("Ctrl+C") && r[1].starts_with(" "),
            "перенос прижат вправо, левая часть пустая: {:?}",
            r[1]
        );
        // Колонки совпадают по вертикали: `Ctrl+C` (низ) ровно под `Ctrl+P` (верх).
        // Сравниваем позицию в СИМВОЛАХ (кириллица многобайтна → байтовый offset не
        // равен колонке; здесь все символы шириной 1, так что символ == колонка).
        let char_col = |line: &str, pat: &str| line.find(pat).map(|b| line[..b].chars().count());
        assert_eq!(
            char_col(&r[1], "Ctrl+C"),
            char_col(&r[0], "Ctrl+P"),
            "перенос ровно под столбцом выше:\n{:?}\n{:?}",
            r[0],
            r[1]
        );
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

//! Статус-бар: состояние сервера, индикатор генерации, счётчик токенов
//! ответа/контекста, подсказки по клавишам. См. spec §11.1. Модель/профиль —
//! позже.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::shared::i18n::Locale;
use crate::shared::server::{ServerStatus, ServerStatuses};
use crate::shared::theme::Palette;
use crate::shared::wrap;

/// Ключи постоянных хоткеев статус-бара (после тумблера мыши `Ctrl+W`, чьё описание
/// зависит от режима). «Клавиша» + ключ описания (локализуется в [`lines`]).
const HOTKEYS: [(&str, &str); 5] = [
    ("F1", "ui.status.hotkey.help"),
    ("Esc", "ui.status.hotkey.chats"),
    ("Ctrl+N", "ui.status.hotkey.new"),
    ("Ctrl+P", "ui.status.hotkey.settings"),
    ("Ctrl+Q", "ui.status.hotkey.quit"),
];

/// Зазор между столбцами хоткеев и между пилюлей статуса и сеткой хоткеев.
const GAP: usize = 3;

/// Снимок состояния для строки статуса — экран собирает его в одном месте
/// ([`ChatScreen::status_model`](crate::screens::chat::ChatScreen)), поэтому новый
/// индикатор добавляет поле, а не расширяет сигнатуры `render`/`height`.
/// `tokens` — токенов ответа (live), `context` — токенов переписки (промпта; `None`
/// — неизвестно), `context_exact` — точное ли это число из `usage` сервера (иначе
/// оценка, помечается `~`). `mouse_scroll` — включён ли захват мыши для прокрутки
/// колесом (иначе нативное выделение текста). `background` — тихий индикатор фоновой
/// задачи (авто-рефлексия/консолидация; `None` — нет). См. spec §11.1, §11.3.
pub struct StatusModel<'a> {
    pub statuses: &'a ServerStatuses,
    pub generating: bool,
    pub tokens: u64,
    pub context: Option<u64>,
    pub context_exact: bool,
    /// Reasoning-токенов («мыслей») в ответе (входят в `tokens`); `0` — не показывать.
    pub reasoning: u32,
    pub mouse_scroll: bool,
    pub background: Option<&'a str>,
    /// Идёт озвучивание (`/tts`) — тихий чип «озвучка». См. spec §11.9.
    pub speaking: bool,
}

/// Глиф индикатора озвучивания (WGL4, ширина 1 колонка — сетка хоткеев не «съезжает»).
const SPEAKING_GLYPH: char = '♪';

/// Рисует статус-строку в `area`. Когда хоткеи не помещаются по ширине, они
/// переносятся на следующие строки аккуратной сеткой (как в оверлее списка чатов);
/// высоту под это отводит вызывающий через [`height`]. См. spec §11.1, §11.3.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    model: &StatusModel,
    palette: &Palette,
    loc: &'static Locale,
) {
    let lines = lines(area.width as usize, model, palette, loc);
    frame.render_widget(Paragraph::new(lines), area);
}

/// Сколько строк займёт статус-бар при ширине `width` — вызывающий отводит под него
/// ровно эту высоту (минимум 1). Хоткеи переносятся, когда не помещаются.
pub fn height(width: usize, model: &StatusModel, palette: &Palette, loc: &'static Locale) -> u16 {
    lines(width, model, palette, loc)
        .len()
        .clamp(1, u16::MAX as usize) as u16
}

/// Раскладывает хоткеи в аккуратную сетку, **прижатую к правому краю** `width` — та
/// же логика переноса, что и в статус-баре экрана чата, но без пилюли статуса
/// (`state = None`). Число столбцов — максимум влезающих в ширину (→ минимум строк);
/// при переполнении хоткеи переносятся вниз, столбцы совпадают по вертикали, а
/// неполная нижняя строка прижата вправо под столбцами выше. Возвращает пусто на
/// пустом наборе. Используется экраном настроек (см. spec §11.1, §11.6).
pub fn hotkey_lines(
    width: usize,
    hotkeys: &[(&str, &str)],
    palette: &Palette,
) -> Vec<Line<'static>> {
    if hotkeys.is_empty() {
        return Vec::new();
    }
    // Ширина ячейки как в `lines`: «клавиша» (+2 на отступы) + пробел + описание.
    let cell_w: Vec<usize> = hotkeys
        .iter()
        .map(|(key, desc)| str_w(key) + 2 + 1 + str_w(desc))
        .collect();
    let n = hotkeys.len();
    let cols = (1..=n)
        .rev()
        .find(|&c| grid_layout(&cell_w, c).2 <= width)
        .unwrap_or(1);
    right_grid(hotkeys, &cell_w, cols, width, None, None, palette)
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
fn lines(
    width: usize,
    model: &StatusModel,
    palette: &Palette,
    loc: &'static Locale,
) -> Vec<Line<'static>> {
    // Поля Copy (ссылки/скаляры) — разворачиваем в локальные, тело ниже не меняется.
    let statuses = model.statuses;
    let generating = model.generating;
    let tokens = model.tokens;
    let context = model.context;
    let context_exact = model.context_exact;
    let reasoning = model.reasoning;
    let mouse_scroll = model.mouse_scroll;
    let background = model.background;
    let speaking = model.speaking;
    let sep = || Span::styled("  │  ", Style::new().fg(palette.border));
    let muted = palette.muted_style();

    // --- «состояние»: чипы серверов + генерация + счётчик токенов ---
    // Чат-сервер показывается всегда (с причиной обрыва — он блокирует генерацию);
    // эмбеддинги и имперсонация — отдельными чипами и только когда настроены
    // (`NotConfigured`, в т.ч. имперсонация в `shared`, → чип скрыт).
    let mut state = chat_chip(&statuses.chat, palette, loc);
    for chip in [
        secondary_chip(loc.t("ui.status.chip.embed"), &statuses.embed, palette),
        secondary_chip(
            loc.t("ui.status.chip.imp"),
            &statuses.impersonation,
            palette,
        ),
    ]
    .into_iter()
    .flatten()
    {
        state.push(Span::raw("  ")); // зазор между чипами
        state.extend(chip);
    }

    if generating {
        state.push(sep());
        state.push(Span::styled(
            format!(
                "{} {}",
                palette.glyphs().busy,
                loc.t("ui.status.generating")
            ),
            palette.accent_style(),
        ));
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
        // Reasoning-токены («мысли») — отдельной пометкой, они входят в общий счёт.
        let reason = if reasoning > 0 {
            loc.tf("ui.status.reasoning", &[("n", &reasoning.to_string())])
        } else {
            String::new()
        };
        let label = loc.tf(
            "ui.status.tokens",
            &[
                ("approx", approx),
                ("total", &total.to_string()),
                ("reason", &reason),
            ],
        );
        if generating {
            state.push(Span::styled(label, palette.accent_style()));
        } else {
            state.push(Span::styled(label, muted));
        }
    }
    // Тихий индикатор фоновой задачи (авто-рефлексия/консолидация) — приглушённо,
    // глиф `✻` (компат — `*`) шириной 1 колонка (раскладка сетки не «съезжает»).
    if let Some(hint) = background {
        state.push(sep());
        state.push(Span::styled(
            format!("{} {hint}", palette.glyphs().background),
            muted,
        ));
    }
    // Тихий индикатор озвучивания — тем же приглушённым стилем. Нота `♪` (U+266A)
    // входит в WGL4 и шириной 1 колонка, поэтому в компат-режиме не заменяется
    // (как `▌`-рейлы ленты и `█` скроллбара). См. spec §11.9.
    if speaking {
        state.push(sep());
        state.push(Span::styled(
            format!("{SPEAKING_GLYPH} {}", loc.t("ui.status.speaking")),
            muted,
        ));
    }
    let state_w = spans_width(&state);

    // Хоткеи: тумблер мыши `Ctrl+W` (описание = текущий режим) + постоянные.
    let mouse_desc = if mouse_scroll {
        loc.t("ui.status.mouse.scroll")
    } else {
        loc.t("ui.status.mouse.select")
    };
    let mut hotkeys: Vec<(&str, &str)> = vec![("Ctrl+W", mouse_desc)];
    hotkeys.extend(HOTKEYS.iter().map(|(key, k)| (*key, loc.t(k))));
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

/// Глиф и цвет статуса сервера для чипа строки статуса. Глифы шириной 1 колонка
/// (без эмодзи) — раскладка строки от них не «съезжает». Готовность — `●` в обоих
/// наборах (WGL4-безопасен); «подключение»/«нет связи» в компат-режиме — `○`/`×`.
fn status_glyph(status: &ServerStatus, palette: &Palette) -> (&'static str, Color) {
    let glyphs = palette.glyphs();
    match status {
        ServerStatus::Ready => ("●", palette.success),
        ServerStatus::Connecting => (glyphs.status_connecting, palette.warning),
        ServerStatus::NotConfigured => (glyphs.status_off, palette.warning),
        ServerStatus::Disconnected(_) => (glyphs.status_off, palette.error),
    }
}

/// Собирает чип: жирный глиф + метка (с ведущим пробелом), оба цвета статуса.
fn chip(glyph: &'static str, label: String, color: Color) -> Vec<Span<'static>> {
    vec![
        Span::styled(glyph, Style::new().fg(color).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {label}"), Style::new().fg(color)),
    ]
}

/// Чип чат-сервера — показывается всегда. Помимо глифа и метки несёт текст причины,
/// когда сервер недоступен/не настроен: он блокирует генерацию, и пользователю
/// нужно знать, почему (у вторичных серверов причина опущена ради компактности).
fn chat_chip(status: &ServerStatus, palette: &Palette, loc: &'static Locale) -> Vec<Span<'static>> {
    let (glyph, color) = status_glyph(status, palette);
    let label = match status {
        ServerStatus::Ready | ServerStatus::Connecting => loc.t("ui.status.chip.chat").to_string(),
        ServerStatus::NotConfigured => loc.t("ui.status.chip.chat_off").to_string(),
        ServerStatus::Disconnected(why) => loc.tf("ui.status.chip.chat_down", &[("why", why)]),
    };
    chip(glyph, label, color)
}

/// Чип вторичного сервера (эмбеддинги/имперсонация): глиф + метка, цвет по статусу,
/// без текста причины (компактно). `None`, когда сервер не настроен
/// (`NotConfigured`) — чип скрыт, не захламляет строку (имперсонация в режиме
/// `shared` тоже `NotConfigured`).
fn secondary_chip(
    label: &str,
    status: &ServerStatus,
    palette: &Palette,
) -> Option<Vec<Span<'static>>> {
    if matches!(status, ServerStatus::NotConfigured) {
        return None;
    }
    let (glyph, color) = status_glyph(status, palette);
    Some(chip(glyph, label.to_string(), color))
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
                        spans.extend(palette.hint_highlight_value(key, desc, palette.accent));
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

    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    /// Снимок статусов: чат `chat`, эмбеддинги/имперсонация не настроены (чипы скрыты).
    fn only_chat(chat: ServerStatus) -> ServerStatuses {
        ServerStatuses {
            chat,
            embed: ServerStatus::NotConfigured,
            impersonation: ServerStatus::NotConfigured,
        }
    }

    /// Снимок с готовым чат-сервером (вторичные не настроены).
    fn ready() -> ServerStatuses {
        only_chat(ServerStatus::Ready)
    }

    /// Снимок статуса для тестов (background = `None`).
    fn model<'a>(
        statuses: &'a ServerStatuses,
        generating: bool,
        tokens: u64,
        context: Option<u64>,
        context_exact: bool,
        mouse_scroll: bool,
    ) -> StatusModel<'a> {
        StatusModel {
            statuses,
            generating,
            tokens,
            context,
            context_exact,
            reasoning: 0,
            mouse_scroll,
            background: None,
            speaking: false,
        }
    }

    /// Весь текст статус-бара одной строкой (широкая ширина → одна строка).
    fn flat(
        statuses: &ServerStatuses,
        generating: bool,
        tokens: u64,
        context: Option<u64>,
        context_exact: bool,
        mouse_scroll: bool,
    ) -> String {
        let m = model(
            statuses,
            generating,
            tokens,
            context,
            context_exact,
            mouse_scroll,
        );
        lines(200, &m, &Palette::default(), ru())
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect()
    }

    fn text(statuses: &ServerStatuses, generating: bool) -> String {
        flat(statuses, generating, 0, None, false, false)
    }

    #[test]
    fn shows_chat_state() {
        assert!(text(&ready(), false).contains("чат"));
        assert!(text(&only_chat(ServerStatus::NotConfigured), false).contains("не настроен"));
        assert!(
            text(&only_chat(ServerStatus::Disconnected("boom".into())), false).contains("boom")
        );
    }

    #[test]
    fn localized_for_all_langs() {
        // Статус-бар (ось B) под каждым языком: чип чата и хоткей выхода — из бандла
        // того же языка; en — латиница. Проверяем и отсутствие паники на разной
        // ширине текста перевода.
        let statuses = ready();
        let m = model(&statuses, false, 0, None, false, false);
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            let flat: String = lines(200, &m, &Palette::default(), loc)
                .iter()
                .flat_map(|l| l.spans.iter())
                .map(|s| s.content.as_ref())
                .collect();
            assert!(
                flat.contains(loc.t("ui.status.chip.chat")),
                "{lang:?}: {flat}"
            );
            assert!(
                flat.contains(loc.t("ui.status.hotkey.quit")),
                "{lang:?}: {flat}"
            );
        }
        let en = crate::shared::i18n::locale(crate::shared::i18n::Lang::En);
        assert_eq!(en.t("ui.status.chip.chat"), "chat");
    }

    #[test]
    fn secondary_chips_shown_only_when_configured() {
        // Эмбеддинги и имперсонация настроены — оба чипа видны.
        let all = ServerStatuses {
            chat: ServerStatus::Ready,
            embed: ServerStatus::Ready,
            impersonation: ServerStatus::Connecting,
        };
        let t = text(&all, false);
        assert!(
            t.contains("чат") && t.contains("эмб") && t.contains("имп"),
            "{t}"
        );
        // Имперсонация в `shared` / эмбеддинги выкл (NotConfigured) — чипы скрыты.
        let t = text(&ready(), false);
        assert!(
            t.contains("чат") && !t.contains("эмб") && !t.contains("имп"),
            "{t}"
        );
    }

    #[test]
    fn compat_palette_swaps_chip_and_activity_glyphs() {
        // В режиме совместимости: «подключение» — `○`, обрыв — `×`, генерация — `»`,
        // фоновая задача — `*`; готовность остаётся `●` (WGL4-безопасен).
        let compat = Palette::default().with_compat(true);
        let flat_compat = |statuses: &ServerStatuses, generating: bool| -> String {
            let m = StatusModel {
                statuses,
                generating,
                tokens: 0,
                context: None,
                context_exact: false,
                reasoning: 0,
                mouse_scroll: false,
                background: Some("рефлексия"),
                speaking: false,
            };
            lines(200, &m, &compat, ru())
                .iter()
                .flat_map(|l| l.spans.iter())
                .map(|s| s.content.as_ref())
                .collect()
        };
        let t = flat_compat(&only_chat(ServerStatus::Connecting), true);
        assert!(t.contains("○ чат"), "компат-глиф подключения: {t}");
        assert!(t.contains("» генерация"), "компат-глиф генерации: {t}");
        assert!(t.contains("* рефлексия"), "компат-глиф фоновой задачи: {t}");
        for banned in ['◐', '⟳', '✻'] {
            assert!(!t.contains(banned), "остался {banned}: {t}");
        }
        let t = flat_compat(&only_chat(ServerStatus::Disconnected("boom".into())), false);
        assert!(t.contains("× чат"), "компат-глиф обрыва: {t}");
        let t = flat_compat(&ready(), false);
        assert!(t.contains("● чат"), "готовность остаётся ●: {t}");
    }

    #[test]
    fn chip_color_reflects_status() {
        let palette = Palette::default();
        // Цвет глифа (первый спан) по статусу: готов — success, обрыв — error.
        let glyph_fg = |status: ServerStatus| {
            chat_chip(&status, &palette, ru())
                .first()
                .and_then(|s| s.style.fg)
                .unwrap()
        };
        assert_eq!(glyph_fg(ServerStatus::Ready), palette.success);
        assert_eq!(
            glyph_fg(ServerStatus::Disconnected("x".into())),
            palette.error
        );
        assert_eq!(glyph_fg(ServerStatus::Connecting), palette.warning);
    }

    #[test]
    fn generating_indicator_toggles() {
        assert!(text(&ready(), true).contains("генерация"));
        assert!(!text(&ready(), false).contains("генерация"));
    }

    #[test]
    fn shows_mouse_mode() {
        let mode = |scroll| flat(&ready(), false, 0, None, false, scroll);
        assert!(mode(true).contains("мышь: прокрутка"));
        assert!(mode(false).contains("мышь: выделение"));
    }

    #[test]
    fn scroll_mode_highlights_only_value() {
        let palette = Palette::default();
        let span_fg = |scroll, needle: &str| {
            let statuses = ready();
            let m = model(&statuses, false, 0, None, false, scroll);
            lines(200, &m, &palette, ru())
                .iter()
                .flat_map(|l| l.spans.clone())
                .find(|s| s.content.contains(needle))
                .and_then(|s| s.style.fg)
        };
        // В режиме прокрутки выделено только значение «прокрутка» (цветом `accent`, как
        // заголовки markdown), а подпись «мышь:» остаётся приглушённой — отдельные спаны.
        assert_eq!(span_fg(true, "прокрутка"), Some(palette.accent));
        assert_eq!(span_fg(true, "мышь:"), Some(palette.muted));
        // В режиме выделения — всё описание приглушённое.
        assert_eq!(span_fg(false, "мышь: выделение"), Some(palette.muted));
    }

    #[test]
    fn token_counter_shows_summed_total() {
        let line = |tokens, context, exact| flat(&ready(), true, tokens, context, exact, false);
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
    fn token_counter_shows_reasoning_when_present() {
        let statuses = ready();
        let with_reason = |reasoning: u32| -> String {
            let m = StatusModel {
                statuses: &statuses,
                generating: true,
                tokens: 42,
                context: Some(1000),
                context_exact: true,
                reasoning,
                mouse_scroll: false,
                background: None,
                speaking: false,
            };
            lines(200, &m, &Palette::default(), ru())
                .iter()
                .flat_map(|l| l.spans.iter())
                .map(|s| s.content.as_ref())
                .collect()
        };
        // Reasoning-токены показываются отдельной пометкой (входят в общий счёт).
        assert!(with_reason(300).contains("токены: 1042 (рассужд. 300)"));
        // Ноль reasoning-токенов — пометки нет.
        let none = with_reason(0);
        assert!(none.contains("токены: 1042"));
        assert!(!none.contains("рассужд"));
    }

    #[test]
    fn hotkeys_fit_on_one_line_when_wide() {
        let statuses = ready();
        let m = model(&statuses, false, 0, None, false, false);
        let n = lines(200, &m, &Palette::default(), ru()).len();
        assert_eq!(n, 1);
    }

    #[test]
    fn hotkeys_wrap_to_grid_when_narrow() {
        // Узкая ширина → хоткеи не помещаются и переносятся на следующие строки.
        let statuses = ready();
        let m = model(&statuses, false, 0, None, false, false);
        let h = height(40, &m, &Palette::default(), ru());
        assert!(h > 1, "ожидался перенос хоткеев, высота = {h}");
        // Все хоткеи присутствуют, несмотря на перенос (включая тумблер мыши).
        let flat = flat(&ready(), false, 0, None, false, false);
        assert!(flat.contains("Ctrl+W") && flat.contains("мышь: выделение"));
        for (_, desc) in HOTKEYS {
            assert!(flat.contains(ru().t(desc)));
        }
    }

    /// Рендерит статус-бар на ширине `w` и возвращает строки буфера как текст.
    fn rows(w: u16) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let p = Palette::default();
        let statuses = ready();
        let m = model(&statuses, false, 0, None, false, false);
        let h = height(w as usize, &m, &p, ru());
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| render(f, f.area(), &m, &p, ru())).unwrap();
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
    fn chips_left_hotkeys_right_on_one_line() {
        // Широко — одна строка: чип статуса прижат влево, хоткеи — вправо.
        let r = rows(160);
        assert_eq!(r.len(), 1);
        let line = &r[0];
        assert!(line.trim_start().starts_with("●"), "чип слева: {line:?}");
        assert!(
            line.trim_end().ends_with("выход"),
            "последний хоткей прижат вправо: {line:?}"
        );
        // Между чипом и хоткеями — заметный зазор (они не слиплись).
        assert!(line.contains("чат   "), "есть зазор после чипа: {line:?}");
    }

    #[test]
    fn wrapped_grid_is_right_aligned_with_pill_on_top_line() {
        // Узко — хоткеи переносятся аккуратной сеткой, прижатой вправо; чип статуса
        // делит ВЕРХНЮЮ строку с сеткой, перенос — ровно под столбцом выше.
        let r = rows(100);
        assert_eq!(r.len(), 2, "ожидался перенос на 2 строки: {r:?}");
        // Чип статуса — на верхней строке слева.
        assert!(
            r[0].trim_start().starts_with("●"),
            "чип сверху слева: {:?}",
            r[0]
        );
        // Перенесённый хоткей — на нижней строке, прижат вправо (левая часть пустая).
        assert!(
            r[1].trim_start().starts_with("Ctrl+Q") && r[1].starts_with(" "),
            "перенос прижат вправо, левая часть пустая: {:?}",
            r[1]
        );
        // Колонки совпадают по вертикали: `Ctrl+Q` (низ) ровно под `Ctrl+P` (верх).
        // Сравниваем позицию в СИМВОЛАХ (кириллица многобайтна → байтовый offset не
        // равен колонке; здесь все символы шириной 1, так что символ == колонка).
        let char_col = |line: &str, pat: &str| line.find(pat).map(|b| line[..b].chars().count());
        assert_eq!(
            char_col(&r[1], "Ctrl+Q"),
            char_col(&r[0], "Ctrl+P"),
            "перенос ровно под столбцом выше:\n{:?}\n{:?}",
            r[0],
            r[1]
        );
    }

    #[test]
    fn hotkey_lines_wraps_and_right_aligns() {
        let p = Palette::default();
        let items: &[(&str, &str)] = &[
            ("Tab", "секция"),
            ("↑↓", "поля"),
            ("Enter", "правка"),
            ("Esc", "назад"),
            ("Ctrl+C", "выход"),
        ];
        // Широко — одна строка, прижата вправо (заканчивается описанием последней клавиши).
        let wide = hotkey_lines(200, items, &p);
        assert_eq!(wide.len(), 1);
        let flat: String = wide[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(flat.contains("Tab") && flat.trim_end().ends_with("выход"));
        // Узко — переносится на несколько строк.
        assert!(hotkey_lines(24, items, &p).len() > 1);
        // Пустой набор — без строк.
        assert!(hotkey_lines(80, &[], &p).is_empty());
    }

    #[test]
    fn render_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut term = Terminal::new(TestBackend::new(60, 1)).unwrap();
        let all = ServerStatuses {
            chat: ServerStatus::Ready,
            embed: ServerStatus::Ready,
            impersonation: ServerStatus::Connecting,
        };
        let m = StatusModel {
            statuses: &all,
            generating: true,
            tokens: 123,
            context: Some(456),
            context_exact: true,
            reasoning: 0,
            mouse_scroll: true,
            background: Some("рефлексия"),
            speaking: true,
        };
        term.draw(|f| render(f, f.area(), &m, &Palette::default(), ru()))
            .unwrap();
    }
}

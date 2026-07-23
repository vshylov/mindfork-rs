//! Экран чата — попапы: подсказки орфографии, подтверждение, эмодзи, справка. Часть модуля [`super`]; разбито из
//! монолита chat.rs (см. docs/history/refactoring-god-objects.md, этап 2).

use ratatui::style::Color;

use super::render::centered_rect;
use super::*;
use crate::shared::credits;
use crate::widgets::logo::{LOCKUP_COLS, LOCKUP_ROWS, lockup_lines};

/// Отступ лockup'а слева — тот же, с которого начинается список клавиш.
const LOGO_INDENT: u16 = 2;

impl ChatScreen {
    /// Открывает попап подсказок для слова с ошибкой под курсором (если есть).
    pub(super) fn open_suggestions(&mut self) {
        let Some(spell) = &self.spell else { return };
        let (row, col) = self.input.cursor();
        let lines = self.input.line_strings();
        let Some(line) = lines.get(row) else { return };
        let Some(word) = spell.misspelled_word_at(line, col) else {
            return;
        };
        let mut items: Vec<SuggestItem> = spell
            .suggest(&word.text)
            .into_iter()
            .map(SuggestItem::Replace)
            .collect();
        items.push(SuggestItem::AddToDictionary);
        self.suggest = Some(SuggestPopup {
            word: word.text,
            row,
            start: word.start,
            end: word.end,
            items,
            selected: 0,
        });
    }

    /// Обрабатывает клавишу модального попапа подтверждения: `Enter` подтверждает
    /// (отдаёт намерение), `Esc` отменяет, прочие клавиши игнорируются. Подтверждение
    /// гасится при попадании в генерацию (намерение всё равно гейтит оркестратор).
    pub(super) fn handle_confirm_key(&mut self, key: KeyEvent) -> Option<ChatIntent> {
        let action = self.confirm?;
        // Ctrl+Q/F10 пробивают попап на выход (раскладко-независимо). См. spec §11.7.
        if key.code == KeyCode::F(10)
            || (key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char(c) if keys::physical_char(c) == 'q'))
        {
            self.confirm = None;
            return Some(ChatIntent::Quit);
        }
        match key.code {
            KeyCode::Enter => {
                self.confirm = None;
                (!self.generating).then(|| action.intent())
            }
            KeyCode::Esc => {
                self.confirm = None;
                None
            }
            _ => None,
        }
    }

    pub(super) fn handle_suggest_key(&mut self, key: KeyEvent) {
        let Some(popup) = &mut self.suggest else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.suggest = None,
            KeyCode::Up => {
                popup.selected = popup.selected.saturating_sub(1);
                return;
            }
            KeyCode::Down => {
                popup.selected = (popup.selected + 1).min(popup.items.len().saturating_sub(1));
                return;
            }
            KeyCode::Enter => self.apply_suggestion(),
            // Прочие клавиши попап не меняют (и не закрывают) — перерисовка не нужна.
            _ => return,
        }
        // Попап закрылся, а с ним ушёл широкий глиф «➕» пункта «добавить в словарь»:
        // его хвостовую ячейку diff шлёт только когда глиф нёс заметный на пустой
        // ячейке стиль (у выделенной строки — подложка `keycap_bg`), поэтому
        // страхуемся полной перерисовкой. На **сдвиг выделения** её не просим — глиф
        // остаётся широким, и
        // сентинел обязан пропускать его хвост (ratatui#2651), т.е. пользы там нет:
        // подсветку снимает перепечатка самого глифа. См. [`Self::request_full_redraw`].
        self.request_full_redraw();
    }

    /// Применяет выбранный пункт попапа подсказок и закрывает его.
    pub(super) fn apply_suggestion(&mut self) {
        let Some(popup) = self.suggest.take() else {
            return;
        };
        match popup.items.get(popup.selected) {
            Some(SuggestItem::Replace(word)) => {
                self.input
                    .replace_range(popup.row, popup.start, popup.end, word);
            }
            Some(SuggestItem::AddToDictionary) => {
                if let Some(spell) = &mut self.spell
                    && let Err(err) = spell.add_to_personal(&popup.word)
                {
                    tracing::warn!(error = %err, "не удалось дописать персональный словарь");
                }
            }
            None => {}
        }
        self.mark_input_changed();
    }

    /// Обрабатывает клавишу попапа выбора эмодзи: `Enter` вставляет выбранный
    /// эмодзи в поле ввода на месте курсора и закрывает попап, `Esc` — закрывает
    /// без вставки. См. spec §11.5.
    pub(super) fn handle_emoji_key(&mut self, key: KeyEvent) {
        let Some(picker) = &mut self.emoji else {
            return;
        };
        let action = picker.on_key(key);
        // Выделение читаем сразу (после `on_key` — оно могло сдвинуться), чтобы заём
        // `self.emoji` закончился здесь и ниже был доступен весь `self`.
        let selected = picker.selected();
        // Закрытие попапа убирает широкие глифы эмодзи с экрана, а хвостовые половины
        // НЕстилизованных глифов сетки поячеечный diff не шлёт (ratatui ≥ 0.1.2 шлёт
        // только у тех, что несли фон/REVERSED) → просим полную перерисовку.
        //
        // На **сдвиг выделения** перерисовку не просим: глиф остаётся широким, и
        // сентинел полной перерисовки обязан пропускать его хвост (иначе бэкенд
        // напечатает половину без `MoveTo` и сдвинет ряд — ratatui#2651). То есть для
        // сдвига она провабельно ничего не даёт — подложку снимает перепечатка самого
        // глифа, которая приходит обычным diff'ом. Канарейка на обе границы — в
        // `widgets::emoji_picker`. См. [`Self::request_full_redraw`].
        match action {
            EmojiPickerAction::None => {}
            EmojiPickerAction::Cancel => {
                // Запоминаем выделение и при отмене (попап помнит, где был курсор).
                self.emoji_last = selected;
                self.emoji = None;
                self.request_full_redraw();
            }
            EmojiPickerAction::Pick(emoji) => {
                self.emoji_last = selected;
                self.emoji = None;
                self.request_full_redraw();
                // insert_str безопасен для многоскалярных эмодзи (`❤️`, `👍🏽`) и
                // ставит курсор за вставленным.
                self.input.insert_str(&emoji);
                self.mark_input_changed();
            }
        }
    }
}

/// Горячие клавиши для вкладки «Горячие клавиши» (`F1`/`?`). Пары `(keycap, desc_key)`:
/// `keycap` — литеральная «клавиша» (ASCII, универсальна) **или** `ui.*`-ключ там,
/// где сам ярлык содержит слова (мышь); `desc_key` — всегда `ui.*`-ключ описания. Оба
/// резолвятся через локаль в [`key_lines`]. Команды поля ввода (`/…`) вынесены в
/// [`HELP_COMMANDS`] (отдельная вкладка). См. spec §11.7, docs/i18n-ui.md.
pub(super) const HELP_KEYS: &[(&str, &str)] = &[
    ("Enter", "ui.help.send"),
    ("Shift+Enter / Alt+Enter", "ui.help.newline"),
    ("Shift+←/→/↑/↓", "ui.help.select"),
    ("Ctrl+A", "ui.help.select_all"),
    ("Ctrl+C", "ui.help.copy"),
    ("Ctrl+X", "ui.help.cut"),
    ("Ctrl+V", "ui.help.paste"),
    ("Esc", "ui.help.esc"),
    ("Ctrl+N", "ui.help.new_chat"),
    ("F3", "ui.help.self_model"),
    ("F5", "ui.help.copy_chat"),
    ("Ctrl+R", "ui.help.regenerate"),
    ("Ctrl+E", "ui.help.delete_exchange"),
    ("Ctrl+U", "ui.help.impersonate"),
    ("Ctrl+K", "ui.help.clear_input"),
    ("Ctrl+Z / Ctrl+Y", "ui.help.undo_redo"),
    ("Ctrl+←/→", "ui.help.word_move"),
    ("Ctrl+Backspace/Delete", "ui.help.word_delete"),
    ("Ctrl+Home/End", "ui.help.doc_move"),
    ("Ctrl+P", "ui.help.settings"),
    ("Ctrl+T", "ui.help.thoughts"),
    ("Ctrl+G", "ui.help.spell"),
    ("Ctrl+B", "ui.help.emoji"),
    ("Ctrl+W", "ui.help.mouse_toggle"),
    ("ui.help.k.mouse", "ui.help.mouse_action"),
    ("PageUp/PageDown", "ui.help.scroll"),
    ("F1 / ?", "ui.help.help"),
    ("Ctrl+Q / F10", "ui.help.quit"),
];

/// Команды поля ввода для вкладки «Команды» (`F1`/`?`). Формат — как у [`HELP_KEYS`];
/// ярлык-команда (`/…`) рисуется цветом команды. Вынесены из клавиш, чтобы не мешать
/// их чтению. См. spec §11.7.
pub(super) const HELP_COMMANDS: &[(&str, &str)] = &[
    ("ui.help.k.rag_add", "ui.help.rag_add"),
    ("ui.help.k.rag_remove", "ui.help.rag_remove"),
    ("/rag list", "ui.help.rag_list"),
    ("/rag rebuild", "ui.help.rag_rebuild"),
    ("ui.help.k.tts", "ui.help.tts"),
    ("/tts stop", "ui.help.tts_stop"),
    ("/tts pause · resume", "ui.help.tts_pause"),
];

/// Ширина диалога справки/«О программе» в колонках (без рамки) — комфортная и
/// стабильная между вкладками, чтобы окно не «прыгало» при переключении.
const HELP_WIDTH: u16 = 76;
/// Высота диалога в строках (без рамки).
const HELP_HEIGHT: u16 = 34;

/// Рисует диалог справки/«О программе» по центру экрана (в стиле KDE/Qt): лockup
/// логотипа, таб-стрип вкладок и прокручиваемое содержимое активной вкладки со
/// скроллбаром на правой рамке. Навигация — в [`ChatScreen::handle_key`]; `scroll`
/// клампится здесь (высота попапа известна только при отрисовке). См. spec §11.7.
pub(super) fn render_help(
    frame: &mut Frame,
    help: &mut HelpState,
    palette: &Palette,
    loc: &'static Locale,
) {
    let full = frame.area();
    let area = centered_rect(HELP_WIDTH + 2, HELP_HEIGHT + 2, full);
    frame.render_widget(Clear, area);

    // Заголовок несёт бренд-имя+версию (язык-нейтрально) рядом с локализованным
    // титулом; футер — навигация по вкладкам/прокрутке/закрытию.
    let title = format!(
        "{}{} · {} v{}",
        palette.glyphs().help_icon,
        loc.t("ui.help.title"),
        credits::APP_NAME,
        env!("CARGO_PKG_VERSION"),
    );
    let block = palette.panel(title, true).title_bottom(
        Line::from(Span::styled(
            loc.t("ui.help.footer.tabs"),
            palette.muted_style(),
        ))
        .centered(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let inner_w = inner.width as usize;

    // Шапка: (опц. лockup + отбивка) + таб-стрип + разделительная линия. Лockup
    // рисуется только при запасе места (docs/branding.md §5) — на тесном терминале
    // он отодвинул бы вкладки; жёсткая деградация, как у скроллбара/Mermaid.
    let show_logo = inner.height >= LOCKUP_ROWS + 6 && inner.width > LOCKUP_COLS + LOGO_INDENT;
    let mut header: Vec<Line> = Vec::new();
    if show_logo {
        let pad = " ".repeat(LOGO_INDENT as usize);
        header.push(Line::raw("")); // отбивка над логотипом
        header.extend(lockup_lines(palette.text).into_iter().map(|line| {
            let mut spans = vec![Span::raw(pad.clone())];
            spans.extend(line.spans);
            Line::from(spans)
        }));
        header.push(Line::raw(""));
    }
    header.push(help_tab_strip(help.tab, palette, loc));
    header.push(Line::styled(
        "─".repeat(inner_w),
        palette.border_style(false),
    ));
    let header_h = header.len() as u16;

    let [head_area, body_area] =
        Layout::vertical([Constraint::Length(header_h), Constraint::Min(0)]).areas(inner);
    frame.render_widget(Paragraph::new(header), head_area);

    // Содержимое активной вкладки (язык-нейтральные данные — лицензия/компоненты —
    // берутся из `shared::credits` напрямую, минуя локали).
    let content = match help.tab {
        HelpTab::About => about_lines(palette, loc),
        HelpTab::Hotkeys => key_lines(HELP_KEYS, palette, loc),
        HelpTab::Commands => key_lines(HELP_COMMANDS, palette, loc),
        HelpTab::License => license_lines(palette, inner_w),
        HelpTab::Components => component_lines(palette, loc),
    };
    let total = content.len();
    let view_h = body_area.height as usize;
    help.scroll = help.scroll.min(total.saturating_sub(view_h));
    frame.render_widget(
        Paragraph::new(Text::from(content)).scroll((help.scroll as u16, 0)),
        body_area,
    );
    // Скроллбар — на правой линии рамки вдоль области содержимого (не всей высоты,
    // чтобы бегунок не заезжал на шапку с вкладками).
    let bar_area = Rect {
        x: area.x,
        y: body_area.y,
        width: area.width,
        height: body_area.height,
    };
    render_scrollbar(frame, bar_area, total, view_h, help.scroll, true, palette);
}

/// Таб-стрип вкладок диалога справки: `О программе │ Горячие клавиши │ …`. Активная
/// вкладка — на приглушённой подложке жирным (как выделенная вкладка настроек);
/// разделитель `│` и содержимое WGL4-безопасны. Диалог всегда модальный (в фокусе),
/// поэтому подсветка активной вкладки всегда «фокусная».
fn help_tab_strip(active: HelpTab, palette: &Palette, loc: &'static Locale) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
    for (i, tab) in HelpTab::ALL.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" │", palette.border_style(false)));
        }
        let label = loc.t(tab.label_key());
        let style = if *tab == active {
            Style::new().fg(palette.text).bg(palette.keycap_bg).bold()
        } else {
            palette.muted_style()
        };
        spans.push(Span::styled(format!(" {label} "), style));
    }
    Line::from(spans)
}

/// Отступ содержимого вкладок слева (та же колонка, что у лockup'а).
const HELP_PAD: &str = "  ";

/// Вкладка «О программе»: имя/описание + автор, версия и ссылки (сайт/репозиторий/
/// крейт). Ссылки — цветом акцента (как «клавиши-команды»), подписи — приглушённо.
fn about_lines(palette: &Palette, loc: &'static Locale) -> Vec<Line<'static>> {
    let rows: [(&str, String, Color); 5] = [
        (
            loc.t("ui.about.author"),
            credits::AUTHOR.to_string(),
            palette.text,
        ),
        (
            loc.t("ui.about.version"),
            env!("CARGO_PKG_VERSION").to_string(),
            palette.text,
        ),
        (
            loc.t("ui.about.site"),
            credits::SITE_URL.to_string(),
            palette.accent,
        ),
        (
            loc.t("ui.about.repo"),
            credits::REPO_URL.to_string(),
            palette.accent,
        ),
        (
            loc.t("ui.about.crate"),
            credits::CRATE_URL.to_string(),
            palette.accent,
        ),
    ];
    // Ширина колонки подписей (с двоеточием), чтобы значения выровнялись.
    let label_w = rows
        .iter()
        .map(|(l, _, _)| l.chars().count() + 1)
        .max()
        .unwrap_or(0);
    let mut lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            format!("{HELP_PAD}{}", credits::APP_NAME),
            Style::new().fg(palette.assistant).bold(),
        )),
        Line::from(Span::styled(
            format!("{HELP_PAD}{}", loc.t("ui.about.desc")),
            palette.muted_style(),
        )),
    ];
    // Пункты через пустую строку — список «дышит» (просьба: промежутки между элементами).
    for (label, value, color) in rows {
        lines.push(Line::raw(""));
        let field = format!("{label}:");
        let pad = " ".repeat((label_w + 1).saturating_sub(field.chars().count()));
        lines.push(Line::from(vec![
            Span::raw(HELP_PAD),
            Span::styled(field, palette.muted_style()),
            Span::raw(pad),
            Span::styled(value, Style::new().fg(color)),
        ]));
    }
    lines
}

/// Вкладки «Горячие клавиши»/«Команды»: список пар `(ярлык, описание)` — «клавиша» +
/// описание (ярлык-команда `/…` — цветом команды). Локаль резолвит и ярлыки-ключи, и
/// описания. Общий для обеих вкладок ([`HELP_KEYS`]/[`HELP_COMMANDS`]).
fn key_lines(
    entries: &[(&str, &str)],
    palette: &Palette,
    loc: &'static Locale,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];
    for (k, d) in entries {
        let key = loc.t(k).to_string();
        let desc = loc.t(d).to_string();
        let key_span = if key.starts_with('/') {
            Span::styled(format!(" {key} "), Style::new().fg(palette.warning))
        } else {
            palette.keycap(key)
        };
        lines.push(Line::from(vec![
            Span::raw(HELP_PAD),
            key_span,
            Span::styled(format!(" {desc}"), Style::new().fg(palette.text)),
        ]));
    }
    lines
}

/// Вкладка «Лицензия»: текст лицензии приложения (MIT). Абзацы (в файле разделены
/// пустой строкой) собираются заново и переносятся по словам под ширину содержимого
/// `width` — исходный жёсткий перенос под ~76 колонок иначе клипался бы справа, а
/// построчный перенос оставлял бы «сироты»-слова. Перенос даёт логические строки,
/// поэтому модель прокрутки/скроллбара (по числу строк) не ломается. Сборка по
/// `lines()` устойчива к CRLF.
fn license_lines(palette: &Palette, width: usize) -> Vec<Line<'static>> {
    let body_w = width.saturating_sub(HELP_PAD.len()).max(1);
    // Собираем абзацы: непустые строки склеиваются пробелом, пустая — граница.
    let mut paras: Vec<String> = Vec::new();
    let mut cur = String::new();
    for raw in credits::LICENSE_TEXT.lines() {
        if raw.trim().is_empty() {
            if !cur.is_empty() {
                paras.push(std::mem::take(&mut cur));
            }
        } else {
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(raw.trim());
        }
    }
    if !cur.is_empty() {
        paras.push(cur);
    }

    let mut lines = vec![Line::raw("")];
    for para in paras {
        let src = Line::from(Span::styled(para, Style::new().fg(palette.text)));
        for wrapped in crate::shared::wrap::wrap_line(&src, body_w) {
            let mut spans = vec![Span::raw(HELP_PAD)];
            spans.extend(wrapped.spans);
            lines.push(Line::from(spans));
        }
        lines.push(Line::raw("")); // отбивка между абзацами
    }
    lines
}

/// Вкладка «Компоненты»: имя (выровнено в колонку), версия и лицензия. Имя — основным
/// цветом, версия и лицензия — приглушённо, столбцы выровнены.
fn component_lines(palette: &Palette, loc: &'static Locale) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            format!("{HELP_PAD}{}", loc.t("ui.components.intro")),
            palette.muted_style(),
        )),
        Line::raw(""),
    ];
    let name_w = credits::COMPONENTS
        .iter()
        .map(|(n, ..)| n.chars().count())
        .max()
        .unwrap_or(0);
    let ver_w = credits::COMPONENTS
        .iter()
        .map(|(_, v, _)| v.chars().count())
        .max()
        .unwrap_or(0);
    for (name, version, license) in credits::COMPONENTS {
        let name_pad = " ".repeat(name_w + 2 - name.chars().count());
        let ver_pad = " ".repeat(ver_w + 2 - version.chars().count());
        lines.push(Line::from(vec![
            Span::raw(HELP_PAD),
            Span::styled((*name).to_string(), Style::new().fg(palette.text)),
            Span::raw(name_pad),
            Span::styled((*version).to_string(), palette.muted_style()),
            Span::raw(ver_pad),
            Span::styled((*license).to_string(), palette.muted_style()),
        ]));
    }
    lines
}

/// Рисует попап подсказок орфографии по центру экрана.
pub(super) fn render_suggest(
    frame: &mut Frame,
    popup: &SuggestPopup,
    palette: &Palette,
    loc: &'static Locale,
) {
    let rows = (popup.items.len() as u16 + 2).min(frame.area().height);
    let area = centered_rect(40, rows, frame.area());
    frame.render_widget(Clear, area);

    let block = palette
        .panel(popup.word.clone(), true)
        .title_bottom(Line::from(Span::styled(
            loc.t("ui.suggest.footer"),
            palette.muted_style(),
        )));
    let selected = popup.selected.min(popup.items.len().saturating_sub(1));
    let items: Vec<ListItem> = popup
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            // Рейл выделенной строки (2 колонки), как в списке чатов и настройках;
            // у прочих строк — отступ той же ширины, чтобы текст не «прыгал».
            let rail = if i == selected {
                Span::styled("▌ ", Style::new().fg(palette.success))
            } else {
                Span::raw("  ")
            };
            let body = match item {
                SuggestItem::Replace(word) => {
                    Span::styled(word.clone(), Style::new().fg(palette.text))
                }
                SuggestItem::AddToDictionary => Span::styled(
                    format!("{} {}", palette.glyphs().add, loc.t("ui.suggest.add")),
                    palette.success_style(),
                )
                .italic(),
            };
            ListItem::new(Line::from(vec![rail, body]))
        })
        .collect();
    // Выделение — мягкая подложка (как в списке чатов, настройках и «модели себя»),
    // а не инверсия всей строки: реверс свапает fg↔bg у каждого спана по отдельности,
    // из-за чего рейл `▌` (левый полублок) расползается, а спаны получают разный фон.
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::new().bg(palette.keycap_bg));
    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}

/// Рисует модальный попап подтверждения необратимой операции (`Ctrl+R`/`Ctrl+E`)
/// по центру экрана. См. spec §11.7.
pub(super) fn render_confirm(
    frame: &mut Frame,
    action: ConfirmAction,
    palette: &Palette,
    loc: &'static Locale,
) {
    let width = 56u16.min(frame.area().width);
    let area = centered_rect(width, 5, frame.area());
    frame.render_widget(Clear, area);
    let block = palette.panel(loc.t("ui.confirm.title"), true).title_bottom(
        Line::from(Span::styled(
            loc.t("ui.confirm.footer"),
            palette.muted_style(),
        ))
        .centered(),
    );
    let body = Paragraph::new(Line::from(Span::styled(
        action.prompt(loc),
        Style::new().fg(palette.text),
    )))
    .block(block)
    .wrap(Wrap { trim: true });
    frame.render_widget(body, area);
}

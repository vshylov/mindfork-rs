//! Экран чата — попапы: подсказки орфографии, подтверждение, эмодзи, справка. Часть модуля [`super`]; разбито из
//! монолита chat.rs (см. docs/history/refactoring-god-objects.md, этап 2).

use super::render::centered_rect;
use super::*;

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
            KeyCode::Up => popup.selected = popup.selected.saturating_sub(1),
            KeyCode::Down => {
                popup.selected = (popup.selected + 1).min(popup.items.len().saturating_sub(1));
            }
            KeyCode::Enter => self.apply_suggestion(),
            _ => {}
        }
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
        match picker.on_key(key) {
            EmojiPickerAction::None => {}
            EmojiPickerAction::Cancel => {
                // Запоминаем выделение и при отмене (попап помнит, где был курсор).
                self.emoji_last = picker.selected();
                self.emoji = None;
            }
            EmojiPickerAction::Pick(emoji) => {
                self.emoji_last = picker.selected();
                self.emoji = None;
                // insert_str безопасен для многоскалярных эмодзи (`❤️`, `👍🏽`) и
                // ставит курсор за вставленным.
                self.input.insert_str(&emoji);
                self.mark_input_changed();
            }
        }
    }
}

/// Список горячих клавиш для оверлея помощи (`F1`/`?`). Пары `(keycap, desc_key)`:
/// `keycap` — литеральная «клавиша» (ASCII, универсальна) **или** `ui.*`-ключ там,
/// где сам ярлык содержит слова (мышь, `<путь>`); `desc_key` — всегда `ui.*`-ключ
/// описания. Оба резолвятся через локаль в [`render_help`]. См. spec §11.7,
/// docs/i18n-ui.md.
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
    ("ui.help.k.rag_add", "ui.help.rag_add"),
    ("ui.help.k.rag_remove", "ui.help.rag_remove"),
    ("/rag list", "ui.help.rag_list"),
    ("/rag rebuild", "ui.help.rag_rebuild"),
    ("PageUp/PageDown", "ui.help.scroll"),
    ("F1 / ?", "ui.help.help"),
    ("Ctrl+Q / F10", "ui.help.quit"),
];

/// Рисует оверлей помощи по центру экрана: «клавиши» + приглушённые описания.
/// На коротком терминале список не помещается и прокручивается (`↑↓`/`PgUp`/
/// `PgDn` в `handle_key`) со скроллбаром на правой рамке; `scroll` клампится
/// здесь — только при отрисовке известна фактическая высота попапа.
pub(super) fn render_help(
    frame: &mut Frame,
    scroll: &mut usize,
    palette: &Palette,
    loc: &'static Locale,
) {
    // Резолвим клавиши и описания через локаль заранее (ширины и цвет команды
    // считаем от локализованных строк). Литеральный keycap (ASCII) → сам себя
    // (фолбэк `t` на отсутствующий ключ); `ui.*`-ключ → перевод.
    let resolved: Vec<(String, String)> = HELP_KEYS
        .iter()
        .map(|(k, d)| (loc.t(k).to_string(), loc.t(d).to_string()))
        .collect();
    let rows = (resolved.len() as u16 + 2).min(frame.area().height);
    let key_width = resolved
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0)
        + 4;
    let desc_width = resolved
        .iter()
        .map(|(_, d)| d.chars().count())
        .max()
        .unwrap_or(0);
    // Ширина строки: "  " слева + поле клавиш + " " + описание + "  " справа + рамка (2).
    let width = (2 + key_width + 1 + desc_width + 2 + 2) as u16;
    let area = centered_rect(width, rows, frame.area());
    frame.render_widget(Clear, area);

    let total = resolved.len();
    let view_h = area.height.saturating_sub(2) as usize; // минус рамка
    *scroll = (*scroll).min(total.saturating_sub(view_h));
    let hint = if total > view_h {
        loc.t("ui.help.footer.scroll")
    } else {
        loc.t("ui.help.footer.any")
    };
    let block = palette
        .panel(
            format!("{}{}", palette.glyphs().help_icon, loc.t("ui.help.title")),
            true,
        )
        .title_bottom(Line::from(Span::styled(hint, palette.muted_style())).centered());
    let lines: Vec<Line> = resolved
        .iter()
        .map(|(k, d)| {
            // Команды (`/rag …`) красим как команду, обычные клавиши — «клавишей».
            let key_span = if k.starts_with('/') {
                Span::styled(format!(" {k} "), Style::new().fg(palette.warning))
            } else {
                palette.keycap(k.clone())
            };
            Line::from(vec![
                Span::raw("  "),
                key_span,
                Span::styled(format!(" {d}"), Style::new().fg(palette.text)),
            ])
        })
        .collect();
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .scroll((*scroll as u16, 0)),
        area,
    );
    render_scrollbar(
        frame,
        area.inner(Margin::new(0, 1)),
        total,
        view_h,
        *scroll,
        true, // рамка попапа — в фокусном цвете (panel(_, true))
        palette,
    );
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
    let items: Vec<ListItem> = popup
        .items
        .iter()
        .map(|item| {
            ListItem::new(match item {
                SuggestItem::Replace(word) => {
                    Line::from(Span::styled(word.clone(), Style::new().fg(palette.text)))
                }
                SuggestItem::AddToDictionary => Line::from(
                    Span::styled(
                        format!("{} {}", palette.glyphs().add, loc.t("ui.suggest.add")),
                        palette.success_style(),
                    )
                    .italic(),
                ),
            })
        })
        .collect();
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::new().reversed());
    let mut state = ListState::default();
    state.select(Some(
        popup.selected.min(popup.items.len().saturating_sub(1)),
    ));
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

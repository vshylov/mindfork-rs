//! Экран чата — обработка клавиш/мыши/вставки, черновик, орфография, команды. Часть модуля [`super`]; разбито из
//! монолита chat.rs (см. docs/history/refactoring-god-objects.md, этап 2).

use super::feed::feed_msg_has_vs16;
use super::*;

impl ChatScreen {
    // ---------- ввод ----------

    /// Обрабатывает нажатие клавиши, возвращая намерение для `app` (или `None`,
    /// если клавиша обработана внутри экрана: ввод, скролл, навигация оверлея).
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<ChatIntent> {
        if key.kind != KeyEventKind::Press {
            return None;
        }
        // Оверлей помощи перехватывает ввод: ↑↓/PgUp/PgDn прокручивают список
        // (на коротком терминале он не помещается), любая другая клавиша
        // закрывает. Кламп прокрутки — в `render_help`.
        if self.show_help {
            match key.code {
                KeyCode::Up => self.help_scroll = self.help_scroll.saturating_sub(1),
                KeyCode::Down => self.help_scroll = self.help_scroll.saturating_add(1),
                KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(PAGE_SCROLL),
                KeyCode::PageDown => {
                    self.help_scroll = self.help_scroll.saturating_add(PAGE_SCROLL)
                }
                _ => self.show_help = false,
            }
            return None;
        }
        // Во время имперсонации поле ввода скрыто (показан предпросмотр): реагируем
        // только на отмену (`Esc`) и выход (`Ctrl+C`); прочие клавиши игнорируем.
        if self.impersonation.is_some() {
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && let KeyCode::Char(c) = key.code
                && keys::physical_char(c) == 'c'
            {
                return Some(ChatIntent::Quit);
            }
            if key.code == KeyCode::Esc {
                return Some(ChatIntent::CancelImpersonation);
            }
            return None;
        }
        // Модальный попап подтверждения (`Ctrl+R`/`Ctrl+E`): Enter — да, Esc — нет,
        // прочие клавиши игнорируются (попап остаётся открытым). См. spec §11.7.
        if self.confirm.is_some() {
            return self.handle_confirm_key(key);
        }
        if self.suggest.is_some() {
            self.handle_suggest_key(key);
            return None;
        }
        if self.emoji.is_some() {
            self.handle_emoji_key(key);
            return None;
        }
        if self.profile_overlay.is_some() {
            return self.handle_profile_overlay_key(key);
        }
        // Шорткаты с Ctrl матчим по «физической» латинской клавише — чтобы они
        // срабатывали при любой раскладке (русская ЙЦУКЕН даёт `Ctrl+д` вместо
        // `Ctrl+l`). См. shared::keys, spec §11.7.
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && let KeyCode::Char(c) = key.code
        {
            match keys::physical_char(c) {
                'c' => return Some(ChatIntent::Quit),
                // Экран настроек (Ctrl+P) — открывается, если снимок настроек получен.
                'p' => {
                    return self
                        .settings_snapshot
                        .is_some()
                        .then_some(ChatIntent::OpenSettings);
                }
                'n' => return self.request_new_chat(),
                // Перегенерация / удаление последнего обмена (только когда не идёт
                // генерация). См. spec §11.7.
                'r' => return self.trigger_destructive(ConfirmAction::Regenerate),
                'e' => return self.trigger_destructive(ConfirmAction::DeleteExchange),
                // Имперсонация: написать сообщение от лица пользователя (spec §11.8).
                // `seed` — уже введённый текст (модель продолжит его).
                'u' => {
                    return (!self.generating).then(|| ChatIntent::Impersonate {
                        seed: self.input.text(),
                    });
                }
                // Удалить весь текст ввода / вернуть удалённое (spec §11.5).
                // Повторное нажатие восстанавливает удалённое, если после него
                // ничего не вводилось.
                'k' => {
                    self.input.clear_or_restore();
                    self.mark_input_changed();
                    return None;
                }
                // Подсказки орфографии для слова под курсором (spec §11.5).
                'g' => {
                    self.open_suggestions();
                    return None;
                }
                // Попап выбора эмодзи (spec §11.5). Восстанавливаем прошлое выделение.
                'b' => {
                    self.emoji = Some(EmojiPickerState::with_selected(self.emoji_last));
                    return None;
                }
                // Сворачивание «мыслей» (spec §11.3).
                't' => {
                    self.feed_view.toggle_thoughts();
                    return None;
                }
                // Тумблер прокрутки колесом ↔ выделения текста мышью (spec §11.3).
                // `Ctrl+M` для этого непригоден: терминал отдаёт его как Enter.
                'w' => {
                    self.mouse_scroll = !self.mouse_scroll;
                    return Some(ChatIntent::SetMouseCapture(self.mouse_scroll));
                }
                _ => {}
            }
        }
        match (key.code, key.modifiers) {
            // Помощь по клавишам: F1 всегда; `?` — только при пустом вводе (иначе
            // символ печатается). См. spec §11.7.
            (KeyCode::F(1), _) => {
                self.show_help = true;
                self.help_scroll = 0;
                None
            }
            (KeyCode::Char('?'), KeyModifiers::NONE) if self.input.is_empty() => {
                self.show_help = true;
                self.help_scroll = 0;
                None
            }
            // Просмотр «модели себя» активного профиля (read-only вид).
            (KeyCode::F(3), _) => Some(ChatIntent::OpenSelfModel),
            // Копирование переписки активного чата в буфер обмена (как F5 в списке
            // чатов). Подтверждение/ошибка приходят заметкой в ленту (оверлея нет).
            (KeyCode::F(5), _) => self.active_chat.map(ChatIntent::CopyChat),
            // Прокрутка ленты (spec §11.3).
            (KeyCode::PageUp, _) => {
                self.feed_view.scroll_up(PAGE_SCROLL);
                None
            }
            (KeyCode::PageDown, _) => {
                self.feed_view.scroll_down(PAGE_SCROLL);
                None
            }
            // Esc открывает экран списка чатов (`app` создаёт его из снимка списка;
            // `Esc` там закрывает экран — переключение «список ↔ чат»). Во время
            // генерации Esc сперва отменяет её. Выход — `Ctrl+C`. См. spec §11.7.
            (KeyCode::Esc, _) => {
                if self.generating {
                    Some(ChatIntent::Cancel)
                } else {
                    Some(ChatIntent::OpenChatList)
                }
            }
            // Shift+Enter — перенос строки; Enter — отправка (spec §11.7).
            (KeyCode::Enter, KeyModifiers::SHIFT) => {
                self.input.insert_newline();
                self.mark_input_changed();
                None
            }
            (KeyCode::Enter, _) => {
                let text = self.input.text();
                if text.trim().is_empty() {
                    return None;
                }
                // Slash-команда RAG (`/rag add …`) — не отправляется как сообщение и
                // работает независимо от генерации (фоновая индексация).
                if let Some(parsed) = crate::features::rag_command::parse(&text) {
                    use crate::features::rag_command::RagCommand;
                    self.input.clear();
                    self.mark_input_changed();
                    return match parsed {
                        Ok(RagCommand::Add { path, recursive }) => {
                            Some(ChatIntent::RagAdd { path, recursive })
                        }
                        Ok(RagCommand::Delete { path }) => Some(ChatIntent::RagDelete { path }),
                        Ok(RagCommand::List) => Some(ChatIntent::RagList),
                        Ok(RagCommand::Rebuild) => Some(ChatIntent::RagRebuild),
                        Err(msg) => {
                            self.push_note(&format!("RAG: {msg}"));
                            None
                        }
                    };
                }
                if !self.generating {
                    self.input.clear();
                    self.mark_input_changed();
                    Some(ChatIntent::Send(text))
                } else {
                    None
                }
            }
            _ => {
                // Помечаем ввод «грязным» только на реальной правке — голое движение
                // курсора (`Moved`) не должно зря будить дебаунс орфографии и слать
                // `SetDraft`. См. [`KeyOutcome`].
                if self.input.on_key(key).edited() {
                    self.mark_input_changed();
                }
                None
            }
        }
    }

    /// Вставляет текст из буфера обмена в поле ввода (событие `Event::Paste` —
    /// bracketed paste). Вставка идёт одним куском, переводы строк сохраняются как
    /// текст (а НЕ трактуются как Enter/отправка). Работает только в основном виде:
    /// при открытой справке/попапе/оверлее (их однострочные поля) — no-op. Вставка
    /// не отправляет сообщение даже с переносами внутри. См. spec §11.5.
    pub fn handle_paste(&mut self, text: &str) {
        if self.show_help
            || self.suggest.is_some()
            || self.emoji.is_some()
            || self.profile_overlay.is_some()
            || self.confirm.is_some()
        {
            return;
        }
        if text.is_empty() {
            return;
        }
        self.input.insert_str(text);
        self.mark_input_changed();
    }

    /// Обрабатывает событие мыши: колесо прокручивает ленту чата. Работает только
    /// в основном виде — при открытом оверлее/попапе/справке прокрутка ленты под
    /// ними была бы неожиданной, поэтому это no-op. См. spec §11.3.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.show_help
            || self.suggest.is_some()
            || self.emoji.is_some()
            || self.profile_overlay.is_some()
            || self.confirm.is_some()
        {
            return;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => self.feed_view.scroll_up(WHEEL_SCROLL),
            MouseEventKind::ScrollDown => self.feed_view.scroll_down(WHEEL_SCROLL),
            _ => {}
        }
    }

    /// Забирает флаг «ленту только что прокрутили» и сообщает петле, нужна ли полная
    /// перерисовка терминала (`terminal.clear()` — стирает «висячие» артефакты).
    ///
    /// Полную перерисовку (с кратким миганием от escape-очистки экрана) делаем
    /// **только если** в ленте есть «съезжающие» на legacy-терминалах кластеры —
    /// VS16-эмодзи (содержат селектор U+FE0F, напр. `🕸️`/`🗂️`): Command Prompt/
    /// conhost рисует их физически шире модели ratatui, контент уезжает, и при
    /// странично-скачковой прокрутке остаётся «висячий» символ. Чистый текст и
    /// обычные широкие эмодзи артефактов не дают — там прокрутка не мигает.
    /// См. [`crate::widgets::message_feed::MessageFeed::take_scrolled`].
    pub fn take_feed_scrolled(&mut self) -> bool {
        // Сбросить внутренний флаг нужно всегда, даже если перерисовка не потребуется.
        if !self.feed_view.take_scrolled() {
            return false;
        }
        self.feed.iter().any(feed_msg_has_vs16)
    }

    /// Помечает ввод изменённым (запускает дебаунс перепроверки орфографии и
    /// сохранение черновика в активном чате).
    pub(super) fn mark_input_changed(&mut self) {
        self.spell_dirty = true;
        self.draft_dirty = true;
        self.last_edit = Some(Instant::now());
    }

    /// Забирает изменённый черновик ввода для сохранения в активном чате (или
    /// `None`, если с прошлого раза не менялся). Петля шлёт его командой `SetDraft`;
    /// запись на диск в оркестраторе идёт с дебаунсом. См. spec §11.7.
    pub fn take_dirty_draft(&mut self) -> Option<String> {
        if !self.draft_dirty {
            return None;
        }
        self.draft_dirty = false;
        Some(self.input.text())
    }

    /// Перепроверяет орфографию ввода, если истёк дебаунс. Возвращает `true`, если
    /// подсветка ошибок была пересчитана (нужна перерисовка). Вызывается из петли
    /// каждый тик (она и обеспечивает пробуждение по истечении дебаунса — рендер
    /// сам по тикам уже не запускается). См. spec §11.5.
    pub fn maybe_recheck_spelling(&mut self) -> bool {
        let Some(spell) = &self.spell else {
            return false;
        };
        if !self.spell_dirty {
            return false;
        }
        // Команды (`/rag …`) и пути файлов орфографией не проверяем — снимаем
        // возможные подчёркивания (они подсвечиваются жёлтым целиком при рендере).
        if self.input_is_command() {
            self.input.set_misspelled(Vec::new());
            self.spell_dirty = false;
            return true;
        }
        if let Some(t) = self.last_edit
            && t.elapsed() < SPELL_DEBOUNCE
        {
            return false; // ещё печатает — не флагуем текущее слово
        }
        let ranges = self
            .input
            .line_strings()
            .iter()
            .map(|line| spell.misspellings(line))
            .collect();
        self.input.set_misspelled(ranges);
        self.spell_dirty = false;
        true
    }

    /// Является ли текущий ввод командой (`/rag …`). Такой текст подсвечивается
    /// жёлтым и не проверяется орфографией. См. spec §11.5.
    pub(super) fn input_is_command(&self) -> bool {
        crate::features::rag_command::parse(&self.input.text()).is_some()
    }

    /// Запускает необратимую операцию (`Ctrl+R`/`Ctrl+E`): сразу отдаёт намерение,
    /// либо — если включено подтверждение — открывает модальный попап. Во время
    /// генерации обе операции игнорируются (как было). См. spec §11.7.
    pub(super) fn trigger_destructive(&mut self, action: ConfirmAction) -> Option<ChatIntent> {
        if self.generating {
            return None;
        }
        if self.confirm_destructive {
            self.confirm = Some(action);
            None
        } else {
            Some(action.intent())
        }
    }

    /// Запрашивает создание чата: при >1 профиле открывает оверлей выбора,
    /// иначе сразу создаёт из единственного/дефолтного профиля (spec §10).
    /// Публичный: `app` вызывает его, когда `Ctrl+N` нажат в экране списка чатов
    /// (выбор профиля живёт здесь, в экране чата).
    pub fn request_new_chat(&mut self) -> Option<ChatIntent> {
        if self.profiles.len() > 1 {
            self.profile_overlay = Some(ProfileListState::new(self.profiles.clone()));
            None
        } else {
            Some(ChatIntent::NewChat {
                profile_id: self.profiles.first().map(|p| p.id),
            })
        }
    }

    pub(super) fn handle_profile_overlay_key(&mut self, key: KeyEvent) -> Option<ChatIntent> {
        let overlay = self.profile_overlay.as_mut()?;
        match overlay.on_key(key) {
            ProfileListAction::None => None,
            ProfileListAction::Cancel => {
                self.profile_overlay = None;
                None
            }
            ProfileListAction::Pick(id) => {
                self.profile_overlay = None;
                Some(ChatIntent::NewChat {
                    profile_id: Some(id),
                })
            }
        }
    }
}

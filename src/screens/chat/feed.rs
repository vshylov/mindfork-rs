//! Экран чата — проекция AppEvent в ленту (сообщения, генерация, tool-блоки, токены). Часть модуля [`super`]; разбито из
//! монолита chat.rs (см. docs/history/refactoring-god-objects.md, этап 2).

use super::*;

impl ChatScreen {
    /// Обновляет заголовок чата в проекции (после ручного/авто-переименования).
    /// Меняет заголовок в шапке ленты, если это активный чат. Список и оверлей
    /// дополнительно обновляются событием `ChatList` (`set_chat_list`).
    pub fn rename_chat(&mut self, id: Uuid, title: String) {
        if self.active_chat == Some(id) {
            self.title = title.clone();
        }
        if let Some(c) = self.chats.iter_mut().find(|c| c.id == id) {
            c.title = title;
        }
    }

    pub fn activate_chat(&mut self, id: Uuid, title: String, messages: &[Message], draft: &str) {
        // Смена чата сбрасывает состояние генерации: «осиротевшие» чанки прежней
        // генерации не должны попадать в ленту нового чата.
        self.active_chat = Some(id);
        self.title = title;
        self.current_gen = None;
        self.generating = false;
        // Счётчик токенов относится к прежнему чату — гасим его, чтобы он не висел
        // в строке статуса после переключения (статус-бар скрывает счётчик при
        // `tokens == 0 && context == None`).
        self.gen_tokens = 0;
        self.gen_context = None;
        self.gen_context_exact = false;
        self.gen_reasoning = 0;
        // Склейка раундов agentic-loop в один блок «Ассистент:» с инлайн tool-блоками.
        self.feed = FeedMessage::from_messages(messages);
        self.feed_view.scroll_to_bottom();
        // Загружаем сохранённый черновик чата в поле ввода (пустой у нового чата).
        // НЕ помечаем `draft_dirty` — иначе тут же отправили бы его обратно тем же
        // `SetDraft`; перепроверку орфографии запускаем напрямую.
        self.input.set_text(draft);
        self.spell_dirty = true;
        self.last_edit = None;
    }

    /// Возвращает текст в поле ввода после удаления последнего обмена. Если поле
    /// непустое — текст добавляется в его начало (существующий ввод не теряется).
    /// См. spec §11.7.
    pub fn restore_input(&mut self, text: String) {
        let existing = self.input.text();
        let combined = if existing.is_empty() {
            text
        } else {
            format!("{text}{existing}")
        };
        self.input.set_text(&combined);
        self.mark_input_changed();
    }

    pub fn push_user_message(&mut self, text: String) {
        self.feed.push(FeedMessage {
            role: FeedRole::User,
            text,
            thoughts: String::new(),
            tools: Vec::new(),
            streaming: false,
        });
        self.feed_view.scroll_to_bottom();
    }

    pub fn begin_generation(&mut self, generation_id: Uuid) {
        self.current_gen = Some(generation_id);
        self.generating = true;
        self.gen_tokens = 0;
        self.gen_context = None;
        self.gen_context_exact = false;
        self.gen_reasoning = 0;
        self.pending_text_sep = false;
        self.pending_thoughts_sep = false;
        self.feed.push(FeedMessage {
            role: FeedRole::Assistant,
            text: String::new(),
            thoughts: String::new(),
            tools: Vec::new(),
            streaming: true,
        });
        self.feed_view.scroll_to_bottom();
    }

    /// Добавляет tool-блок к текущему сообщению ассистента (live во время хода).
    pub fn push_tool_call(
        &mut self,
        generation_id: Uuid,
        name: String,
        arguments: String,
        result: String,
    ) {
        if self.current_gen == Some(generation_id)
            && let Some(last) = self.feed.last_mut()
        {
            // Вызов сделан после уже накопленного текста ответа — фиксируем позицию,
            // чтобы tool-блок встал на месте вызова, а не в «шапке».
            let text_offset = last.text.len();
            last.tools.push(crate::widgets::message_feed::FeedToolCall {
                name,
                arguments,
                result,
                text_offset,
            });
            // Текст/мысли следующего раунда отделяем разделителем (как при перезагрузке).
            self.pending_text_sep = true;
            self.pending_thoughts_sep = true;
            self.feed_view.scroll_to_bottom();
        }
    }

    /// Ассистент написал сообщение и продолжает вторым (инструмент
    /// `send_followup_message`): завершаем текущий пузырь и добавляем новый
    /// стримящийся пузырь ассистента — в него пойдёт текст следующего раунда.
    /// Так live-лента совпадает с перезагрузкой (`from_messages` не склеивает
    /// сообщение с `new_bubble`). См. spec §9.3.
    pub fn continue_assistant(&mut self, generation_id: Uuid) {
        if self.current_gen != Some(generation_id) {
            return;
        }
        if let Some(last) = self.feed.last_mut() {
            last.streaming = false;
        }
        self.pending_text_sep = false;
        self.pending_thoughts_sep = false;
        self.feed.push(FeedMessage {
            role: FeedRole::Assistant,
            text: String::new(),
            thoughts: String::new(),
            tools: Vec::new(),
            streaming: true,
        });
        self.feed_view.scroll_to_bottom();
    }

    /// Ассистент решил переписать текущее сообщение (инструмент
    /// `rewrite_current_message`): отбрасываем уже накопленный текст/мысли/вызовы
    /// текущего пузыря — переписанный ответ пойдёт в него же. См. spec §9.3.
    pub fn rewrite_assistant(&mut self, generation_id: Uuid) {
        if self.current_gen != Some(generation_id) {
            return;
        }
        if let Some(last) = self.feed.last_mut() {
            last.text.clear();
            last.thoughts.clear();
            last.tools.clear();
            last.streaming = true;
        }
        self.pending_text_sep = false;
        self.pending_thoughts_sep = false;
        self.feed_view.scroll_to_bottom();
    }

    /// Гарантирует, что `last` — стримящийся пузырь ассистента (цель для чанков).
    /// Если посреди генерации в ленту вклинилась заметка (напр. `AppEvent::Error`
    /// о достижении лимита раундов перед финальным синтезом), `last` оказывается
    /// заметкой — тогда открываем новый пузырь ассистента, иначе стрим уходил бы в
    /// заметку и рендерился простым текстом без markdown.
    fn ensure_streaming_bubble(&mut self) {
        let ok = matches!(
            self.feed.last(),
            Some(m) if m.role == FeedRole::Assistant && m.streaming
        );
        if !ok {
            self.feed.push(FeedMessage {
                role: FeedRole::Assistant,
                text: String::new(),
                thoughts: String::new(),
                tools: Vec::new(),
                streaming: true,
            });
        }
    }

    pub fn push_chunk(&mut self, generation_id: Uuid, text: &str) {
        if self.current_gen != Some(generation_id) {
            return;
        }
        self.ensure_streaming_bubble();
        if let Some(last) = self.feed.last_mut() {
            // Первый текст раунда после вызова инструмента — с пустой строкой-
            // разделителем (совпадение с `FeedMessage::from_messages`).
            if self.pending_text_sep {
                self.pending_text_sep = false;
                if !last.text.is_empty() {
                    last.text.push_str("\n\n");
                }
            }
            last.text.push_str(text);
        }
    }

    pub fn push_thoughts(&mut self, generation_id: Uuid, text: &str) {
        if self.current_gen != Some(generation_id) {
            return;
        }
        self.ensure_streaming_bubble();
        if let Some(last) = self.feed.last_mut() {
            if self.pending_thoughts_sep {
                self.pending_thoughts_sep = false;
                if !last.thoughts.is_empty() {
                    last.thoughts.push('\n');
                }
            }
            last.thoughts.push_str(text);
        }
    }

    /// Обновляет счётчик токенов текущей генерации (live). Игнорирует устаревшие
    /// события (по `generation_id`). Контекст (переписку) обновляет только когда он
    /// задан (`Some`), запоминая, точное это число или оценка.
    pub fn set_token_usage(
        &mut self,
        generation_id: Uuid,
        completion: u64,
        context: Option<u64>,
        context_exact: bool,
        reasoning: Option<u32>,
    ) {
        if self.current_gen == Some(generation_id) {
            self.gen_tokens = completion;
            if let Some(c) = context {
                self.gen_context = Some(c);
                self.gen_context_exact = context_exact;
            }
            // Reasoning-токены известны только из `usage` (Some) — иначе не трогаем.
            if let Some(r) = reasoning {
                self.gen_reasoning = r;
            }
        }
    }

    pub fn finish_generation(&mut self, generation_id: Uuid, reason: FinishReason) {
        if self.current_gen != Some(generation_id) {
            return;
        }
        if let Some(last) = self.feed.last_mut() {
            last.streaming = false;
        }
        self.generating = false;
        self.current_gen = None;
        if reason == FinishReason::Cancelled {
            self.push_note("(генерация отменена)");
        }
    }

    pub fn push_error(&mut self, message: &str) {
        let warn = self.palette.glyphs().warn;
        self.push_note(&format!("{warn} {message}"));
    }

    /// Добавляет нейтральную заметку в ленту (напр. подтверждение операции списка
    /// чатов, когда экран списка уже закрыт — поздний ответ авто-названия/копии).
    pub fn push_note(&mut self, text: &str) {
        self.feed.push(FeedMessage::note(text));
        self.feed_view.scroll_to_bottom();
    }
}

/// Селектор эмодзи-представления (U+FE0F): делает VS16-эмодзи (`🕸️`, `🗂️`) шириной
/// 2 в эмодзи-способных терминалах. Именно такие кластеры «съезжают» на conhost и
/// требуют полной перерисовки при прокрутке (см. [`ChatScreen::take_feed_scrolled`]).
pub(super) const EMOJI_VS16: char = '\u{FE0F}';

/// Есть ли в элементе ленты «съезжающий» VS16-кластер (в тексте, «мыслях» или
/// аргументах/результате вызова инструмента).
pub(super) fn feed_msg_has_vs16(m: &FeedMessage) -> bool {
    let has = |s: &str| s.contains(EMOJI_VS16);
    has(&m.text) || has(&m.thoughts) || m.tools.iter().any(|t| has(&t.arguments) || has(&t.result))
}

//! Управление списком чатов: создание, переключение, переименование,
//! клонирование, копирование переписки, удаление и сохранение черновика.

use uuid::Uuid;

use crate::app::events::AppEvent;

use super::Orchestrator;

impl Orchestrator {
    /// Сохраняет черновик поля ввода в активном чате (несохранённый текст). Запись
    /// на диск идёт с дебаунсом (`mark_dirty`); `modified_at` НЕ трогаем — правка
    /// черновика не должна поднимать чат в списке. См. spec §11.7.
    pub(super) fn handle_set_draft(&mut self, text: String) {
        let Some(active_id) = self.active_id else {
            return;
        };
        if let Some(chat) = self.chat_mut(active_id) {
            if chat.draft == text {
                return;
            }
            chat.draft = text;
            self.mark_dirty(active_id);
        }
    }

    pub(super) fn handle_new_chat(&mut self, profile_id: Option<Uuid>) {
        let chat = self.new_chat_value(profile_id);
        let id = chat.id;
        if let Err(err) = self.storage.json().save_chat(&chat) {
            let _ = self.evt_tx.send(AppEvent::Error(
                self.ui_locale()
                    .tf("ui.err.chat_create_failed", &[("err", &err.to_string())]),
            ));
            return;
        }
        self.chats.insert(0, chat);
        self.emit_chat_list();
        self.activate(id);
    }

    pub(super) fn handle_switch(&mut self, id: Uuid) {
        if self.active_id == Some(id) {
            return;
        }
        // Озвучивание прерываем по настройке (по умолчанию — да: слушать чужой
        // чат неожиданно). См. spec §11.9.
        if self.config.tts.stop_on_chat_switch {
            self.stop_tts();
        }
        // Если идёт генерация — отменяем её (частичный ответ сохранится для
        // исходного чата по приходу GenResult).
        if let Some(token) = self.gen_state.request_cancel() {
            token.cancel();
        }
        if self.chats.iter().any(|c| c.id == id) {
            self.activate(id);
        }
    }

    pub(super) fn handle_rename(&mut self, id: Uuid, title: String) {
        let title = title.trim().to_string();
        if title.is_empty() {
            return;
        }
        if let Some(chat) = self.chat_mut(id) {
            chat.title = title.clone();
            self.mark_dirty(id);
            self.emit_chat_list();
            let _ = self.evt_tx.send(AppEvent::ChatRenamed { id, title });
        }
    }

    pub(super) fn handle_clone(&mut self, id: Uuid) {
        let Some(src) = self.chats.iter().find(|c| c.id == id) else {
            return;
        };
        let now = chrono::Utc::now();
        let mut clone = src.clone();
        clone.id = Uuid::new_v4();
        clone.title = self
            .ui_locale()
            .tf("ui.chat.clone_suffix", &[("orig", &src.title)]);
        clone.created_at = now;
        clone.modified_at = now;
        let new_id = clone.id;
        if let Err(err) = self.storage.json().save_chat(&clone) {
            let _ = self.evt_tx.send(AppEvent::ChatListError(
                self.ui_locale()
                    .tf("ui.err.chat_clone_failed", &[("err", &err.to_string())]),
            ));
            return;
        }
        self.chats.insert(0, clone);
        self.emit_chat_list();
        self.activate(new_id);
    }

    /// Копирование всей переписки чата в буфер обмена (spec §11.2): оркестратор
    /// (владелец `Chat`) формирует текст и эмитит `CopyToClipboard` — запись в буфер
    /// и подтверждение делает UI-слой (`runtime`). Пустой чат → понятная ошибка.
    pub(super) fn handle_copy_chat(&mut self, id: Uuid) {
        let Some(chat) = self.chats.iter().find(|c| c.id == id) else {
            return;
        };
        match crate::features::chat_export::format_conversation(
            &chat.title,
            &chat.messages,
            &self.config.copy,
            self.ui_locale(),
        ) {
            Some(text) => {
                let _ = self.evt_tx.send(AppEvent::CopyToClipboard(text));
            }
            None => {
                let _ = self.evt_tx.send(AppEvent::ChatListError(
                    self.ui_locale().t("ui.err.nothing_to_copy").into(),
                ));
            }
        }
    }

    pub(super) fn handle_delete(&mut self, id: Uuid) {
        // Безусловно (не настройка): озвучиваемого чата сейчас не станет.
        if self.active_id == Some(id) {
            self.stop_tts();
        }
        match self.storage.json().hide_chat(id) {
            Ok(false) => return,
            Err(err) => {
                let _ = self.evt_tx.send(AppEvent::ChatListError(
                    self.ui_locale()
                        .tf("ui.err.chat_delete_failed", &[("err", &err.to_string())]),
                ));
                return;
            }
            Ok(true) => {}
        }
        self.chats.retain(|c| c.id != id);
        self.saves.forget(id);

        // Если удалили активный — выбираем другой (или создаём новый).
        if self.active_id == Some(id) {
            self.active_id = None;
            if let Some(next) = self.chats.first().map(|c| c.id) {
                self.emit_chat_list();
                self.activate(next);
            } else {
                self.handle_new_chat(None);
            }
        } else {
            self.emit_chat_list();
        }
    }
}

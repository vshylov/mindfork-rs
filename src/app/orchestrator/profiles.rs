//! Управление профилями: создание, мягкое удаление с каскадом, правка.

use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::features::profiles::ProfileEdit;

use super::Orchestrator;

impl Orchestrator {
    /// Создаёт новый профиль (валидирует имя), сохраняет и обновляет список.
    pub(super) fn handle_create_profile(&mut self, name: String, system_message: String) {
        let Some(mut profile) = crate::features::profiles::create(&name, system_message) else {
            let _ = self.evt_tx.send(AppEvent::Error(
                self.ui_locale().t("ui.err.profile_name_empty").into(),
            ));
            return;
        };
        // Новый профиль создаётся на языке каркаса по умолчанию (defaults.json); пока
        // у него нет данных, язык можно сменить в настройках. См. docs/i18n.md.
        profile.language = self.default_language;
        if let Err(err) = self.storage.json().upsert_profile(&profile) {
            let _ = self.evt_tx.send(AppEvent::Error(
                self.ui_locale()
                    .tf("ui.err.profile_create_failed", &[("err", &err.to_string())]),
            ));
            return;
        }
        self.profiles.push(profile);
        self.emit_profile_list();
    }

    /// Мягко удаляет профиль с каскадом: скрываются его чаты, а заметки/RAG
    /// становятся недостижимы (профиль скрыт). См. spec §10, §12.3.
    pub(super) fn handle_delete_profile(&mut self, id: Uuid) {
        // Нельзя удалить последний профиль — иначе не из чего создавать чаты.
        if self.profiles.len() <= 1 {
            let _ = self.evt_tx.send(AppEvent::Error(
                self.ui_locale().t("ui.err.profile_delete_last").into(),
            ));
            return;
        }
        match self.storage.hide_profile_cascade(id) {
            Ok(false) => return,
            Err(err) => {
                let _ = self.evt_tx.send(AppEvent::Error(
                    self.ui_locale()
                        .tf("ui.err.profile_delete_failed", &[("err", &err.to_string())]),
                ));
                return;
            }
            Ok(true) => {}
        }
        self.profiles.retain(|p| p.id != id);
        // Убираем из памяти чаты удалённого профиля.
        let removed: Vec<Uuid> = self
            .chats
            .iter()
            .filter(|c| c.profile_id == id)
            .map(|c| c.id)
            .collect();
        self.chats.retain(|c| c.profile_id != id);
        for cid in &removed {
            self.saves.forget(*cid);
        }
        self.emit_profile_list();

        // Если активный чат принадлежал удалённому профилю — переключаемся.
        let active_removed = self.active_id.is_some_and(|a| removed.contains(&a));
        if active_removed {
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

    /// Применяет правки профиля (не затрагивает уже созданные чаты — у них свои
    /// копии, spec §10). Сохраняет и переэмитит список профилей/настройки.
    pub(super) fn handle_update_profile(&mut self, id: Uuid, mut edit: ProfileEdit) {
        // Авторитетный гейт смены языка каркаса (ось A, docs/i18n.md): если у профиля
        // уже есть данные, реальная смена языка отклоняется (страховка поверх
        // блокировки поля в UI). Правку языка при этом гасим, остальные — применяем.
        if let Some(new_lang) = edit.language {
            let current = self
                .profiles
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.language);
            if current == Some(new_lang) {
                edit.language = None; // язык не меняется — гейт неактуален
            } else if self.profile_has_data(id) {
                edit.language = None;
                let _ = self.evt_tx.send(AppEvent::Error(
                    self.ui_locale().t("ui.err.profile_language_locked").into(),
                ));
            }
        }
        let Some(profile) = self.profiles.iter_mut().find(|p| p.id == id) else {
            return;
        };
        if !crate::features::profiles::apply_edit(profile, edit) {
            let _ = self.evt_tx.send(AppEvent::Error(
                self.ui_locale().t("ui.err.profile_name_empty").into(),
            ));
            return;
        }
        let profile = profile.clone();
        if let Err(err) = self.storage.json().upsert_profile(&profile) {
            let _ = self.evt_tx.send(AppEvent::Error(
                self.ui_locale()
                    .tf("ui.err.profile_save_failed", &[("err", &err.to_string())]),
            ));
            return;
        }
        self.emit_profile_list();
        self.emit_settings();
    }
}

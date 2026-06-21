//! Управление профилями: создание, мягкое удаление с каскадом, правка.

use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::features::profiles::ProfileEdit;

use super::Orchestrator;

impl Orchestrator {
    /// Создаёт новый профиль (валидирует имя), сохраняет и обновляет список.
    pub(super) fn handle_create_profile(&mut self, name: String, system_message: String) {
        let Some(profile) = crate::features::profiles::create(&name, system_message) else {
            let _ = self
                .evt_tx
                .send(AppEvent::Error("Имя профиля не может быть пустым".into()));
            return;
        };
        if let Err(err) = self.storage.json().upsert_profile(&profile) {
            let _ = self.evt_tx.send(AppEvent::Error(format!(
                "Не удалось создать профиль: {err}"
            )));
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
            let _ = self
                .evt_tx
                .send(AppEvent::Error("Нельзя удалить последний профиль".into()));
            return;
        }
        match self.storage.hide_profile_cascade(id) {
            Ok(false) => return,
            Err(err) => {
                let _ = self.evt_tx.send(AppEvent::Error(format!(
                    "Не удалось удалить профиль: {err}"
                )));
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
    pub(super) fn handle_update_profile(&mut self, id: Uuid, edit: ProfileEdit) {
        let Some(profile) = self.profiles.iter_mut().find(|p| p.id == id) else {
            return;
        };
        if !crate::features::profiles::apply_edit(profile, edit) {
            let _ = self
                .evt_tx
                .send(AppEvent::Error("Имя профиля не может быть пустым".into()));
            return;
        }
        let profile = profile.clone();
        if let Err(err) = self.storage.json().upsert_profile(&profile) {
            let _ = self.evt_tx.send(AppEvent::Error(format!(
                "Не удалось сохранить профиль: {err}"
            )));
            return;
        }
        self.emit_profile_list();
        self.emit_settings();
    }
}

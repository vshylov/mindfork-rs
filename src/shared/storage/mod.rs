//! Слой хранения (`shared/storage`): JSON (конфиг/чаты/профили) + SQLite
//! (заметки/RAG). Фасад [`Storage`] объединяет оба и координирует каскадное
//! мягкое удаление. См. spec §5.2, §4.4.2.

pub mod db;
pub mod json;

use anyhow::Result;
use uuid::Uuid;

use crate::shared::paths::Paths;

pub use db::Db;
pub use json::JsonStore;

/// Единый фасад хранилища. Единственный писатель — оркестратор (spec §4.4).
pub struct Storage {
    json: JsonStore,
    db: Db,
}

impl Storage {
    /// Открывает хранилище по путям приложения.
    pub fn open(paths: Paths) -> Result<Self> {
        let db = Db::open(&paths.data_db())?;
        let json = JsonStore::new(paths);
        Ok(Self { json, db })
    }

    /// JSON-репозиторий (конфиг/профили/чаты).
    pub fn json(&self) -> &JsonStore {
        &self.json
    }

    /// SQLite-репозиторий (заметки/RAG).
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// Мягкое удаление профиля с каскадом на его чаты. Заметки/RAG физически
    /// не удаляются — они исключаются из выдачи, т.к. профиль скрыт
    /// (фильтрация по видимым профилям — на стороне выборок). См. spec §12.3.
    pub fn hide_profile_cascade(&self, profile_id: Uuid) -> Result<bool> {
        let found = self.json.hide_profile(profile_id)?;
        if found {
            self.json.hide_chats_of_profile(profile_id)?;
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::chat::Chat;
    use crate::entities::profile::Profile;

    #[test]
    fn hide_profile_cascades_to_chats() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(Paths::with_root(dir.path())).unwrap();

        let p = Profile::new("A", "s");
        storage.json().upsert_profile(&p).unwrap();
        storage
            .json()
            .save_chat(&Chat::from_profile(&p, "c1"))
            .unwrap();
        storage
            .json()
            .save_chat(&Chat::from_profile(&p, "c2"))
            .unwrap();

        assert!(storage.hide_profile_cascade(p.id).unwrap());
        assert!(storage.json().load_profiles().unwrap()[0].is_hidden);
        assert!(
            storage
                .json()
                .load_chats()
                .unwrap()
                .iter()
                .all(|c| c.is_hidden)
        );
    }
}

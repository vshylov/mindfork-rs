//! JSON-хранилище: конфиг (`settings.json`), профили (`profiles.json`),
//! чаты (`chats/{id}.json`). Атомарная запись (write-temp + rename) с бэкапом.
//! См. spec §5.2.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use uuid::Uuid;

use crate::entities::chat::Chat;
use crate::entities::profile::Profile;
use crate::shared::config::AppConfig;
use crate::shared::paths::Paths;

/// Файловое JSON-хранилище конфигурации, профилей и чатов.
pub struct JsonStore {
    paths: Paths,
}

impl JsonStore {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }

    // ---------- конфигурация ----------

    /// Загружает конфиг; если файла нет — возвращает дефолтный.
    pub fn load_config(&self) -> Result<AppConfig> {
        Ok(read_json(&self.paths.settings_file())?.unwrap_or_default())
    }

    pub fn save_config(&self, config: &AppConfig) -> Result<()> {
        write_json(&self.paths.settings_file(), config)
    }

    // ---------- профили (один файл со списком) ----------

    /// Все профили (включая скрытые).
    pub fn load_profiles(&self) -> Result<Vec<Profile>> {
        Ok(read_json(&self.paths.profiles_file())?.unwrap_or_default())
    }

    pub fn save_profiles(&self, profiles: &[Profile]) -> Result<()> {
        write_json(&self.paths.profiles_file(), &profiles)
    }

    /// Добавляет или заменяет профиль по `id`.
    pub fn upsert_profile(&self, profile: &Profile) -> Result<()> {
        let mut profiles = self.load_profiles()?;
        match profiles.iter_mut().find(|p| p.id == profile.id) {
            Some(existing) => *existing = profile.clone(),
            None => profiles.push(profile.clone()),
        }
        self.save_profiles(&profiles)
    }

    /// Помечает профиль скрытым. Возвращает `true`, если профиль найден.
    pub fn hide_profile(&self, id: Uuid) -> Result<bool> {
        let mut profiles = self.load_profiles()?;
        let Some(p) = profiles.iter_mut().find(|p| p.id == id) else {
            return Ok(false);
        };
        p.is_hidden = true;
        self.save_profiles(&profiles)?;
        Ok(true)
    }

    // ---------- чаты (по файлу на чат) ----------

    pub fn save_chat(&self, chat: &Chat) -> Result<()> {
        write_json(&self.paths.chat_file(&chat.id.to_string()), chat)
    }

    pub fn load_chat(&self, id: Uuid) -> Result<Option<Chat>> {
        read_json(&self.paths.chat_file(&id.to_string()))
    }

    /// Все чаты (включая скрытые), отсортированных порядка не гарантирует.
    pub fn load_chats(&self) -> Result<Vec<Chat>> {
        let dir = self.paths.chats_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut chats = Vec::new();
        for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json")
                && let Some(chat) = read_json::<Chat>(&path)?
            {
                chats.push(chat);
            }
        }
        Ok(chats)
    }

    /// Помечает чат скрытым. Возвращает `true`, если чат найден.
    pub fn hide_chat(&self, id: Uuid) -> Result<bool> {
        let Some(mut chat) = self.load_chat(id)? else {
            return Ok(false);
        };
        chat.is_hidden = true;
        self.save_chat(&chat)?;
        Ok(true)
    }

    /// Скрывает все чаты профиля (каскад при скрытии профиля). Возвращает число.
    pub fn hide_chats_of_profile(&self, profile_id: Uuid) -> Result<usize> {
        let mut count = 0;
        for mut chat in self.load_chats()? {
            if chat.profile_id == profile_id && !chat.is_hidden {
                chat.is_hidden = true;
                self.save_chat(&chat)?;
                count += 1;
            }
        }
        Ok(count)
    }
}

/// Читает и десериализует JSON-файл; `None`, если файла нет.
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => {
            let value = serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display()))?;
            Ok(Some(value))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Атомарно записывает значение в JSON: бэкап существующего → temp → rename.
fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating dir {}", parent.display()))?;
    }
    if path.exists() {
        let backup = path.with_extension("bak");
        let _ = fs::copy(path, &backup);
    }
    let data = serde_json::to_vec_pretty(value).context("serializing to JSON")?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, &data).with_context(|| format!("writing {}", tmp.display()))?;
    // std::fs::rename заменяет существующий файл и на Windows, и на Unix.
    fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, JsonStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonStore::new(Paths::with_root(dir.path()));
        (dir, store)
    }

    #[test]
    fn config_defaults_when_missing_then_roundtrips() {
        let (_d, s) = store();
        let cfg = s.load_config().unwrap();
        assert_eq!(cfg, AppConfig::default());

        let mut changed = cfg;
        changed.max_tool_rounds = 3;
        s.save_config(&changed).unwrap();
        assert_eq!(s.load_config().unwrap().max_tool_rounds, 3);
    }

    #[test]
    fn profile_upsert_and_hide() {
        let (_d, s) = store();
        let mut p = Profile::new("Joyce", "sys");
        s.upsert_profile(&p).unwrap();
        assert_eq!(s.load_profiles().unwrap().len(), 1);

        p.name = "Joyce 2".into();
        s.upsert_profile(&p).unwrap();
        let all = s.load_profiles().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Joyce 2");

        assert!(s.hide_profile(p.id).unwrap());
        assert!(s.load_profiles().unwrap()[0].is_hidden);
        assert!(!s.hide_profile(Uuid::new_v4()).unwrap());
    }

    #[test]
    fn chat_save_load_list_hide() {
        let (_d, s) = store();
        let p = Profile::new("X", "s");
        let chat = Chat::from_profile(&p, "Чат 1");
        s.save_chat(&chat).unwrap();

        assert_eq!(s.load_chat(chat.id).unwrap().unwrap().title, "Чат 1");
        assert_eq!(s.load_chats().unwrap().len(), 1);

        assert!(s.hide_chat(chat.id).unwrap());
        assert!(s.load_chat(chat.id).unwrap().unwrap().is_hidden);
    }

    #[test]
    fn hide_chats_of_profile_cascades() {
        let (_d, s) = store();
        let p1 = Profile::new("A", "s");
        let p2 = Profile::new("B", "s");
        s.save_chat(&Chat::from_profile(&p1, "c1")).unwrap();
        s.save_chat(&Chat::from_profile(&p1, "c2")).unwrap();
        s.save_chat(&Chat::from_profile(&p2, "c3")).unwrap();

        assert_eq!(s.hide_chats_of_profile(p1.id).unwrap(), 2);
        let visible = s
            .load_chats()
            .unwrap()
            .into_iter()
            .filter(|c| !c.is_hidden)
            .count();
        assert_eq!(visible, 1);
    }

    #[test]
    fn write_creates_backup_of_previous() {
        let (d, s) = store();
        s.save_config(&AppConfig::default()).unwrap();
        let c = AppConfig {
            max_tool_rounds: 99,
            ..Default::default()
        };
        s.save_config(&c).unwrap();
        assert!(d.path().join("settings.bak").exists());
    }
}

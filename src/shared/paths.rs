//! Портативное расположение данных: всё лежит рядом с исполняемым файлом.
//! См. spec §5.2 (расположение данных) и §12.1 (конфигурация).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Набор путей к данным приложения, вычисленных от корневого каталога.
///
/// В продакшене корень — каталог исполняемого файла (портативный режим);
/// в тестах используется временный каталог через [`Paths::with_root`].
#[derive(Debug, Clone)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    /// Определяет корень как каталог рядом с исполняемым файлом.
    pub fn discover() -> Result<Self> {
        let exe = std::env::current_exe().context("cannot resolve current executable path")?;
        let dir = exe
            .parent()
            .context("cannot determine executable directory")?;
        Ok(Self::with_root(dir))
    }

    /// Создаёт набор путей от произвольного корня (используется в тестах).
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Корневой каталог данных.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Глобальная конфигурация (`settings.json`).
    pub fn settings_file(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    /// Список профилей (`profiles.json`).
    pub fn profiles_file(&self) -> PathBuf {
        self.root.join("profiles.json")
    }

    /// Каталог с файлами чатов (`chats/`).
    pub fn chats_dir(&self) -> PathBuf {
        self.root.join("chats")
    }

    /// Файл конкретного чата (`chats/{id}.json`).
    pub fn chat_file(&self, chat_id: &str) -> PathBuf {
        self.chats_dir().join(format!("{chat_id}.json"))
    }

    /// База данных заметок и RAG (`data.db`).
    pub fn data_db(&self) -> PathBuf {
        self.root.join("data.db")
    }

    /// Каталог Hunspell-словарей (`dictionaries/`).
    pub fn dictionaries_dir(&self) -> PathBuf {
        self.root.join("dictionaries")
    }

    /// Персональный словарь спелл-чекера (`personal_dictionary.txt`).
    pub fn personal_dictionary(&self) -> PathBuf {
        self.root.join("personal_dictionary.txt")
    }

    /// Каталог логов (`logs/`).
    pub fn log_dir(&self) -> PathBuf {
        self.root.join("logs")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_live_under_root() {
        let p = Paths::with_root("data-root");
        assert!(p.settings_file().ends_with("settings.json"));
        assert!(p.profiles_file().ends_with("profiles.json"));
        assert!(p.data_db().ends_with("data.db"));
        assert!(p.personal_dictionary().ends_with("personal_dictionary.txt"));
        assert!(p.chats_dir().ends_with("chats"));
        assert!(p.dictionaries_dir().ends_with("dictionaries"));
        assert!(p.log_dir().ends_with("logs"));
    }

    #[test]
    fn chat_file_is_named_by_id_under_chats_dir() {
        let p = Paths::with_root("data-root");
        let f = p.chat_file("abc123");
        assert!(f.ends_with("abc123.json"));
        assert_eq!(f.parent(), Some(p.chats_dir().as_path()));
    }

    #[test]
    fn root_is_preserved() {
        let p = Paths::with_root("some/root");
        assert_eq!(p.root(), Path::new("some/root"));
    }
}

//! Расположение пользовательских данных. По умолчанию — портативный режим: данные
//! лежат в подкаталоге `data/` рядом с исполняемым файлом (подкаталог отделяет данные
//! от служебных файлов/кэшей сборки, особенно в dev — `target/debug/data/`). Файл-
//! маркер `location.json` рядом с бинарником может переключить хранение в стандартную
//! ОС-папку или в произвольный каталог. См. spec §5.2 (расположение данных) и §12.1.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Имя файла-маркера режима хранения (всегда лежит рядом с бинарником, не в `data/`).
pub const LOCATION_MARKER: &str = "location.json";

/// Подкаталог данных в портативном режиме (рядом с бинарником).
pub const PORTABLE_DATA_SUBDIR: &str = "data";

/// Режим хранения пользовательских данных, заданный файлом-маркером `location.json`
/// рядом с исполняемым файлом. Отсутствие/пустой маркер → [`DataLocation::Portable`]
/// (обратная совместимость: существующие установки держат данные рядом с бинарником).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum DataLocation {
    /// Данные рядом с исполняемым файлом (портативная установка).
    #[default]
    Portable,
    /// Данные в стандартной ОС-папке пользователя (Windows `%APPDATA%\mindfork-rs`,
    /// Linux `~/.local/share/mindfork-rs`).
    System,
    /// Данные в произвольном каталоге, указанном пользователем.
    Path { path: String },
}

impl DataLocation {
    /// Читает маркер режима хранения по пути `marker`. Файла нет → `Portable`.
    /// Повреждённый JSON — **ошибка** (а не молчаливый откат к портативному), чтобы
    /// опечатка в пути не привела к работе с пустым набором данных не там, где надо.
    pub fn read(marker: &Path) -> Result<Self> {
        match std::fs::read(marker) {
            Ok(bytes) if bytes.iter().all(u8::is_ascii_whitespace) => Ok(Self::Portable),
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("разбор файла-маркера {}", marker.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::Portable),
            Err(e) => Err(e).with_context(|| format!("чтение файла-маркера {}", marker.display())),
        }
    }

    /// Корневой каталог данных для этого режима. `exe_dir` — каталог бинарника
    /// (в портативном режиме корень = `exe_dir/data`).
    pub fn root_dir(&self, exe_dir: &Path) -> Result<PathBuf> {
        match self {
            Self::Portable => Ok(exe_dir.join(PORTABLE_DATA_SUBDIR)),
            Self::System => {
                let dirs = directories::ProjectDirs::from("", "", "mindfork-rs")
                    .context("не удалось определить стандартную ОС-папку для данных")?;
                Ok(dirs.data_dir().to_path_buf())
            }
            Self::Path { path } => {
                let trimmed = path.trim();
                anyhow::ensure!(
                    !trimmed.is_empty(),
                    "в режиме \"path\" файл-маркер {LOCATION_MARKER} должен задавать непустой путь"
                );
                Ok(PathBuf::from(trimmed))
            }
        }
    }
}

/// Набор путей к данным приложения, вычисленных от корневого каталога.
///
/// В продакшене корень — каталог исполняемого файла (портативный режим);
/// в тестах используется временный каталог через [`Paths::with_root`].
#[derive(Debug, Clone)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    /// Определяет корень данных по файлу-маркеру `location.json` рядом с бинарником.
    /// Нет маркера → портативный режим (корень = `data/` рядом с бинарником). Корень
    /// при необходимости создаётся (в т.ч. портативный подкаталог `data/`).
    pub fn discover() -> Result<Self> {
        let exe = std::env::current_exe().context("cannot resolve current executable path")?;
        let exe_dir = exe
            .parent()
            .context("cannot determine executable directory")?;

        let location = DataLocation::read(&exe_dir.join(LOCATION_MARKER))?;
        let root = location.root_dir(exe_dir)?;
        std::fs::create_dir_all(&root)
            .with_context(|| format!("создание каталога данных {}", root.display()))?;
        Ok(Self::with_root(root))
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

    /// Каталог резервных копий (`backups/`).
    pub fn backups_dir(&self) -> PathBuf {
        self.root.join("backups")
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

    #[test]
    fn backups_dir_under_root() {
        let p = Paths::with_root("data-root");
        assert!(p.backups_dir().ends_with("backups"));
        assert_eq!(p.backups_dir().parent(), Some(p.root()));
    }

    #[test]
    fn missing_marker_is_portable() {
        let dir = tempfile::tempdir().unwrap();
        let loc = DataLocation::read(&dir.path().join(LOCATION_MARKER)).unwrap();
        assert_eq!(loc, DataLocation::Portable);
        // Портативный режим: корень = подкаталог `data/` рядом с бинарником.
        assert_eq!(
            loc.root_dir(dir.path()).unwrap(),
            dir.path().join(PORTABLE_DATA_SUBDIR)
        );
    }

    #[test]
    fn whitespace_marker_is_portable() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join(LOCATION_MARKER);
        std::fs::write(&marker, "  \n\t").unwrap();
        assert_eq!(DataLocation::read(&marker).unwrap(), DataLocation::Portable);
    }

    #[test]
    fn portable_and_path_modes_round_trip() {
        for (json, expect) in [
            (r#"{"mode":"portable"}"#, DataLocation::Portable),
            (r#"{"mode":"system"}"#, DataLocation::System),
            (
                r#"{"mode":"path","path":"D:\\data"}"#,
                DataLocation::Path {
                    path: "D:\\data".into(),
                },
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let marker = dir.path().join(LOCATION_MARKER);
            std::fs::write(&marker, json).unwrap();
            assert_eq!(DataLocation::read(&marker).unwrap(), expect);
        }
    }

    #[test]
    fn explicit_path_mode_resolves_to_that_path() {
        let loc = DataLocation::Path {
            path: "  custom/data/dir  ".into(),
        };
        // Путь триммится; каталог бинарника игнорируется.
        assert_eq!(
            loc.root_dir(Path::new("/exe")).unwrap(),
            PathBuf::from("custom/data/dir")
        );
    }

    #[test]
    fn empty_path_mode_errors() {
        let loc = DataLocation::Path { path: "   ".into() };
        assert!(loc.root_dir(Path::new("/exe")).is_err());
    }

    #[test]
    fn corrupted_marker_errors() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join(LOCATION_MARKER);
        std::fs::write(&marker, "{ not valid json").unwrap();
        assert!(DataLocation::read(&marker).is_err());
    }

    #[test]
    fn system_mode_root_mentions_app() {
        // Не утверждаем точный путь (зависит от ОС/пользователя), но он должен
        // указывать на каталог приложения.
        let root = DataLocation::System.root_dir(Path::new("/exe")).unwrap();
        assert!(root.to_string_lossy().contains("mindfork-rs"));
    }
}

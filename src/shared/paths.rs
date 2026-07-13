//! Расположение пользовательских данных и установочные умолчания. По умолчанию —
//! портативный режим: данные лежат в подкаталоге `data/` рядом с исполняемым файлом
//! (подкаталог отделяет данные от служебных файлов/кэшей сборки, особенно в dev —
//! `target/debug/data/`). Файл-маркер `defaults.json` рядом с бинарником может
//! переключить хранение в стандартную ОС-папку или произвольный каталог **и** задать
//! язык служебного каркаса новых профилей (`default_language`, ось A — docs/i18n.md;
//! инсталлятор заполнит его по выбору пользователя при установке). Для обратной
//! совместимости читается и старый маркер `location.json` (только режим хранения).
//! См. spec §5.2 (расположение данных) и §12.1.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::shared::i18n::Lang;

/// Имя файла установочных умолчаний (всегда лежит рядом с бинарником, не в `data/`).
pub const DEFAULTS_MARKER: &str = "defaults.json";

/// Устаревшее имя файла-маркера (только режим хранения) — читается для обратной
/// совместимости, если `defaults.json` отсутствует.
pub const LEGACY_LOCATION_MARKER: &str = "location.json";

/// Подкаталог данных в портативном режиме (рядом с бинарником).
pub const PORTABLE_DATA_SUBDIR: &str = "data";

/// Режим хранения пользовательских данных, заданный файлом умолчаний `defaults.json`
/// рядом с исполняемым файлом (поле `mode`, при `path` — ещё `path`). Отсутствие/пустой
/// маркер → [`DataLocation::Portable`] (обратная совместимость: существующие установки
/// держат данные рядом с бинарником).
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
                    "в режиме \"path\" файл {DEFAULTS_MARKER} должен задавать непустой путь"
                );
                Ok(PathBuf::from(trimmed))
            }
        }
    }
}

/// Установочные умолчания из файла `defaults.json` рядом с бинарником: режим хранения
/// данных (`mode`/`path`, плоско — совместимо со старым `location.json`) **и** язык
/// служебного каркаса новых профилей (`default_language`, ось A — docs/i18n.md).
/// Заполняется инсталлятором (или вручную). Отсутствие полей → дефолты (портативно, `ru`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Defaults {
    /// Режим хранения (плоско в JSON: `mode`/`path` на верхнем уровне — байт-совместимо
    /// с прежним `location.json`).
    #[serde(flatten, default)]
    pub location: DataLocation,
    /// Язык служебного каркаса, на котором создаётся **первый** профиль (bootstrap) и
    /// новые профили. Не язык интерфейса и не язык ответа модели. См. docs/i18n.md.
    #[serde(default)]
    pub default_language: Lang,
}

impl Defaults {
    /// Читает установочные умолчания рядом с бинарником: сначала `defaults.json`, при
    /// его отсутствии — устаревший `location.json` (только режим хранения; язык =
    /// дефолт). Нет обоих/пустой файл → дефолты. Повреждённый JSON — **ошибка** (а не
    /// молчаливый откат), чтобы опечатка не увела на пустой набор данных не туда.
    pub fn read(exe_dir: &Path) -> Result<Self> {
        let primary = exe_dir.join(DEFAULTS_MARKER);
        let marker = if primary.exists() {
            primary
        } else {
            exe_dir.join(LEGACY_LOCATION_MARKER)
        };
        match std::fs::read(&marker) {
            Ok(bytes) if bytes.iter().all(u8::is_ascii_whitespace) => Ok(Self::default()),
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("разбор файла умолчаний {}", marker.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => {
                Err(e).with_context(|| format!("чтение файла умолчаний {}", marker.display()))
            }
        }
    }
}

/// Набор путей к данным приложения, вычисленных от корневого каталога.
///
/// В продакшене корень — каталог исполняемого файла (портативный режим);
/// в тестах используется временный каталог через [`Paths::with_root`]. Несёт также
/// язык каркаса новых профилей (`default_language`) из `defaults.json`.
#[derive(Debug, Clone)]
pub struct Paths {
    root: PathBuf,
    default_language: Lang,
}

impl Paths {
    /// Определяет корень данных и умолчания по файлу `defaults.json` рядом с бинарником
    /// (fallback — устаревший `location.json`). Нет файла → портативный режим (корень =
    /// `data/` рядом с бинарником), язык каркаса = дефолт (`ru`). Корень при
    /// необходимости создаётся (в т.ч. портативный подкаталог `data/`).
    pub fn discover() -> Result<Self> {
        let exe = std::env::current_exe().context("cannot resolve current executable path")?;
        let exe_dir = exe
            .parent()
            .context("cannot determine executable directory")?;

        let defaults = Defaults::read(exe_dir)?;
        let root = defaults.location.root_dir(exe_dir)?;
        std::fs::create_dir_all(&root)
            .with_context(|| format!("создание каталога данных {}", root.display()))?;
        let mut paths = Self::with_root(root);
        paths.default_language = defaults.default_language;
        // Каталог внешних локалей создаётся для обнаруживаемости (пустой каталог
        // сигналит «клади файлы сюда»); ошибку создания не эскалируем — внешние
        // локали опциональны, при их отсутствии работают вшитые бандлы.
        let _ = std::fs::create_dir_all(paths.locales_dir());
        Ok(paths)
    }

    /// Создаёт набор путей от произвольного корня (используется в тестах). Язык
    /// каркаса новых профилей — дефолт (`ru`).
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            default_language: Lang::default(),
        }
    }

    /// Корневой каталог данных.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Язык служебного каркаса новых профилей (из `defaults.json`, ось A). Bootstrap
    /// первого профиля и создание профилей берут его. См. docs/i18n.md.
    pub fn default_language(&self) -> Lang {
        self.default_language
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

    /// Каталог внешних локалей (`locales/`): `<code>.json` переопределяет вшитый бандл
    /// того же языка или добавляет новый язык без пересборки (ось A/B, Ярус 3 —
    /// docs/i18n-external-locales.md). Загружается один раз при старте.
    pub fn locales_dir(&self) -> PathBuf {
        self.root.join("locales")
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

    /// Каталог песочницы Python (`sandbox/`): бинарь `wasmer`, `python.webc`,
    /// `site-packages/`. Наполняется командой `mindfork sandbox setup` (Фаза 2).
    pub fn sandbox_dir(&self) -> PathBuf {
        self.root.join("sandbox")
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
    fn locales_dir_under_root() {
        let p = Paths::with_root("data-root");
        assert!(p.locales_dir().ends_with("locales"));
        assert_eq!(p.locales_dir().parent(), Some(p.root()));
    }

    #[test]
    fn missing_marker_is_portable() {
        let dir = tempfile::tempdir().unwrap();
        let loc = Defaults::read(dir.path()).unwrap().location;
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
        std::fs::write(dir.path().join(DEFAULTS_MARKER), "  \n\t").unwrap();
        assert_eq!(
            Defaults::read(dir.path()).unwrap().location,
            DataLocation::Portable
        );
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
            std::fs::write(dir.path().join(DEFAULTS_MARKER), json).unwrap();
            assert_eq!(Defaults::read(dir.path()).unwrap().location, expect);
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
        std::fs::write(dir.path().join(DEFAULTS_MARKER), "{ not valid json").unwrap();
        assert!(Defaults::read(dir.path()).is_err());
    }

    #[test]
    fn system_mode_root_mentions_app() {
        // Не утверждаем точный путь (зависит от ОС/пользователя), но он должен
        // указывать на каталог приложения.
        let root = DataLocation::System.root_dir(Path::new("/exe")).unwrap();
        assert!(root.to_string_lossy().contains("mindfork-rs"));
    }

    #[test]
    fn defaults_reads_storage_and_language() {
        // defaults.json несёт режим хранения (плоско) + язык каркаса.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(DEFAULTS_MARKER),
            r#"{"mode":"path","path":"D:\\data","default_language":"en"}"#,
        )
        .unwrap();
        let d = Defaults::read(dir.path()).unwrap();
        assert_eq!(
            d.location,
            DataLocation::Path {
                path: "D:\\data".into()
            }
        );
        assert_eq!(d.default_language, Lang::En);
    }

    #[test]
    fn defaults_missing_is_portable_ru() {
        let dir = tempfile::tempdir().unwrap();
        let d = Defaults::read(dir.path()).unwrap();
        assert_eq!(d, Defaults::default());
        assert_eq!(d.location, DataLocation::Portable);
        assert_eq!(d.default_language, Lang::Ru);
    }

    #[test]
    fn defaults_falls_back_to_legacy_location_marker() {
        // Нет defaults.json → читается старый location.json (только режим; язык дефолт).
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(LEGACY_LOCATION_MARKER),
            r#"{"mode":"system"}"#,
        )
        .unwrap();
        let d = Defaults::read(dir.path()).unwrap();
        assert_eq!(d.location, DataLocation::System);
        assert_eq!(d.default_language, Lang::Ru);
    }

    #[test]
    fn defaults_json_takes_precedence_over_legacy() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(DEFAULTS_MARKER),
            r#"{"mode":"portable","default_language":"en"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join(LEGACY_LOCATION_MARKER),
            r#"{"mode":"system"}"#,
        )
        .unwrap();
        let d = Defaults::read(dir.path()).unwrap();
        assert_eq!(d.location, DataLocation::Portable);
        assert_eq!(d.default_language, Lang::En);
    }

    #[test]
    fn defaults_corrupted_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(DEFAULTS_MARKER), "{ bad json").unwrap();
        assert!(Defaults::read(dir.path()).is_err());
    }

    #[test]
    fn with_root_defaults_to_ru_language() {
        assert_eq!(Paths::with_root("r").default_language(), Lang::Ru);
    }
}

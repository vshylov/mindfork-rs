//! Расположение пользовательских данных и установочные умолчания. По умолчанию —
//! портативный режим: данные лежат в подкаталоге `data/` рядом с исполняемым файлом
//! (подкаталог отделяет данные от служебных файлов/кэшей сборки, особенно в dev —
//! `target/debug/data/`). Файл-маркер `defaults.json` рядом с бинарником может
//! переключить хранение в стандартную ОС-папку или произвольный каталог **и** задать
//! язык служебного каркаса новых профилей (`default_language`, ось A — docs/history/i18n.md;
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
                // Контекст на английском: эта ошибка возникает в `Paths::resolve` до
                // определения языка CLI (docs/history/i18n-cli.md §7 — граница «до знания языка»).
                let dirs = directories::ProjectDirs::from("", "", "mindfork-rs")
                    .context("cannot determine the standard OS data folder")?;
                Ok(dirs.data_dir().to_path_buf())
            }
            Self::Path { path } => {
                let trimmed = path.trim();
                anyhow::ensure!(
                    !trimmed.is_empty(),
                    "in \"path\" mode the {DEFAULTS_MARKER} file must specify a non-empty path"
                );
                Ok(PathBuf::from(trimmed))
            }
        }
    }
}

/// Установочные умолчания из файла `defaults.json` рядом с бинарником: режим хранения
/// данных (`mode`/`path`, плоско — совместимо со старым `location.json`) **и** язык
/// служебного каркаса новых профилей (`default_language`, ось A — docs/history/i18n.md).
/// Заполняется инсталлятором (или вручную). Отсутствие полей → дефолты (портативно, язык
/// определяется по локали ОС — см. [`Paths::resolve`]).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Defaults {
    /// Режим хранения (плоско в JSON: `mode`/`path` на верхнем уровне — байт-совместимо
    /// с прежним `location.json`).
    #[serde(flatten, default)]
    pub location: DataLocation,
    /// Язык служебного каркаса, на котором создаётся **первый** профиль (bootstrap) и
    /// новые профили, **и** язык интерфейса при свежей установке. Не язык ответа модели.
    /// `Some(..)` — задан инсталлятором явно (Windows); `None` (поле отсутствует —
    /// напр. deb/rpm-пакет пишет только `{"mode":"system"}`, §4.3 installers.md) →
    /// [`Paths::resolve`] определяет язык по локали ОС. См. docs/history/i18n.md.
    #[serde(default)]
    pub default_language: Option<Lang>,
}

/// Отбрасывает ведущий UTF-8 BOM (`EF BB BF`), если он есть. Инсталляторы и редакторы
/// (Pascal-хелперы Inno, ряд Windows-редакторов) могут записать `defaults.json` с BOM —
/// `serde_json::from_slice` на нём падает. Прецеденты отбрасывания в проекте:
/// `rag_ingest::read_text`, импортёр LameLLaMA.
fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

impl Defaults {
    /// Читает установочные умолчания рядом с бинарником: сначала `defaults.json`, при
    /// его отсутствии — устаревший `location.json` (только режим хранения; язык = `None`).
    /// Нет обоих/пустой файл → дефолты. Повреждённый JSON — **ошибка** (а не молчаливый
    /// откат), чтобы опечатка не увела на пустой набор данных не туда. Ведущий UTF-8 BOM
    /// отбрасывается ([`strip_bom`]).
    pub fn read(exe_dir: &Path) -> Result<Self> {
        let primary = exe_dir.join(DEFAULTS_MARKER);
        let marker = if primary.exists() {
            primary
        } else {
            exe_dir.join(LEGACY_LOCATION_MARKER)
        };
        // Контексты на английском: `Defaults::read` вызывается из `Paths::resolve` до
        // определения языка CLI (docs/history/i18n-cli.md §7 — граница «до знания языка»).
        match std::fs::read(&marker) {
            Ok(bytes) => {
                let bytes = strip_bom(&bytes);
                if bytes.iter().all(u8::is_ascii_whitespace) {
                    Ok(Self::default())
                } else {
                    serde_json::from_slice(bytes)
                        .with_context(|| format!("parsing defaults file {}", marker.display()))
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading defaults file {}", marker.display())),
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
    /// Каталог исполняемого файла (на Linux — реальный путь: `current_exe` резолвит
    /// `/proc/self/exe`, т.е. цель симлинка `/usr/bin/…` → `/usr/lib/<pkg>/…`). Нужен,
    /// чтобы найти read-only ресурсы, положенные рядом с бинарником (словари, §4.2
    /// installers.md), когда корень данных — не портативный (`system`/`path`). `None` в
    /// тестах (`with_root`).
    exe_dir: Option<PathBuf>,
}

impl Paths {
    /// Вычисляет корень данных и умолчания по файлу `defaults.json` рядом с бинарником
    /// (fallback — устаревший `location.json`) **без создания каталогов** — для ранней
    /// «peek»-фазы CLI, где нужно узнать язык/корень до разбора аргументов, но `--help`
    /// не должен трогать диск (docs/history/i18n-cli.md §3.2). Язык каркаса/интерфейса:
    /// явный из `defaults.json` (`Some`) или определённый по локали ОС (`None` — свежая
    /// установка, §4.3 installers.md). Создание каталогов — отдельно [`Paths::ensure_dirs`].
    pub fn resolve() -> Result<Self> {
        let exe = std::env::current_exe().context("cannot resolve current executable path")?;
        let exe_dir = exe
            .parent()
            .context("cannot determine executable directory")?
            .to_path_buf();

        let defaults = Defaults::read(&exe_dir)?;
        let root = defaults.location.root_dir(&exe_dir)?;
        let mut paths = Self::with_root(root);
        paths.default_language = defaults
            .default_language
            .unwrap_or_else(crate::shared::i18n::detect_os_language);
        paths.exe_dir = Some(exe_dir);
        Ok(paths)
    }

    /// Создаёт каталоги данных (корень + каталог внешних локалей). Вызывается перед
    /// работой с данными (не для `--help`/`--version`). Идемпотентно. Локализованный
    /// контекст ошибки добавляет вызывающий (`main`) — здесь наружу идёт сырая io-ошибка.
    pub fn ensure_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(&self.root)?;
        // Каталог внешних локалей создаётся для обнаруживаемости (пустой каталог
        // сигналит «клади файлы сюда»); ошибку создания не эскалируем — внешние
        // локали опциональны, при их отсутствии работают вшитые бандлы.
        let _ = std::fs::create_dir_all(self.locales_dir());
        Ok(())
    }

    /// Создаёт набор путей от произвольного корня (используется в тестах). Язык
    /// каркаса новых профилей — дефолт (`ru`); каталог бинарника неизвестен (`None`).
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            default_language: Lang::default(),
            exe_dir: None,
        }
    }

    /// Корневой каталог данных.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Язык служебного каркаса новых профилей (из `defaults.json`, ось A). Bootstrap
    /// первого профиля и создание профилей берут его. См. docs/history/i18n.md.
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

    /// Каталог Hunspell-словарей (`dictionaries/`) в корне данных.
    pub fn dictionaries_dir(&self) -> PathBuf {
        self.root.join("dictionaries")
    }

    /// Резервный каталог словарей в **портативной раскладке рядом с бинарником**
    /// (`<exe_dir>/data/dictionaries`) — источник read-only ресурсов, положенных
    /// инсталлятором/пакетом, когда корень данных не портативный (`system`/`path`) и
    /// словарей в нём нет. В портативном режиме совпадает с [`dictionaries_dir`]
    /// (fallback ничего не добавляет — дубли отсеиваются по имени). `None`, когда
    /// каталог бинарника неизвестен (тесты). См. §4.2 / П1 docs/history/installers.md.
    pub fn bundled_dictionaries_dir(&self) -> Option<PathBuf> {
        self.exe_dir
            .as_ref()
            .map(|d| d.join(PORTABLE_DATA_SUBDIR).join("dictionaries"))
    }

    /// Каталог внешних локалей (`locales/`): `<code>.json` переопределяет вшитый бандл
    /// того же языка или добавляет новый язык без пересборки (ось A/B, Ярус 3 —
    /// docs/history/i18n-external-locales.md). Загружается один раз при старте.
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
        assert_eq!(d.default_language, Some(Lang::En));
    }

    #[test]
    fn defaults_missing_has_no_explicit_language() {
        // Нет файла → дефолты: портативно + язык не задан (определится по локали ОС).
        let dir = tempfile::tempdir().unwrap();
        let d = Defaults::read(dir.path()).unwrap();
        assert_eq!(d, Defaults::default());
        assert_eq!(d.location, DataLocation::Portable);
        assert_eq!(d.default_language, None);
    }

    #[test]
    fn defaults_without_language_field_is_none() {
        // Пакет пишет только режим (§4.3 installers.md) → язык None → детект по локали.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(DEFAULTS_MARKER), r#"{"mode":"system"}"#).unwrap();
        let d = Defaults::read(dir.path()).unwrap();
        assert_eq!(d.location, DataLocation::System);
        assert_eq!(d.default_language, None);
    }

    #[test]
    fn defaults_falls_back_to_legacy_location_marker() {
        // Нет defaults.json → читается старый location.json (только режим; язык None).
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(LEGACY_LOCATION_MARKER),
            r#"{"mode":"system"}"#,
        )
        .unwrap();
        let d = Defaults::read(dir.path()).unwrap();
        assert_eq!(d.location, DataLocation::System);
        assert_eq!(d.default_language, None);
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
        assert_eq!(d.default_language, Some(Lang::En));
    }

    #[test]
    fn defaults_tolerates_utf8_bom() {
        // Инсталлятор/редактор мог записать файл с BOM — не должен ронять запуск (П3).
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(br#"{"mode":"system","default_language":"en"}"#);
        std::fs::write(dir.path().join(DEFAULTS_MARKER), bytes).unwrap();
        let d = Defaults::read(dir.path()).unwrap();
        assert_eq!(d.location, DataLocation::System);
        assert_eq!(d.default_language, Some(Lang::En));
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

    #[test]
    fn with_root_has_no_bundled_dictionaries() {
        // Каталог бинарника неизвестен в тестовом конструкторе → fallback-словарей нет.
        assert_eq!(Paths::with_root("r").bundled_dictionaries_dir(), None);
    }
}

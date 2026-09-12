//! User-data location and installation defaults. Default — portable mode:
//! data lives in a `data/` subdirectory next to the executable (the
//! subdirectory separates data from build tooling files/caches, especially in
//! dev — `target/debug/data/`). A `defaults.json` marker file next to the
//! binary can switch storage to a standard OS folder or an arbitrary
//! directory **and** set the agent-scaffold language for new profiles
//! (`default_language`, axis A — docs/history/i18n.md; an installer will fill
//! it in from the user's choice at install time). For backward compatibility
//! the old `location.json` marker (storage mode only) is also read. See spec
//! §5.2 (data location) and §12.1.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::shared::i18n::{Lang, Locale};

/// Name of the installation-defaults file (always lives next to the binary, not in `data/`).
pub const DEFAULTS_MARKER: &str = "defaults.json";

/// Legacy marker-file name (storage mode only) — read for backward
/// compatibility if `defaults.json` is absent.
pub const LEGACY_LOCATION_MARKER: &str = "location.json";

/// Data subdirectory in portable mode (next to the binary).
pub const PORTABLE_DATA_SUBDIR: &str = "data";

/// The invitation file [`Paths::ensure_dirs`] leaves in a freshly created
/// `dictionaries/`. Not a dictionary: the loader reads `*.aff`/`*.dic` pairs and
/// ignores everything else (`features/spellcheck/dict.rs`).
pub const DICTIONARIES_README: &str = "README.txt";

/// Storage mode for user data, set by the `defaults.json` defaults file next
/// to the executable (the `mode` field, plus `path` for `path`). No/empty
/// marker → [`DataLocation::Portable`] (backward compatibility: existing
/// installs keep data next to the binary).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum DataLocation {
    /// Data next to the executable (a portable install).
    #[default]
    Portable,
    /// Data in the user's standard OS folder (Windows
    /// `%APPDATA%\mindfork-rs`, Linux `~/.local/share/mindfork-rs`).
    System,
    /// Data in an arbitrary user-specified directory.
    Path { path: String },
}

impl DataLocation {
    /// Root data directory for this mode. `exe_dir` — the binary's directory
    /// (in portable mode the root = `exe_dir/data`).
    pub fn root_dir(&self, exe_dir: &Path) -> Result<PathBuf> {
        match self {
            Self::Portable => Ok(exe_dir.join(PORTABLE_DATA_SUBDIR)),
            Self::System => {
                // English context: this error occurs in `Paths::resolve` before
                // the CLI language is known (docs/history/i18n-cli.md §7 — the
                // "before the language is known" boundary).
                // Deliberately the project id `mindfork-rs`, NOT the binary name
                // (`mindfork`, credits::APP_NAME): this string keys the existing
                // per-user data folder, and following the rename would strand
                // that data (docs/research/binary-rename.md §3).
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

/// Installation defaults from the `defaults.json` file next to the binary:
/// data storage mode (`mode`/`path`, flat — compatible with the old
/// `location.json`) **and** the agent-scaffold language for new profiles
/// (`default_language`, axis A — docs/history/i18n.md). Filled in by the
/// installer (or by hand). Missing fields → defaults (portable, language
/// detected from the OS locale — see [`Paths::resolve`]).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Defaults {
    /// Storage mode (flat in JSON: `mode`/`path` at the top level — byte-compatible
    /// with the previous `location.json`).
    #[serde(flatten, default)]
    pub location: DataLocation,
    /// The language the **first** profile (bootstrap) and new profiles are
    /// created in, **and** the interface language on a fresh install. Not the
    /// model's reply language. `Some(..)` — explicitly set by the installer
    /// (Windows); `None` (the field is absent — e.g. a deb/rpm package writes
    /// only `{"mode":"system"}`, installers.md §4.3) → [`Paths::resolve`]
    /// detects the language from the OS locale. See docs/history/i18n.md.
    #[serde(default)]
    pub default_language: Option<Lang>,
}

/// Drops a leading UTF-8 BOM (`EF BB BF`), if present. Installers and editors
/// (Inno's Pascal helpers, some Windows editors) may write `defaults.json`
/// with a BOM — `serde_json::from_slice` fails on it. Precedents for dropping
/// it in this project: `rag_ingest::read_text`, the LameLLaMA importer.
fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

impl Defaults {
    /// Reads installation defaults next to the binary: `defaults.json` first,
    /// falling back to the legacy `location.json` (storage mode only; language
    /// = `None`) if absent. Neither present/an empty file → defaults. A
    /// corrupt JSON is an **error** (not a silent fallback), so a typo doesn't
    /// quietly redirect to the wrong empty dataset. A leading UTF-8 BOM is
    /// dropped ([`strip_bom`]).
    pub fn read(exe_dir: &Path) -> Result<Self> {
        let primary = exe_dir.join(DEFAULTS_MARKER);
        let marker = if primary.exists() {
            primary
        } else {
            exe_dir.join(LEGACY_LOCATION_MARKER)
        };
        // English contexts: `Defaults::read` is called from `Paths::resolve`
        // before the CLI language is known (docs/history/i18n-cli.md §7 — the
        // "before the language is known" boundary).
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

/// A set of paths to application data, computed from the root directory.
///
/// In production the root is the executable's directory (portable mode); in
/// tests a temp directory is used via [`Paths::with_root`]. Also carries the
/// scaffold language of new profiles (`default_language`) from `defaults.json`.
#[derive(Debug, Clone)]
pub struct Paths {
    root: PathBuf,
    default_language: Lang,
    /// The executable's directory (on Linux — the real path: `current_exe`
    /// resolves `/proc/self/exe`, i.e. the symlink target `/usr/bin/…` →
    /// `/usr/lib/<pkg>/…`). Needed to find read-only resources placed next to
    /// the binary (dictionaries, installers.md §4.2) when the data root isn't
    /// portable (`system`/`path`). `None` in tests (`with_root`).
    exe_dir: Option<PathBuf>,
    /// A sandbox directory other than `<root>/sandbox`: a live test pointing a temporary
    /// data root at a provisioned sandbox (docs/history/sandbox-file-exchange.md §11 S13). Only
    /// [`Paths::with_sandbox_dir`], a test helper, sets it.
    sandbox_override: Option<PathBuf>,
}

impl Paths {
    /// Computes the data root and defaults from the `defaults.json` file next
    /// to the binary (fallback — the legacy `location.json`) **without
    /// creating directories** — for the CLI's early "peek" phase, where the
    /// language/root must be known before parsing arguments, but `--help`
    /// mustn't touch disk (docs/history/i18n-cli.md §3.2). Scaffold/interface
    /// language: explicit from `defaults.json` (`Some`) or detected from the
    /// OS locale (`None` — a fresh install, installers.md §4.3). Creating
    /// directories is separate — [`Paths::ensure_dirs`].
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

    /// Creates the data directories (root + the external-locales and
    /// dictionaries directories). Called before working with data (not for
    /// `--help`/`--version`). Idempotent. The localized error context is added
    /// by the caller (`main`) — here a raw io error propagates outward.
    ///
    /// `loc` is the locale the invitation file in a freshly created
    /// `dictionaries/` is written in ([`Paths::seed_dictionaries_dir`]).
    pub fn ensure_dirs(&self, loc: &Locale) -> Result<()> {
        std::fs::create_dir_all(&self.root)?;
        // The external-locales directory is created for discoverability (an
        // empty directory signals "put files here"); creation errors aren't
        // escalated — external locales are optional, built-in bundles work
        // without them.
        let _ = std::fs::create_dir_all(self.locales_dir());
        self.seed_dictionaries_dir(loc);
        Ok(())
    }

    /// Creates `dictionaries/` under the data root when it is missing, with a
    /// `README.txt` in it inviting the user to add their own Hunspell pairs.
    ///
    /// **Why it has to exist at all.** Where the shipped dictionaries land
    /// depends on the storage mode: portable puts them in this very directory,
    /// while `system`/`path` leaves them next to the binary
    /// ([`Paths::bundled_dictionaries_dir`], installers.md §4.2) — the install
    /// folder (`…\AppData\Local\Programs\mindfork-rs` on Windows), which is
    /// the right place for files the installer put there and the wrong place
    /// for the user's own, since an update or an uninstall owns it. Under a
    /// system install the data root then had **no** `dictionaries/` at all, so
    /// there was nowhere obvious to add one. The
    /// empty directory is the same discoverability argument as `locales/`
    /// above; the file in it is what a directory alone cannot say — the file
    /// naming convention, and that the shipped dictionaries stay loaded.
    ///
    /// **Only when the directory is missing entirely**, never into one that
    /// already exists: after the first launch the file is the user's — deleting
    /// it has to stick, and an edited copy must not be overwritten on the next
    /// start. Which also means a portable install (where the directory arrives
    /// with the dictionaries in it) is left exactly as it was.
    ///
    /// Failures are not escalated, for the same reason as `locales/`: the
    /// bundled dictionaries load either way, and a startup that dies over an
    /// unwritable hint would be worse than the hint being missing.
    fn seed_dictionaries_dir(&self, loc: &Locale) {
        let dir = self.dictionaries_dir();
        if dir.exists() || std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let text = loc.t("dictionaries.readme");
        // Platform line endings: this file exists to be opened in whatever
        // editor the OS puts in front of the user, and on Windows that is still
        // Notepad's world. The bundles hold plain `\n`.
        #[cfg(windows)]
        let text = text.replace('\n', "\r\n");
        let _ = std::fs::write(dir.join(DICTIONARIES_README), text.as_bytes());
    }

    /// Creates a path set from an arbitrary root (used in tests). The
    /// scaffold language of new profiles is the default (`ru`); the binary's
    /// directory is unknown (`None`).
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            default_language: Lang::default(),
            exe_dir: None,
            sandbox_override: None,
        }
    }

    /// Overrides the default language (tests only). [`Paths::with_root`] uses
    /// `Lang::default()`, which is the same value `AppConfig` deserializes to —
    /// so a test asserting that a language was *seeded* from here would pass
    /// even with the seeding removed. This makes such a test able to fail.
    #[cfg(test)]
    pub fn with_default_language(mut self, lang: Lang) -> Self {
        self.default_language = lang;
        self
    }

    /// Points the sandbox at `dir` rather than `<root>/sandbox` (tests only): a live smoke
    /// runs the real registry over a temporary data root and a provisioned sandbox —
    /// copying one is 250 MB, and reading an environment variable inside the registry
    /// would be test plumbing in production code.
    #[cfg(test)]
    pub fn with_sandbox_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.sandbox_override = Some(dir.into());
        self
    }

    /// Root data directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The agent-scaffold language for new profiles (from `defaults.json`,
    /// axis A). Bootstrap of the first profile and profile creation take it.
    /// See docs/history/i18n.md.
    pub fn default_language(&self) -> Lang {
        self.default_language
    }

    /// Global configuration (`settings.json`).
    pub fn settings_file(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    /// Profile list (`profiles.json`).
    pub fn profiles_file(&self) -> PathBuf {
        self.root.join("profiles.json")
    }

    /// Directory with chat files (`chats/`).
    pub fn chats_dir(&self) -> PathBuf {
        self.root.join("chats")
    }

    /// A specific chat's file (`chats/{id}.json`).
    pub fn chat_file(&self, chat_id: &str) -> PathBuf {
        self.chats_dir().join(format!("{chat_id}.json"))
    }

    /// Notes and RAG database (`data.db`).
    pub fn data_db(&self) -> PathBuf {
        self.root.join("data.db")
    }

    /// Change journals of chats' code workspaces (`workspace/<chat-id>/`,
    /// spec §9.12). Under the app's data root rather than inside the user's
    /// project: bookkeeping scattered through a checkout would show up in their
    /// own `git status`. Backed up with the rest of the data root — a baseline
    /// is the one thing this track stores that cannot be recomputed.
    pub fn workspace_dir(&self) -> PathBuf {
        self.root.join("workspace")
    }

    /// Files stored with chats (`files/<chat-id>/`): what `python_exec` saved to
    /// `/w/out`, kept for the user (docs/history/sandbox-file-exchange.md, spec §9.7). User data
    /// that cannot be recomputed, so `features::backup` packs it.
    pub fn files_dir(&self) -> PathBuf {
        self.root.join("files")
    }

    /// The disposable search cache (`cache.db`) — a full-text index over chat
    /// content. Derived data: deleting it is a supported repair, and
    /// `features/backup.rs` leaves it out of archives by construction (its
    /// include list is an allowlist). See
    /// docs/research/chat-content-search.md §2.
    pub fn cache_db(&self) -> PathBuf {
        self.root.join("cache.db")
    }

    /// Hunspell dictionaries directory (`dictionaries/`) under the data root.
    pub fn dictionaries_dir(&self) -> PathBuf {
        self.root.join("dictionaries")
    }

    /// Fallback dictionaries directory in the **portable layout next to the
    /// binary** (`<exe_dir>/data/dictionaries`) — a source of read-only
    /// resources placed by the installer/package when the data root isn't
    /// portable (`system`/`path`) and it has no dictionaries. In portable mode
    /// coincides with [`dictionaries_dir`] (the fallback adds nothing — dupes
    /// are filtered by name). `None` when the binary's directory is unknown
    /// (tests). See installers.md §4.2 / P1.
    pub fn bundled_dictionaries_dir(&self) -> Option<PathBuf> {
        self.exe_dir
            .as_ref()
            .map(|d| d.join(PORTABLE_DATA_SUBDIR).join("dictionaries"))
    }

    /// The application binary's own directory. `None` when it is unknown
    /// (`with_root`, i.e. tests). Used to look for read-only resources placed
    /// beside the executable — the dictionaries above, and a `llama-server`
    /// unpacked next to the app (spec §3.4).
    pub fn exe_dir(&self) -> Option<&Path> {
        self.exe_dir.as_deref()
    }

    /// External-locales directory (`locales/`): `<code>.json` overrides the
    /// built-in bundle of the same language or adds a new language without
    /// rebuilding (axis A/B, Tier 3 — docs/history/i18n-external-locales.md).
    /// Loaded once at startup.
    pub fn locales_dir(&self) -> PathBuf {
        self.root.join("locales")
    }

    /// Spellcheck personal dictionary (`personal_dictionary.txt`).
    pub fn personal_dictionary(&self) -> PathBuf {
        self.root.join("personal_dictionary.txt")
    }

    /// Log directory (`logs/`).
    pub fn log_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// Backups directory (`backups/`).
    pub fn backups_dir(&self) -> PathBuf {
        self.root.join("backups")
    }

    /// Python sandbox directory (`sandbox/`): the `wasmer` binary,
    /// `python.webc`, `site-packages/`. Populated by `mindfork sandbox setup`
    /// (Phase 2).
    pub fn sandbox_dir(&self) -> PathBuf {
        self.sandbox_override
            .clone()
            .unwrap_or_else(|| self.root.join("sandbox"))
    }

    /// Downloaded llama.cpp builds (`llama/`), one directory per install named
    /// `<backend>-<tag>` (`cuda-12.4-b10883`). Populated by `mindfork llama
    /// setup` — see [docs/research/llama-cpp-download.md](../../docs/research/llama-cpp-download.md)
    /// §4.3 and spec §3.4.
    ///
    /// Like `sandbox/`, it holds re-downloadable assets rather than user data:
    /// `features::backup` works off an allow-list, so this directory is neither
    /// packed into an archive nor removed by a restore.
    pub fn llama_dir(&self) -> PathBuf {
        self.root.join("llama")
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
    fn cache_db_under_root_next_to_data_db() {
        // The search index is a *second* database at the root, not a sibling of
        // `data.db` inside it — that separation is what lets it be deleted (and
        // excluded from backups) without touching user content (research §2).
        let p = Paths::with_root("data-root");
        assert!(p.cache_db().ends_with("cache.db"));
        assert_eq!(p.cache_db().parent(), Some(p.root()));
        assert_ne!(p.cache_db(), p.data_db());
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
        // Portable mode: root = the `data/` subdirectory next to the binary.
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
        // The path is trimmed; the binary's directory is ignored.
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
        // We don't assert the exact path (OS/user-dependent), but it must
        // point to the application's directory.
        let root = DataLocation::System.root_dir(Path::new("/exe")).unwrap();
        assert!(root.to_string_lossy().contains("mindfork-rs"));
    }

    #[test]
    fn defaults_reads_storage_and_language() {
        // defaults.json carries the storage mode (flat) + the scaffold language.
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
        // No file → defaults: portable + language unset (detected from the OS locale).
        let dir = tempfile::tempdir().unwrap();
        let d = Defaults::read(dir.path()).unwrap();
        assert_eq!(d, Defaults::default());
        assert_eq!(d.location, DataLocation::Portable);
        assert_eq!(d.default_language, None);
    }

    #[test]
    fn defaults_without_language_field_is_none() {
        // A package writes only the mode (installers.md §4.3) → language None → detected from the locale.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(DEFAULTS_MARKER), r#"{"mode":"system"}"#).unwrap();
        let d = Defaults::read(dir.path()).unwrap();
        assert_eq!(d.location, DataLocation::System);
        assert_eq!(d.default_language, None);
    }

    #[test]
    fn defaults_falls_back_to_legacy_location_marker() {
        // No defaults.json → the old location.json is read (mode only; language None).
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
        // An installer/editor might write the file with a BOM — must not crash startup (P3).
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
        // The binary's directory is unknown in the test constructor → no fallback dictionaries.
        assert_eq!(Paths::with_root("r").bundled_dictionaries_dir(), None);
    }

    /// A `Paths` over a not-yet-existing root inside a temp directory — what a
    /// first launch under `system`/`path` storage actually sees.
    fn fresh_root(dir: &tempfile::TempDir) -> Paths {
        Paths::with_root(dir.path().join("data"))
    }

    #[test]
    fn ensure_dirs_creates_a_dictionaries_directory_with_an_invitation() {
        // Under a system install the shipped dictionaries live next to the
        // binary, so without this the data root had no `dictionaries/` at all
        // and the user had nowhere to put their own.
        let dir = tempfile::tempdir().unwrap();
        let paths = fresh_root(&dir);
        paths
            .ensure_dirs(crate::shared::i18n::locale(Lang::En))
            .unwrap();

        let readme = paths.dictionaries_dir().join(DICTIONARIES_README);
        let text = std::fs::read_to_string(&readme).unwrap();
        // The invitation names the convention the loader actually reads
        // (`features/spellcheck/dict.rs`): a `*.aff` + `*.dic` pair.
        assert!(text.contains(".aff") && text.contains(".dic"), "{text}");
    }

    #[test]
    fn the_invitation_is_written_in_the_given_language() {
        // The language is the caller's (`main`: the settings language, or
        // `default_language` from defaults.json on a fresh install) — the file
        // is the first thing an installed build says about dictionaries.
        let mut texts = Vec::new();
        for lang in [Lang::En, Lang::Ru] {
            let dir = tempfile::tempdir().unwrap();
            let paths = fresh_root(&dir);
            paths
                .ensure_dirs(crate::shared::i18n::locale(lang))
                .unwrap();
            texts.push(
                std::fs::read_to_string(paths.dictionaries_dir().join(DICTIONARIES_README))
                    .unwrap(),
            );
        }
        assert_ne!(texts[0], texts[1]);
        assert!(!texts[0].chars().any(|c| ('а'..='я').contains(&c)));
        assert!(texts[1].chars().any(|c| ('а'..='я').contains(&c)));
    }

    #[test]
    fn an_existing_dictionaries_directory_is_left_alone() {
        // A portable install arrives with the directory already full; writing
        // into it would drop a file next to dictionaries that are not ours.
        let dir = tempfile::tempdir().unwrap();
        let paths = fresh_root(&dir);
        std::fs::create_dir_all(paths.dictionaries_dir()).unwrap();
        paths
            .ensure_dirs(crate::shared::i18n::locale(Lang::En))
            .unwrap();
        assert!(!paths.dictionaries_dir().join(DICTIONARIES_README).exists());
    }

    #[test]
    fn a_deleted_invitation_stays_deleted() {
        // After the first launch the file is the user's: the directory exists,
        // so no later start writes into it again.
        let dir = tempfile::tempdir().unwrap();
        let paths = fresh_root(&dir);
        let loc = crate::shared::i18n::locale(Lang::En);
        paths.ensure_dirs(loc).unwrap();
        let readme = paths.dictionaries_dir().join(DICTIONARIES_README);
        std::fs::remove_file(&readme).unwrap();

        paths.ensure_dirs(loc).unwrap();
        assert!(!readme.exists());
    }
}

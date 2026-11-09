//! Backup and restore of user data into a zip archive.
//!
//! Launched via command-line arguments (`--backup` / `--restore`) with no TUI,
//! and ends the process (see `main.rs`). Archive contents (paths in the archive
//! are relative to the data root):
//! - files `settings.json`, `profiles.json`, `data.db` (+ sidecar `-wal`/`-shm`,
//!   if present — see the compaction note below), `personal_dictionary.txt`;
//! - directories `chats/`, `dictionaries/`, `locales/`, `files/` and `workspace/`
//!   (recursively — their `*.bak` files are pulled in too; `locales/` — user overrides
//!   of the scaffold/UI text; `files/` — what the Python sandbox saved for each chat,
//!   which cannot be recomputed, docs/history/sandbox-file-exchange.md F10;
//!   `workspace/` — the change journals of chats' code workspaces, spec §9.12: the
//!   **pre-image** of every file the assistant edited, which exists nowhere else once
//!   the file is overwritten, so a restore without them can show a change but not put
//!   it back); `stories/` — each chat's Fabula stories, `stories/<chat-id>/<story-id>.db`
//!   (docs/history/fabula.md §6 T5), packed as they lie: every read and write of one goes
//!   through its database actor and the default rollback journal, and a backup holds
//!   the single-instance lock, so no story is mid-write while it is packed;
//! - all `*.bak` at the root (`settings.bak`, `profiles.bak`);
//! - the file-tools "sandbox" directory (`config.tools.fs_root`) — **only if**
//!   it lies inside the data root.
//!
//! Excluded: `backups/`, `logs/`, `story-logs/` (each story's call log, kept for
//! replay and outside backups by the owner's decision, docs/research/fabula-engine.md
//! §15: a restore neither packs nor clears it), and the install-defaults files
//! `defaults.json`/`location.json` (they're about the install, not user data). Additionally, a
//! `manifest.json` (schema versions + app version) is written into the archive —
//! metadata for warning on restoring data newer than the running build reads; it
//! is **not** extracted into the root. Its schemas are those of the files packed,
//! not the build's — a pre-migration backup holds them before the step. See
//! [`BackupManifest`].
//!
//! **The database is compacted** on both paths (spec §12.3): a backup packs a
//! `VACUUM INTO` copy of `data.db` instead of the live file (free pages left by
//! deleted notes/RAG chunks are dropped, and the `-wal`/`-shm` sidecars are
//! folded in, so they aren't packed), and a restore compacts what it unpacked —
//! which is what an archive made before this existed, or by another tool, needs.
//! Both are **best effort**: if the file can't be compacted (corrupt, or not a
//! database at all) the raw file is packed / left as unpacked, because a backup
//! that happens is worth more than a compact one. Details — [`db::vacuum_into`].
//!
//! **Packing and unpacking run on a pool of threads** ([`Packing`], spec §12.3,
//! docs/research/backup-speed.md): a worker compresses — and encrypts — one entry
//! whole, into a zip of its own, and the writer appends it raw
//! (`raw_copy_file`), in the order the entries finish; unpacking gives every
//! worker a handle on the archive of its own. The archive is the standard zip a
//! single thread wrote — any reader, any older build — only its entries' order
//! differs. The database's compaction is the pool's first job, so the chats are
//! packed while it runs. How many workers — `backup.threads`, never more than
//! the cores; at which level — `backup.compression` unless `-c` says otherwise,
//! the copies the app makes itself included. See [`parallel`].
//!
//! **Restore is transactional.** The archive is validated first (before any
//! destructive action). If the root already has data, it is automatically
//! saved into `backups/` (a pre-restore copy), and only then is the root
//! cleared and the given archive unpacked. If unpacking fails and a
//! pre-restore copy was created, a rollback to it is performed. See spec §12.3.
//!
//! **The archive can be password-protected** (spec §12.3): every data entry is
//! encrypted with WinZip AES-256, so a backup that leaves the machine is useless
//! without the password. `manifest.json` is deliberately left **unencrypted** —
//! it holds no user data, and keeping it readable lets the "this backup is from a
//! newer version" warning work without a password. Two consequences worth
//! knowing, both measured (docs/history/backup-password.md §1):
//!
//! * a password handed to an **unencrypted** archive is discarded by the zip
//!   layer, so restoring either kind needs no detection branch;
//! * the password is verified when an entry is **opened**, not after reading it,
//!   so [`validate_archive`] rejects a wrong password *before* the destructive
//!   phase — the transactional guarantee above survives.
//!
//! What this does **not** protect: entry names, sizes and the directory
//! structure are visible without the password (ZIP AES encrypts content only),
//! and the key derivation is fixed by the format at PBKDF2-HMAC-SHA1/1000, which
//! is weak against offline brute force of a short password — hence the settings
//! hint asking for a passphrase. See docs/history/backup-password.md §2.
//!
//! **Both paths report progress** through a `progress` callback of already
//! localized lines (the `sandbox_setup::setup` shape): packing a real data root
//! takes seconds, and a CLI that prints nothing until it is done is
//! indistinguishable from one that has hung — the more so right after the
//! password prompt, where the echo-less input leaves the user unsure it was
//! taken at all. The `features` layer has no TUI, so the CLI command decides
//! where the lines go (`println!`) and a non-interactive caller passes `|_| {}`.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};
use chrono::Local;
use serde::{Deserialize, Serialize};
use zip::ZipArchive;

use crate::shared::config::{BackupSettings, MAX_BACKUP_COMPRESSION};
use crate::shared::i18n::Locale;
use crate::shared::paths::Paths;
use crate::shared::storage::db;
use crate::shared::storage::schema::{
    self, CHAT_SCHEMA, DB_SCHEMA, PROFILES_SCHEMA, SETTINGS_SCHEMA, STORY_SCHEMA,
};

/// Manifest file name inside the archive (schema-version metadata; not
/// extracted into the root — read separately by [`read_manifest`]). See
/// release-engineering.md §3.4.
const MANIFEST_NAME: &str = "manifest.json";

mod dates;
mod parallel;

pub(crate) use dates::dated;

/// The data schema of each kind of stored file in an archive (ADR 0006 §11):
/// what the archive **holds**, read from its files as they are packed
/// ([`SchemaVersions::held_by`]). Not the writing build's: a backup can be
/// taken of files no start has migrated yet, the pre-migration backup above
/// all — the copy to take back to the release before the step, which the
/// build's schemas had warn that its own data came from a newer version
/// (spec §12.3). A kind with no file is `0` ([`Default`]): nothing in the
/// archive asks a reader for it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaVersions {
    pub settings: u32,
    pub profiles: u32,
    pub chat: u32,
    pub db: u32,
    /// The newest of `stories/*/*.db`. `default`: a manifest written before
    /// stories reads as holding none, which is what it holds.
    #[serde(default)]
    pub story: u32,
}

impl SchemaVersions {
    /// This build's schemas.
    fn current() -> Self {
        Self {
            settings: SETTINGS_SCHEMA,
            profiles: PROFILES_SCHEMA,
            chat: CHAT_SCHEMA,
            db: DB_SCHEMA,
            story: STORY_SCHEMA,
        }
    }

    /// The schemas of the files a backup of `paths` packs, read the way the
    /// start-up reads them (`schema::*_artifact().detect`): `settings.json`,
    /// `profiles.json`, the **newest** of `chats/*.json` — a reader older than
    /// one chat refuses that one file — and the newest `user_version` among the
    /// stories, read from the header (never opened through SQLite — see
    /// [`db::header_user_version`]). A file that is missing or does not parse
    /// counts as none: `0`. Never fails — a backup that happens is worth more
    /// than an exact manifest.
    ///
    /// `db` is left `0`: it is the `user_version` of the database file that goes
    /// into the archive — the compacted copy, else the live file — known once
    /// the pool has packed it ([`parallel`] fills it in).
    fn held_by(paths: &Paths) -> Self {
        let chat = schema::chat_artifact();
        let chats = fs::read_dir(paths.chats_dir())
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension() == Some(OsStr::new("json")))
            .map(|path| detected(&path, &chat))
            .max()
            .unwrap_or(0);
        Self {
            settings: detected(&paths.settings_file(), &schema::settings_artifact()),
            profiles: detected(&paths.profiles_file(), &schema::profiles_artifact()),
            chat: chats,
            db: 0,
            story: story_files(&paths.stories_dir())
                .iter()
                .filter_map(|path| db::header_user_version(path))
                .max()
                .unwrap_or(0),
        }
    }
}

/// The story databases under `dir`: `<chat-id>/<story-id>.db`, the layout
/// `Paths::story_db` names. Whatever else lies there is packed, not read.
pub(crate) fn story_files(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|chat| chat.path().is_dir())
        .flat_map(|chat| fs::read_dir(chat.path()).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension() == Some(OsStr::new("db")))
        .collect();
    found.sort();
    found
}

/// The schema `art` detects in the JSON file at `path`; `0` — no file, or not
/// JSON.
fn detected(path: &Path, art: &schema::JsonArtifact) -> u32 {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .map_or(0, |value| (art.detect)(&value))
}

/// One kind of file an archive holds at a schema newer than a reader's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NewerSchema {
    /// The file as a person finds it in the data root (`chats/*.json`).
    file: &'static str,
    /// Its schema in the archive.
    found: u32,
    /// The newest schema the reader reads.
    supported: u32,
}

/// Backup manifest (`manifest.json` in the archive): the version of the app
/// that wrote the archive, the schemas of what it holds ([`SchemaVersions`]),
/// and the creation time. Needed so that restoring data **newer** than this
/// build reads gets a warning (data is intact; the startup downgrade guard
/// protects it regardless — ADR 0006).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupManifest {
    pub app_version: String,
    pub schemas: SchemaVersions,
    pub created_at: String,
}

impl BackupManifest {
    /// A manifest this build writes now, for an archive whose files are at
    /// `schemas`.
    fn written_now(schemas: SchemaVersions) -> Self {
        Self {
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            schemas,
            created_at: Local::now().to_rfc3339(),
        }
    }

    /// The kinds of file the archive holds at a schema newer than `reader`'s,
    /// in the order the start-up reads them. Empty — the reader reads it all.
    fn newer_than(&self, reader: &SchemaVersions) -> Vec<NewerSchema> {
        let (a, r) = (&self.schemas, reader);
        [
            ("settings.json", a.settings, r.settings),
            ("profiles.json", a.profiles, r.profiles),
            ("chats/*.json", a.chat, r.chat),
            ("data.db", a.db, r.db),
            ("stories/*/*.db", a.story, r.story),
        ]
        .into_iter()
        .filter(|(_, found, supported)| found > supported)
        .map(|(file, found, supported)| NewerSchema {
            file,
            found,
            supported,
        })
        .collect()
    }

    /// The kinds of file the archive holds at a schema newer than this build
    /// reads.
    fn newer_schemas(&self) -> Vec<NewerSchema> {
        self.newer_than(&SchemaVersions::current())
    }

    /// Does the archive hold data at a schema **newer** than this build reads —
    /// a signal to warn on restore.
    pub fn is_newer_than_current(&self) -> bool {
        !self.newer_schemas().is_empty()
    }

    /// The warning `restore` prints before it starts, when the archive holds
    /// data newer than this build reads: which files, at which schema, and the
    /// version that wrote the archive. `None` — nothing to warn about.
    pub fn restore_warning(&self, loc: &Locale) -> Option<String> {
        let newer = self.newer_schemas();
        if newer.is_empty() {
            return None;
        }
        let schemas = newer
            .iter()
            .map(|n| {
                loc.tf(
                    "backup.warn.newer_schema",
                    &[
                        ("file", n.file),
                        ("found", &n.found.to_string()),
                        ("supported", &n.supported.to_string()),
                    ],
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        Some(loc.tf(
            "backup.warn.newer_manifest",
            &[("schemas", &schemas), ("version", &self.app_version)],
        ))
    }
}

/// Reads `manifest.json` from the archive. `None` — an old backup with no
/// manifest (created before stage 5). Read/parse errors aren't fatal for
/// restore (the caller swallows them).
pub fn read_manifest(archive: &Path) -> Result<Option<BackupManifest>> {
    let file = File::open(archive)?;
    let mut zip = ZipArchive::new(file)?;
    match zip.by_name(MANIFEST_NAME) {
        Ok(mut entry) => {
            let mut buf = String::new();
            io::Read::read_to_string(&mut entry, &mut buf)?;
            Ok(Some(serde_json::from_str(&buf)?))
        }
        Err(zip::result::ZipError::FileNotFound) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// Top-level files included in the backup (missing ones are skipped).
const TOP_FILES: &[&str] = &["settings.json", "profiles.json", "personal_dictionary.txt"];

/// The database and its sidecars. Listed apart from [`TOP_FILES`] because they
/// are packed as a **single compacted copy** when `VACUUM INTO` succeeds (which
/// folds the sidecars in) and raw only as a fallback — see [`compacted_db`].
/// Clearing on restore always covers all three.
const DB_FILES: &[&str] = &["data.db", "data.db-wal", "data.db-shm"];

/// Directories included in the backup whole (recursively).
///
/// One list for both halves — what a backup packs and what a restore clears — so a
/// directory cannot be packed and then survive a restore, or be cleared and not come back.
/// `story-logs` is in neither half, by design (the module doc).
const TOP_DIRS: &[&str] = &[
    "chats",
    "dictionaries",
    "locales",
    "themes",
    "files",
    "workspace",
    "stories",
];

/// A packing entry: the source's absolute path, its name inside the archive (with
/// `/`), its size — which the pool schedules by, largest first, and which
/// decides whether its compressed copy waits in memory or in a scratch file —
/// and its modification time, which the entry carries ([`dates`]; `None` — not
/// known, and the entry goes undated).
struct Entry {
    abs: PathBuf,
    name: String,
    len: u64,
    mtime: Option<SystemTime>,
}

impl Entry {
    /// The file at `abs`, as `name`, with its size and date read from `meta`.
    fn of(abs: PathBuf, name: String, meta: Option<&fs::Metadata>) -> Self {
        Self {
            abs,
            name,
            len: meta.map_or(0, fs::Metadata::len),
            mtime: meta.and_then(|m| m.modified().ok()),
        }
    }
}

/// How an archive is packed and unpacked: the deflate level and how many
/// workers do it (spec §12.3, docs/research/backup-speed.md F2–F4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packing {
    /// `0..=9`, `0` — stored; out of range is clamped where it is used.
    pub level: i64,
    /// Worker threads, at least one. [`Packing::new`] caps the setting at the
    /// cores; the field is public for a test that needs a pool larger than the
    /// machine it runs on.
    pub threads: usize,
}

impl Packing {
    /// `level` on the workers the setting `threads` amounts to here
    /// ([`worker_count`]).
    pub fn new(level: i64, threads: u32) -> Self {
        Self {
            level,
            threads: worker_count(threads),
        }
    }

    /// What the settings say: `backup.compression` on `backup.threads`.
    pub fn from_settings(settings: &BackupSettings) -> Self {
        Self::new(i64::from(settings.compression), settings.threads)
    }

    /// `level` on every core — the tests' shorthand.
    #[cfg(test)]
    pub fn at(level: i64) -> Self {
        Self::new(level, 0)
    }
}

impl Default for Packing {
    /// The settings' defaults: level 6 on every core.
    fn default() -> Self {
        Self::from_settings(&BackupSettings::default())
    }
}

/// The cores this process may run on, as the OS reports them; `1` when it
/// cannot say.
pub fn machine_cores() -> usize {
    std::thread::available_parallelism().map_or(1, NonZeroUsize::get)
}

/// The workers the setting `threads` amounts to on this machine: `0` — every
/// core, any other number — itself, but never more than the cores (the user's
/// decision, docs/research/backup-speed.md F4), and at least one.
pub fn worker_count(threads: u32) -> usize {
    let cores = machine_cores();
    match usize::try_from(threads) {
        Ok(0) | Err(_) => cores,
        Ok(n) => n.min(cores),
    }
}

/// Restore outcome — what actually happened (for user-facing messages).
pub enum RestoreOutcome {
    /// The archive was unpacked successfully. `pre_restore` — the path of the
    /// prior data's auto-copy, if one existed and was saved.
    Restored { pre_restore: Option<PathBuf> },
    /// Unpacking failed, but the prior data was restored from the pre-restore copy.
    RolledBack {
        pre_restore: PathBuf,
        restore_error: anyhow::Error,
    },
    /// Unpacking failed and so did the rollback (or there was nothing to roll
    /// back to). The user needs to intervene manually (`pre_restore` — where
    /// the copy lives).
    Failed {
        pre_restore: Option<PathBuf>,
        restore_error: anyhow::Error,
        rollback_error: Option<anyhow::Error>,
    },
}

/// Whether an archive's encryption matches the password we hold — the question
/// asked *before* anything destructive happens (and the one the CLI's password
/// prompt loops on). See [`check_password`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchivePassword {
    /// The archive isn't encrypted (any password we were given is irrelevant).
    NotNeeded,
    /// Encrypted, and the password opens it.
    Ok,
    /// Encrypted, and we have no password.
    Required,
    /// Encrypted, and the password we have is wrong.
    Wrong,
}

/// Normalizes a password: an empty string means "no password" everywhere, so
/// clearing the setting returns to plain archives (docs/history/backup-password.md §4 F8).
fn normalize(password: Option<&str>) -> Option<&str> {
    password.filter(|p| !p.is_empty())
}

/// Reports whether `password` opens `archive`, without unpacking anything.
///
/// Cheap: the AES layer validates the password when the entry is *opened* (a
/// 2-byte verifier in its header), so this reads no content. `Err` only for an
/// archive that can't be opened as a zip at all.
pub fn check_password(archive: &Path, password: Option<&str>) -> Result<ArchivePassword> {
    let password = normalize(password);
    let mut zip = ZipArchive::new(File::open(archive)?)?;
    // Which entries are encrypted — read first, so the immutable metadata borrow
    // ends before the mutable decrypt attempt below.
    let encrypted: Vec<usize> = (0..zip.len())
        .filter(|&i| zip.by_index_raw(i).is_ok_and(|e| e.encrypted()))
        .collect();
    let Some(&first) = encrypted.first() else {
        return Ok(ArchivePassword::NotNeeded);
    };
    let Some(password) = password else {
        return Ok(ArchivePassword::Required);
    };
    // One entry is enough: every entry of one of our archives carries the same
    // password, and a mixed foreign archive fails later with a clear error.
    match zip.by_index_decrypt(first, password.as_bytes()) {
        Ok(_) => Ok(ArchivePassword::Ok),
        Err(_) => Ok(ArchivePassword::Wrong),
    }
}

/// One file entry of an archive, as [`ArchiveReader::entries`] lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// The path inside the archive, `/`-separated, relative to the data root.
    pub name: String,
    /// The uncompressed size in bytes.
    pub size: u64,
}

/// A backup archive opened to be **read where it lies** — for `mindfork stats
/// <archive>` (docs/history/data-stats.md F5), which summarizes a backup without
/// restoring it. Nothing is unpacked: an entry is handed to the caller as a
/// stream, so no decrypted byte of an encrypted archive reaches the disk.
///
/// [`ArchiveReader::open`] asks the same two questions [`restore_backup`] asks
/// before it destroys anything — is this a zip, does the password open it —
/// through the same [`check_password`], and answers with the same messages.
pub struct ArchiveReader {
    zip: ZipArchive<File>,
    password: Option<String>,
}

impl ArchiveReader {
    /// Opens `archive`. `Err` — a missing file, not a zip, or an encrypted
    /// archive with no password or the wrong one (already localized).
    pub fn open(archive: &Path, password: Option<&str>, loc: &Locale) -> Result<Self> {
        let file = File::open(archive).with_context(|| {
            loc.tf(
                "backup.ctx.open_archive",
                &[("path", &archive.display().to_string())],
            )
        })?;
        let zip = ZipArchive::new(file).with_context(|| loc.t("backup.ctx.corrupt").to_string())?;
        match check_password(archive, password)
            .with_context(|| loc.t("backup.ctx.corrupt").to_string())?
        {
            ArchivePassword::NotNeeded | ArchivePassword::Ok => {}
            ArchivePassword::Required => bail!("{}", loc.t("backup.err.password_required")),
            ArchivePassword::Wrong => bail!("{}", loc.t("backup.err.wrong_password")),
        }
        Ok(Self {
            zip,
            password: normalize(password).map(str::to_string),
        })
    }

    /// Every file entry (directories left out). Names and sizes are readable
    /// without the password — ZIP AES encrypts content only.
    pub fn entries(&mut self) -> Vec<ArchiveEntry> {
        (0..self.zip.len())
            .filter_map(|i| {
                let entry = self.zip.by_index_raw(i).ok()?;
                (!entry.is_dir()).then(|| ArchiveEntry {
                    name: entry_name(&entry),
                    size: entry.size(),
                })
            })
            .collect()
    }

    /// Hands the entry called `name` to `read` as a stream, with its
    /// uncompressed size. `Ok(None)` — the archive has no such entry. A
    /// password given for an unencrypted entry (`manifest.json`, or all of a
    /// plain archive) is discarded by the zip layer, so one path reads both.
    pub fn with_entry<T>(
        &mut self,
        name: &str,
        read: impl FnOnce(&mut dyn io::Read, u64) -> Result<T>,
    ) -> Result<Option<T>> {
        let opened = match &self.password {
            Some(pw) => self.zip.by_name_decrypt(name, pw.as_bytes()),
            None => self.zip.by_name(name),
        };
        match opened {
            Ok(mut entry) => {
                let size = entry.size();
                read(&mut entry, size).map(Some)
            }
            Err(zip::result::ZipError::FileNotFound) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }
}

/// How often the entry-by-entry counter is reported while packing/unpacking.
///
/// Long enough that a small data root finishes in silence (the ticker only
/// speaks once the loop has been running this long), short enough that a slow
/// one keeps moving on screen.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(500);

/// The "N / M entries" counter of a packing/unpacking loop, rate-limited.
///
/// A phase label alone still leaves seconds of silence on a real data root
/// (~300 chat files here), so the loops count themselves out loud. It is
/// deliberately *not* a byte counter: the honest one would have to reach inside
/// the copy of a single large file (`data.db`), and the entry count is what
/// answers "is it moving?" for the bulk of the time.
struct EntryProgress {
    total: usize,
    done: usize,
    last: Instant,
    interval: Duration,
}

impl EntryProgress {
    fn new(total: usize) -> Self {
        Self::every(total, PROGRESS_INTERVAL)
    }

    /// Same, with an explicit interval — the seam the tests use instead of
    /// sleeping.
    fn every(total: usize, interval: Duration) -> Self {
        Self {
            total,
            done: 0,
            // Counted from the start, so a fast loop never reports at all.
            last: Instant::now(),
            interval,
        }
    }

    /// Counts one entry and reports it, at most once per interval.
    fn tick(&mut self, loc: &Locale, progress: &mut impl FnMut(&str)) {
        self.done += 1;
        if self.last.elapsed() < self.interval {
            return;
        }
        self.last = Instant::now();
        progress(&loc.tf(
            "backup.progress.entries",
            &[
                ("done", &self.done.to_string()),
                ("total", &self.total.to_string()),
            ],
        ));
    }
}

/// Creates a backup of user data.
///
/// `output` — path to the archive to create (`None` → an auto-name in
/// `backups/`). `packing` — the compression level `0..=9` (`0` → no
/// compression, store) and the worker count. `fs_root` — the file-tool sandbox directory from the config
/// (included only when it lies inside the data root). `password` — encrypts
/// every data entry with AES-256 (`None`/empty → a plain archive). Returns the
/// path to the created archive. `progress` receives localized lines for a
/// caller that displays them (`|_| {}` to stay silent).
///
/// The manifest records the schemas of the files packed
/// ([`SchemaVersions::held_by`]), not this build's: the data root may hold
/// files no start has migrated yet — a pre-migration backup, a `backup` run by
/// a new version before its first start, the pre-restore copy of a root
/// restored and not yet opened.
pub fn create_backup(
    paths: &Paths,
    output: Option<PathBuf>,
    packing: Packing,
    fs_root: Option<&Path>,
    password: Option<&str>,
    loc: &Locale,
    mut progress: impl FnMut(&str),
) -> Result<PathBuf> {
    let out_path = match output {
        Some(p) => p,
        None => default_backup_path(paths, "mindfork-backup"),
    };
    // A compacted copy is packed in place of the live database (see the module
    // doc) — the pool's first job, which runs while the other entries are
    // packed. The two lines keep their order: the compaction is announced as it
    // starts, and the packing starts with it.
    if paths.data_db().is_file() {
        progress(loc.t("backup.progress.compacting"));
    }
    let entries = gather_entries(paths, fs_root, loc)?;
    let spec = ZipSpec {
        level: packing.level,
        password,
        threads: packing.threads,
        spill_over: parallel::SPILL_OVER,
    };
    progress(loc.t("backup.progress.packing"));
    parallel::write_archive(paths, &out_path, &entries, &spec, loc, &mut progress).with_context(
        || {
            loc.tf(
                "backup.ctx.create_archive",
                &[("path", &out_path.display().to_string())],
            )
        },
    )?;
    Ok(out_path)
}

/// How an archive is written: the compression level `0..=9`, the password that
/// encrypts its data entries (`None`/empty — none), how many workers pack it,
/// and the size from which an entry's compressed copy waits in a scratch file
/// rather than in memory (a seam: the tests lower it to spill every entry).
struct ZipSpec<'a> {
    level: i64,
    password: Option<&'a str>,
    threads: usize,
    spill_over: u64,
}

/// Restores data from archive `archive`, replacing the current data.
///
/// Returns `Err` only for an error **before** any destructive action (no
/// file, a corrupted/unsafe archive, a missing or wrong password). Once the
/// replacement has started it always returns `Ok(RestoreOutcome)` describing
/// the outcome (including a rollback). `fs_root` — the current sandbox (cleared
/// if inside the root).
///
/// `password` is the run's **one effective password**
/// (docs/history/backup-password.md §4 F3): it both opens `archive` and encrypts the
/// pre-restore copy, so the copy is never weaker than what the user asked for.
/// `packing` is the settings': the pre-restore copy is made at their level
/// (docs/research/backup-speed.md F3) and unpacking runs on their workers.
pub fn restore_backup(
    paths: &Paths,
    archive: &Path,
    packing: Packing,
    fs_root: Option<&Path>,
    password: Option<&str>,
    loc: &Locale,
    mut progress: impl FnMut(&str),
) -> Result<RestoreOutcome> {
    let password = normalize(password);
    // 1. Validate the archive before any destructive action.
    progress(loc.t("backup.progress.checking"));
    validate_archive(archive, password, loc).with_context(|| {
        loc.tf(
            "backup.ctx.validate",
            &[("path", &archive.display().to_string())],
        )
    })?;

    // 2. Auto-copy of the prior data, if any. Normally the longest step, so it
    // says so before it starts; the path it produced is the caller's to report
    // once it exists (`RestoreOutcome::Restored`), which is where it is useful.
    let pre_restore = if has_existing_data(paths) {
        progress(loc.t("backup.progress.pre_restore"));
        let path = create_backup(
            paths,
            Some(default_backup_path(paths, "pre-restore")),
            packing,
            fs_root,
            password,
            loc,
            &mut progress,
        )
        .with_context(|| loc.t("backup.ctx.pre_restore").to_string())?;
        Some(path)
    } else {
        None
    };

    // 3. Clear + unpack.
    let attempt = (|| -> Result<()> {
        progress(loc.t("backup.progress.clearing"));
        clear_user_data(paths, fs_root, loc)?;
        progress(loc.t("backup.progress.extracting"));
        parallel::extract_archive(
            paths,
            archive,
            password,
            packing.threads,
            loc,
            &mut progress,
        )
    })();

    match attempt {
        Ok(()) => {
            // 3a. Compact what was unpacked, when there is anything to compact.
            // Deliberately after the attempt rather than inside it: the data is
            // already in place and correct, so a compaction failure must not turn
            // a successful restore into a rollback (the rollback path unpacks a
            // pre-restore copy, which `create_backup` already compacted).
            if restored_db_needs_compaction(paths) {
                progress(loc.t("backup.progress.compacting"));
                compact_restored_db(paths);
            }
            Ok(RestoreOutcome::Restored { pre_restore })
        }
        Err(restore_error) => match &pre_restore {
            // 4. Roll back to the just-created pre-restore copy.
            Some(backup) => {
                let rollback = (|| -> Result<()> {
                    progress(loc.t("backup.progress.rolling_back"));
                    clear_user_data(paths, fs_root, loc)?;
                    let threads = packing.threads;
                    parallel::extract_archive(paths, backup, password, threads, loc, &mut progress)
                })();
                match rollback {
                    Ok(()) => Ok(RestoreOutcome::RolledBack {
                        pre_restore: backup.clone(),
                        restore_error,
                    }),
                    Err(rollback_error) => Ok(RestoreOutcome::Failed {
                        pre_restore: pre_restore.clone(),
                        restore_error,
                        rollback_error: Some(rollback_error),
                    }),
                }
            }
            None => Ok(RestoreOutcome::Failed {
                pre_restore: None,
                restore_error,
                rollback_error: None,
            }),
        },
    }
}

/// Gathers the list of files to pack (deduplicated by archive name) — all but
/// the database, which is the pool's job of its own: compacted first, packed
/// raw with its sidecars only when it cannot be ([`parallel`]).
fn gather_entries(paths: &Paths, fs_root: Option<&Path>, loc: &Locale) -> Result<Vec<Entry>> {
    let root = paths.root();
    let mut out: Vec<Entry> = Vec::new();

    for f in TOP_FILES {
        let abs = root.join(f);
        if abs.is_file() {
            let meta = fs::metadata(&abs).ok();
            out.push(Entry::of(abs, (*f).to_string(), meta.as_ref()));
        }
    }

    // All top-level `*.bak` files (settings.bak, profiles.bak, etc.).
    if let Ok(rd) = fs::read_dir(root) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_file()
                && path.extension().is_some_and(|e| e == "bak")
                && let Some(name) = path.file_name().and_then(OsStr::to_str)
            {
                let meta = entry.metadata().ok();
                out.push(Entry::of(path.clone(), name.to_string(), meta.as_ref()));
            }
        }
    }

    for d in TOP_DIRS {
        collect_dir(&root.join(d), d, &mut out, loc)?;
    }

    // The file-tool sandbox — only if inside the data root.
    if let Some((abs, prefix)) = fs_root_under_root(root, fs_root) {
        collect_dir(&abs, &prefix, &mut out, loc)?;
    }

    // Dedup by archive name (in case fs_root overlaps another path).
    let mut seen = HashSet::new();
    out.retain(|e| seen.insert(e.name.clone()));
    Ok(out)
}

/// A scratch file beside the archive, removed on drop: the compacted copy of
/// `data.db` packed in place of the live file — removed after a failed `VACUUM
/// INTO` too, which can leave a partial file behind — and an entry's compressed
/// copy too big to wait in memory ([`parallel`]).
struct ScratchFile(PathBuf);

impl ScratchFile {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Compacts `data.db` into a scratch file next to the archive.
///
/// `None` — there is no database, or it could not be compacted (a corrupt file,
/// or one that isn't SQLite at all); the caller then packs the raw files, which
/// is the pre-compaction behaviour. Best effort by design: a backup must still
/// happen for a database we can't read.
fn compacted_db(paths: &Paths, out_path: &Path) -> Option<ScratchFile> {
    let src = paths.data_db();
    if !src.is_file() {
        return None;
    }
    // Next to the archive: same volume as the destination, and normally
    // `backups/` — never the data root, which restore clears.
    let dir = out_path.parent().unwrap_or_else(|| Path::new("."));
    if let Err(e) = fs::create_dir_all(dir) {
        tracing::warn!(error = %e, dir = %dir.display(), "backup: no scratch directory for compaction");
        return None;
    }
    // The process id keeps concurrent runs apart (the single-instance lock
    // already makes that unlikely); `Drop` cleans it up either way.
    let temp = ScratchFile(dir.join(format!("data.db.compact-{}.tmp", std::process::id())));
    match db::vacuum_into(&src, temp.path()) {
        Ok(()) => {
            tracing::info!(
                before = file_len(&src),
                after = file_len(temp.path()),
                "backup: database compacted"
            );
            Some(temp)
        }
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), "backup: packing the database uncompacted");
            None
        }
    }
}

/// Whether the unpacked database has anything an in-place `VACUUM` would give
/// back: free pages its header counts, or a `-wal` beside it, whose pages the
/// header does not know of (docs/research/backup-big-entry.md F3). A backup since
/// compaction packs a `VACUUM INTO` copy — no free page, no sidecar — so its
/// restore skips the step, which rewrites the whole file twice for nothing (8 s
/// for 285 MB, measured). An archive from before compaction, or from another
/// tool, is compacted as before. A file that is not a database is left as it is:
/// the `VACUUM` would refuse it anyway.
fn restored_db_needs_compaction(paths: &Paths) -> bool {
    let path = paths.data_db();
    let wal = paths.root().join("data.db-wal");
    path.is_file()
        && (file_len(&wal) > 0 || db::header_freelist_count(&path).is_some_and(|free| free > 0))
}

/// Compacts the restored database in place.
///
/// Best effort, and quiet on failure: the data is already unpacked and correct,
/// so the worst case is that it stays as fragmented as the archive was.
fn compact_restored_db(paths: &Paths) {
    let path = paths.data_db();
    if !path.is_file() {
        return;
    }
    let before = file_len(&path);
    match db::vacuum(&path) {
        Ok(()) => tracing::info!(
            before,
            after = file_len(&path),
            "restore: database compacted"
        ),
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), "restore: database left uncompacted")
        }
    }
}

/// File size in bytes (0 when it can't be read — this only feeds a log line).
fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Recursively collects the files of directory `abs` under name prefix `prefix` (with `/`).
fn collect_dir(abs: &Path, prefix: &str, out: &mut Vec<Entry>, loc: &Locale) -> Result<()> {
    if !abs.is_dir() {
        return Ok(());
    }
    let rd = fs::read_dir(abs).with_context(|| {
        loc.tf(
            "backup.ctx.read_dir",
            &[("path", &abs.display().to_string())],
        )
    })?;
    for entry in rd {
        let entry = entry?;
        let ft = entry.file_type()?;
        let child_abs = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue; // skip a non-UTF-8 name
        };
        let child_name = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if ft.is_dir() {
            collect_dir(&child_abs, &child_name, out, loc)?;
        } else if ft.is_file() {
            let meta = entry.metadata().ok();
            out.push(Entry::of(child_abs, child_name, meta.as_ref()));
        }
    }
    Ok(())
}

/// An entry's name as a person reads it — in a refusal, in a listing. Its raw
/// bytes, lossily: `zip` 9's `name()` is fallible (a name flagged UTF-8 that is
/// not), and the name of an entry being refused is exactly the one not to trust.
/// mindfork's own archives name their entries in UTF-8, where the two agree.
pub(crate) fn entry_name<R: io::Read + ?Sized>(entry: &zip::read::ZipFile<'_, R>) -> String {
    String::from_utf8_lossy(entry.name_raw()).into_owned()
}

/// Checks that the archive opens, that all of its entries are safe relative
/// paths (no `..`/absolute paths — zip-slip protection), and that `password`
/// actually opens it.
///
/// Runs **before** anything destructive, which is what makes a wrong password a
/// clean refusal rather than a rollback.
fn validate_archive(archive: &Path, password: Option<&str>, loc: &Locale) -> Result<()> {
    let file = File::open(archive).with_context(|| {
        loc.tf(
            "backup.ctx.open_archive",
            &[("path", &archive.display().to_string())],
        )
    })?;
    let mut zip = ZipArchive::new(file).with_context(|| loc.t("backup.ctx.corrupt").to_string())?;
    for i in 0..zip.len() {
        // `by_index_raw` doesn't decrypt — the names of an encrypted archive are
        // readable, so zip-slip is still checked before the password question.
        let entry = zip.by_index_raw(i)?;
        if entry.enclosed_name().is_none() {
            bail!(
                "{}",
                loc.tf("backup.err.unsafe_entry", &[("name", &entry_name(&entry))])
            );
        }
    }
    match check_password(archive, password)
        .with_context(|| loc.t("backup.ctx.corrupt").to_string())?
    {
        ArchivePassword::NotNeeded | ArchivePassword::Ok => Ok(()),
        ArchivePassword::Required => bail!("{}", loc.t("backup.err.password_required")),
        ArchivePassword::Wrong => bail!("{}", loc.t("backup.err.wrong_password")),
    }
}

/// Removes user data from the root, **keeping** `backups/`, `logs/`, and the
/// defaults files `defaults.json`/`location.json`. Clears exactly the set
/// that goes into the backup.
fn clear_user_data(paths: &Paths, fs_root: Option<&Path>, loc: &Locale) -> Result<()> {
    let root = paths.root();

    for f in TOP_FILES.iter().chain(DB_FILES) {
        remove_file_if_exists(&root.join(f), loc)?;
    }
    if let Ok(rd) = fs::read_dir(root) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().is_some_and(|e| e == "bak") {
                remove_file_if_exists(&path, loc)?;
            }
        }
    }
    for d in TOP_DIRS {
        remove_dir_if_exists(&root.join(d), loc)?;
    }
    if let Some((abs, _)) = fs_root_under_root(root, fs_root) {
        remove_dir_if_exists(&abs, loc)?;
    }
    Ok(())
}

fn remove_file_if_exists(path: &Path, loc: &Locale) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| {
            loc.tf(
                "backup.ctx.remove_file",
                &[("path", &path.display().to_string())],
            )
        }),
    }
}

fn remove_dir_if_exists(path: &Path, loc: &Locale) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| {
            loc.tf(
                "backup.ctx.remove_dir",
                &[("path", &path.display().to_string())],
            )
        }),
    }
}

/// Is there existing user data at the root (do we need a pre-restore copy).
fn has_existing_data(paths: &Paths) -> bool {
    ["settings.json", "profiles.json", "data.db"]
        .iter()
        .any(|f| paths.root().join(f).exists())
        || dir_non_empty(&paths.chats_dir())
}

fn dir_non_empty(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|mut rd| rd.next().is_some())
}

/// If `fs_root` is set and lies inside the data root, returns (canonical
/// path, archive name-prefix). Otherwise `None` (outside the root → not
/// included in the backup, not cleared).
fn fs_root_under_root(root: &Path, fs_root: Option<&Path>) -> Option<(PathBuf, String)> {
    let fs_root = fs_root?;
    let root_c = fs::canonicalize(root).ok()?;
    let fs_c = fs::canonicalize(fs_root).ok()?;
    let rel = fs_c.strip_prefix(&root_c).ok()?;
    if rel.as_os_str().is_empty() {
        return None;
    }
    let prefix = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if prefix.is_empty() {
        return None;
    }
    Some((fs_c, prefix))
}

/// What a backup or a restore needs out of `settings.json`: the file-tools
/// sandbox (for inclusion/cleanup), the stored backup password, and how the
/// archive is packed — the level and the thread count (`backup`).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BackupInputs {
    pub fs_root: Option<PathBuf>,
    pub stored_password: Option<String>,
    pub archive: BackupSettings,
}

/// Reads [`BackupInputs`] out of the **raw** JSON of `settings.json`, not
/// through `AppConfig`.
///
/// A settings file holding one value the typed parse refuses is a file the app
/// will not start on (`features::data_migration`), and the person it has just
/// refused is the one who wants a backup before repairing anything — so these
/// commands must not need the whole file to be readable, only their own values
/// (docs/research/settings-typed-parse.md F2). Read through the typed config
/// with a fallback to the defaults, such a file gave *no password*, and a
/// backup the settings call encrypted was written in plaintext.
///
/// No file — the defaults. `Err` — there is a file and the sandbox and the
/// password cannot be had from it: it is not JSON, or its key list is not one.
/// The level and the thread count never fail it ([`archive_settings`]).
pub fn backup_settings(paths: &Paths) -> Result<BackupInputs> {
    use crate::shared::secrets;

    let file = paths.settings_file();
    let bytes = match fs::read(&file) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BackupInputs::default()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", file.display())),
    };
    let raw: serde_json::Value =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", file.display()))?;
    let stored_password = match raw.get("api_keys") {
        None => None,
        Some(list) => {
            let entries = Vec::<secrets::ApiKeyEntry>::deserialize(list)
                .with_context(|| format!("parsing api_keys in {}", file.display()))?;
            secrets::stored_key(&entries, secrets::BACKUP_PASSWORD_KEY)
        }
    };
    let fs_root = raw
        .pointer("/tools/fs_root")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from);
    Ok(BackupInputs {
        fs_root,
        stored_password,
        archive: archive_settings(&raw),
    })
}

/// `backup.compression` and `backup.threads` out of the raw settings, each on
/// its own. A value that is not a whole non-negative number is that value's
/// default, said in the log: a backup that happens is worth more than one
/// refused over its level, and nothing read here is saved back. A level above 9
/// is 9, as the settings screen stores it.
fn archive_settings(raw: &serde_json::Value) -> BackupSettings {
    let defaults = BackupSettings::default();
    let number = |key: &str| {
        let value = raw.pointer(key)?;
        let number = value.as_u64();
        if number.is_none() {
            tracing::warn!(key, %value, "backup: not a whole number, using the default");
        }
        number
    };
    BackupSettings {
        compression: number("/backup/compression").map_or(defaults.compression, |v| {
            let max = MAX_BACKUP_COMPRESSION;
            u8::try_from(v).map_or(max, |v| v.min(max))
        }),
        threads: number("/backup/threads")
            .map_or(defaults.threads, |v| u32::try_from(v).unwrap_or(u32::MAX)),
    }
}

/// Auto-name for an archive in `backups/`: `<prefix>-YYYYMMDD-HHMMSS.zip`, or
/// with `-2`, `-3`… when that name is taken ([`unused_name`]).
pub(crate) fn default_backup_path(paths: &Paths, prefix: &str) -> PathBuf {
    let stamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
    unused_name(&paths.backups_dir(), prefix, &stamp)
}

/// `<prefix>-<stamp>.zip` in `dir`, or the first of `<prefix>-<stamp>-2.zip`,
/// `-3`… that does not exist. The stamp is to the second, and a name taken
/// twice in one second was one file: a restore of the pre-restore copy made in
/// that second wrote its own copy over the archive it was about to unpack. The
/// check and the write are not one step; `backup`, `restore` and the start-up's
/// pre-migration backup hold the single-instance lock, so none of them races
/// another.
fn unused_name(dir: &Path, prefix: &str, stamp: &str) -> PathBuf {
    let first = dir.join(format!("{prefix}-{stamp}.zip"));
    if !first.exists() {
        return first;
    }
    (2u32..)
        .map(|n| dir.join(format!("{prefix}-{stamp}-{n}.zip")))
        .find(|path| !path.exists())
        .unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    /// Reference locale for tests (error text isn't checked here — only the
    /// signature matters; ru byte-for-byte with the previous strings).
    fn ru() -> &'static Locale {
        locale(Lang::Ru)
    }

    /// A settings file the app refuses to start on still gives a backup its
    /// two values (docs/research/settings-typed-parse.md F2). Through the typed
    /// config such a file read as the defaults — *no password* — and a backup
    /// the settings called encrypted was written in plaintext.
    #[test]
    fn backup_settings_survive_a_value_the_config_cannot_read() {
        use crate::shared::config::AppConfig;
        use crate::shared::secrets;
        if !secrets::scheme_available() {
            return; // no machine-id: there is no stored password to lose
        }
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        let mut config = AppConfig::default();
        secrets::put_key(
            &mut config.api_keys,
            secrets::BACKUP_PASSWORD_KEY,
            "a pass phrase",
            || "test".into(),
        )
        .unwrap();
        config.tools.fs_root = Some("sandbox-files".into());
        config.backup = BackupSettings {
            compression: 1,
            threads: 2,
        };
        let mut raw = serde_json::to_value(&config).unwrap();
        raw["interface"]["theme"] = "drak".into();
        let bytes = serde_json::to_vec_pretty(&raw).unwrap();
        fs::write(paths.settings_file(), &bytes).unwrap();
        // The file is the kind this is about: the typed parse refuses it.
        assert!(serde_json::from_slice::<AppConfig>(&bytes).is_err());

        assert_eq!(
            backup_settings(&paths).unwrap(),
            BackupInputs {
                fs_root: Some(PathBuf::from("sandbox-files")),
                stored_password: Some("a pass phrase".into()),
                archive: BackupSettings {
                    compression: 1,
                    threads: 2,
                },
            }
        );
    }

    /// No file is a fresh root — the defaults, and no complaint. A file the
    /// values cannot be had from is an error for the caller to report: not
    /// JSON, or a key list that is not one.
    #[test]
    fn backup_settings_tell_no_file_from_an_unreadable_one() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        assert_eq!(backup_settings(&paths).unwrap(), BackupInputs::default());

        for unreadable in ["{ this is not json", r#"{"api_keys": "none"}"#] {
            fs::write(paths.settings_file(), unreadable).unwrap();
            let err = backup_settings(&paths).unwrap_err();
            let said = format!("{err:#}");
            assert!(said.contains("settings.json"), "{unreadable}: {said}");
        }

        // Neither value set: a file that says nothing about them.
        fs::write(paths.settings_file(), r#"{"tools": {"fs_root": null}}"#).unwrap();
        assert_eq!(backup_settings(&paths).unwrap(), BackupInputs::default());
    }

    /// Prepares a root with a typical set of user data.
    fn seed_data(root: &Path) {
        fs::write(root.join("settings.json"), b"{\"v\":1}").unwrap();
        fs::write(root.join("settings.bak"), b"{\"v\":0}").unwrap();
        fs::write(root.join("profiles.json"), b"[]").unwrap();
        fs::write(root.join("data.db"), b"SQLITE").unwrap();
        fs::write(root.join("personal_dictionary.txt"), b"foo\n").unwrap();
        fs::create_dir_all(root.join("chats")).unwrap();
        fs::write(root.join("chats").join("a.json"), b"{}").unwrap();
        fs::write(root.join("chats").join("a.bak"), b"{}").unwrap();
        fs::create_dir_all(root.join("dictionaries")).unwrap();
        fs::write(root.join("dictionaries").join("en.dic"), b"x").unwrap();
        fs::create_dir_all(root.join("locales")).unwrap();
        fs::write(root.join("locales").join("en.json"), b"{}").unwrap();
        fs::create_dir_all(root.join("themes")).unwrap();
        fs::write(
            root.join("themes").join("mine.json"),
            b"{\"canvas\":\"#002b36\"}",
        )
        .unwrap();
        fs::create_dir_all(root.join("files").join("c1")).unwrap();
        fs::write(root.join("files").join("c1").join("chart.png"), b"png").unwrap();
        fs::create_dir_all(root.join("workspace").join("c1")).unwrap();
        fs::write(
            root.join("workspace").join("c1").join("journal.json"),
            b"{\"entries\":[]}",
        )
        .unwrap();
        fs::create_dir_all(root.join("stories").join("c1")).unwrap();
        fs::write(root.join("stories").join("c1").join("s1.db"), b"story").unwrap();
        // Must not end up in the backup:
        fs::create_dir_all(root.join("story-logs").join("c1")).unwrap();
        fs::write(root.join("story-logs").join("c1").join("s1.db"), b"calls").unwrap();
        fs::create_dir_all(root.join("logs")).unwrap();
        fs::write(root.join("logs").join("mindfork.log"), b"log").unwrap();
        fs::create_dir_all(root.join("backups")).unwrap();
        fs::write(root.join("location.json"), b"{\"mode\":\"portable\"}").unwrap();
        fs::write(
            root.join("defaults.json"),
            b"{\"mode\":\"portable\",\"default_language\":\"ru\"}",
        )
        .unwrap();
    }

    /// Replaces the placeholder `data.db` with a real database carrying free
    /// pages (rows inserted, most of them deleted) — what compaction reclaims.
    /// Returns the profile whose one surviving chunk must stay searchable.
    fn seed_fragmented_db(root: &Path) -> uuid::Uuid {
        use crate::entities::rag::RagDocument;
        use crate::shared::storage::db::Db;

        let profile = uuid::Uuid::new_v4();
        let path = root.join("data.db");
        let _ = fs::remove_file(&path);
        let db = Db::open(&path).unwrap();
        // Batched (see `Db::batch`), but in two transactions: the file has to
        // grow to hold every row and only then have pages freed.
        db.batch(|| {
            for i in 0..200 {
                let text = format!("scratch {i} {}", "x".repeat(500));
                db.rag_insert(&RagDocument::new(profile, "scratch", text, vec![0.0, 1.0]))
                    .unwrap();
            }
            db.rag_insert(&RagDocument::new(
                profile,
                "keep",
                "the kept chunk",
                vec![1.0, 0.0],
            ))
            .unwrap();
        });
        db.batch(|| db.rag_delete_by_source(profile, "scratch").unwrap());
        profile
    }

    /// Unpacks one entry of the archive (for inspecting the packed database).
    fn extract_entry(archive: &Path, name: &str, dest: &Path) {
        let mut zip = ZipArchive::new(File::open(archive).unwrap()).unwrap();
        let mut entry = zip.by_name(name).unwrap();
        let mut out = File::create(dest).unwrap();
        io::copy(&mut entry, &mut out).unwrap();
    }

    fn archive_names(archive: &Path) -> Vec<String> {
        let mut zip = ZipArchive::new(File::open(archive).unwrap()).unwrap();
        (0..zip.len())
            .map(|i| entry_name(&zip.by_index(i).unwrap()))
            .collect()
    }

    #[test]
    fn backup_includes_expected_and_excludes_logs_marker() {
        let dir = tempfile::tempdir().unwrap();
        seed_data(dir.path());
        let paths = Paths::with_root(dir.path());

        let out = create_backup(&paths, None, Packing::at(9), None, None, ru(), |_| {}).unwrap();
        assert!(out.starts_with(paths.backups_dir()));
        let names = archive_names(&out);

        for expected in [
            "settings.json",
            "settings.bak",
            "profiles.json",
            "data.db",
            "personal_dictionary.txt",
            "chats/a.json",
            "chats/a.bak",
            "dictionaries/en.dic",
            "locales/en.json",
            // The user's themes: a file somebody wrote, like a locale
            // (spec §11.6, §12.3).
            "themes/mine.json",
            "files/c1/chart.png",
            // The pre-images of what the assistant edited in a chat's project: the one
            // thing that track stores which cannot be recomputed (spec §9.12).
            "workspace/c1/journal.json",
            // A chat's story (docs/history/fabula.md §6, T5).
            "stories/c1/s1.db",
        ] {
            assert!(
                names.contains(&expected.to_string()),
                "missing {expected} in {names:?}"
            );
        }
        // Logs, the stories' call logs (spec §15), the backups directory, and
        // the defaults files are not included.
        assert!(!names.iter().any(|n| n.starts_with("logs/")));
        assert!(!names.iter().any(|n| n.starts_with("story-logs/")));
        assert!(!names.iter().any(|n| n.starts_with("backups/")));
        assert!(!names.contains(&"location.json".to_string()));
        assert!(!names.contains(&"defaults.json".to_string()));
    }

    #[test]
    fn fs_root_included_only_when_inside_root() {
        // Inside the root — included.
        let dir = tempfile::tempdir().unwrap();
        seed_data(dir.path());
        let inside = dir.path().join("sandbox");
        fs::create_dir_all(&inside).unwrap();
        fs::write(inside.join("note.txt"), b"hi").unwrap();
        let paths = Paths::with_root(dir.path());
        let out = create_backup(
            &paths,
            None,
            Packing::at(9),
            Some(&inside),
            None,
            ru(),
            |_| {},
        )
        .unwrap();
        assert!(archive_names(&out).contains(&"sandbox/note.txt".to_string()));

        // Outside the root — not included.
        let outside_root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), b"no").unwrap();
        seed_data(outside_root.path());
        let paths2 = Paths::with_root(outside_root.path());
        let out2 = create_backup(
            &paths2,
            None,
            Packing::at(0),
            Some(outside.path()),
            None,
            ru(),
            |_| {},
        )
        .unwrap();
        assert!(
            !archive_names(&out2)
                .iter()
                .any(|n| n.contains("secret.txt"))
        );
    }

    #[test]
    fn restore_round_trip_replaces_data() {
        // Source.
        let src = tempfile::tempdir().unwrap();
        seed_data(src.path());
        fs::write(src.path().join("settings.json"), b"{\"v\":42}").unwrap();
        let src_paths = Paths::with_root(src.path());
        let archive_path = src.path().join("backups").join("snap.zip");
        create_backup(
            &src_paths,
            Some(archive_path.clone()),
            Packing::at(9),
            None,
            None,
            ru(),
            |_| {},
        )
        .unwrap();

        // Target with different data.
        let dst = tempfile::tempdir().unwrap();
        seed_data(dst.path());
        fs::write(dst.path().join("settings.json"), b"{\"v\":999}").unwrap();
        fs::write(dst.path().join("chats").join("stale.json"), b"{}").unwrap();
        // A theme the archive does not carry, and a changed one it does.
        fs::write(dst.path().join("themes").join("stale.json"), b"{}").unwrap();
        fs::write(dst.path().join("themes").join("mine.json"), b"{}").unwrap();
        // A journal of a chat the archive knows nothing about: the clearing half has to
        // take it, or a restore would leave another conversation's baselines behind.
        fs::create_dir_all(dst.path().join("workspace").join("other")).unwrap();
        fs::write(
            dst.path().join("workspace").join("other").join("j.json"),
            b"{}",
        )
        .unwrap();
        // A story the archive does not carry, and a call log, which no archive
        // carries: the first is cleared, the second stays (spec §15).
        fs::create_dir_all(dst.path().join("stories").join("other")).unwrap();
        fs::write(dst.path().join("stories").join("other").join("s.db"), b"x").unwrap();
        fs::write(
            dst.path().join("stories").join("c1").join("s1.db"),
            b"newer",
        )
        .unwrap();
        fs::write(
            dst.path().join("story-logs").join("c1").join("s1.db"),
            b"its calls",
        )
        .unwrap();
        let dst_paths = Paths::with_root(dst.path());

        let outcome = restore_backup(
            &dst_paths,
            &archive_path,
            Packing::default(),
            None,
            None,
            ru(),
            |_| {},
        )
        .unwrap();
        match outcome {
            RestoreOutcome::Restored { pre_restore } => {
                // The prior data existed, so a pre-restore copy was created.
                let pre = pre_restore.expect("a pre-restore copy should have been created");
                assert!(pre.exists());
                assert!(pre.starts_with(dst_paths.backups_dir()));
            }
            _ => panic!("expected a successful Restored"),
        }
        // Data replaced with the archive's content.
        assert_eq!(
            fs::read(dst.path().join("settings.json")).unwrap(),
            b"{\"v\":42}"
        );
        // The stale chat absent from the archive was removed by the cleanup.
        assert!(!dst.path().join("chats").join("stale.json").exists());
        // The themes are one of the directories packed and cleared together:
        // the archive's theme is back as it was written, the other is gone.
        assert!(!dst.path().join("themes").join("stale.json").exists());
        assert_eq!(
            fs::read(dst.path().join("themes").join("mine.json")).unwrap(),
            b"{\"canvas\":\"#002b36\"}"
        );
        // The code workspaces' change journals travel with the rest: the archive's
        // baseline is back, and the one it does not carry is gone (spec §9.12, §12.3).
        assert!(
            dst.path()
                .join("workspace")
                .join("c1")
                .join("journal.json")
                .exists(),
            "the archive's workspace journal was not restored"
        );
        assert!(
            !dst.path().join("workspace").join("other").exists(),
            "a journal the archive does not carry survived the restore"
        );
        // The stories are packed and cleared together; the call logs are in
        // neither half, so the one here is still here.
        let stories = dst.path().join("stories");
        assert_eq!(
            fs::read(stories.join("c1").join("s1.db")).unwrap(),
            b"story"
        );
        assert!(!stories.join("other").exists());
        assert_eq!(
            fs::read(dst.path().join("story-logs").join("c1").join("s1.db")).unwrap(),
            b"its calls"
        );
        // The backups directory is preserved (it holds the pre-restore copy).
        assert!(dst.path().join("backups").exists());
    }

    /// A zip holds no alternate streams, so a stored file's mark does not survive the round
    /// trip — the restore sets it again on everything under `files/`, and on nothing else
    /// (§13 U11).
    #[cfg(windows)]
    #[test]
    fn a_restored_stored_file_is_marked_as_come_from_elsewhere() {
        use crate::shared::os_open::{FROM_ELSEWHERE, zone_of};
        let src = tempfile::tempdir().unwrap();
        seed_data(src.path());
        let archive_path = src.path().join("backups").join("snap.zip");
        create_backup(
            &Paths::with_root(src.path()),
            Some(archive_path.clone()),
            Packing::at(9),
            None,
            None,
            ru(),
            |_| {},
        )
        .unwrap();

        let dst = tempfile::tempdir().unwrap();
        restore_backup(
            &Paths::with_root(dst.path()),
            &archive_path,
            Packing::default(),
            None,
            None,
            ru(),
            |_| {},
        )
        .unwrap();
        let chart = dst.path().join("files").join("c1").join("chart.png");
        assert_eq!(fs::read(&chart).unwrap(), b"png");
        assert_eq!(zone_of(&chart).as_deref(), Some(FROM_ELSEWHERE));
        assert_eq!(
            zone_of(&dst.path().join("chats").join("a.json")),
            None,
            "only files/ is marked"
        );
    }

    /// The copy a restore makes first is auto-named to the second. Restoring
    /// that copy in the second it was made named the new copy as the archive
    /// being restored, wrote it over that archive, and unpacked it: the data
    /// the copy was there to keep was gone (found by PR 7's round trip,
    /// docs/journal/storage.md). Every name a restore could pick in the next
    /// seconds is taken here, so whichever it picks, nothing there may change.
    #[test]
    fn a_restore_never_writes_its_copy_over_an_archive_already_there() {
        let src = tempfile::tempdir().unwrap();
        seed_data(src.path());
        fs::write(src.path().join("settings.json"), b"{\"v\":42}").unwrap();
        let archive = src.path().join("a.zip");
        let src_paths = Paths::with_root(src.path());
        create_backup(
            &src_paths,
            Some(archive.clone()),
            Packing::at(1),
            None,
            None,
            ru(),
            |_| {},
        )
        .unwrap();
        let dst = tempfile::tempdir().unwrap();
        seed_data(dst.path());
        let dst_paths = Paths::with_root(dst.path());
        let now = Local::now();
        let names: Vec<PathBuf> = (0..6)
            .map(|s| {
                let at = now + chrono::Duration::seconds(s);
                let name = format!("pre-restore-{}.zip", at.format("%Y%m%d-%H%M%S"));
                dst_paths.backups_dir().join(name)
            })
            .collect();
        for name in &names {
            fs::copy(&archive, name).unwrap();
        }
        let before: Vec<Vec<u8>> = names.iter().map(|n| fs::read(n).unwrap()).collect();

        let outcome = restore_backup(
            &dst_paths,
            &names[0],
            Packing::default(),
            None,
            None,
            ru(),
            |_| {},
        )
        .unwrap();

        let RestoreOutcome::Restored {
            pre_restore: Some(copy),
        } = outcome
        else {
            panic!("expected a restore with a pre-restore copy");
        };
        assert!(!names.contains(&copy), "{copy:?} took an archive's name");
        for (name, bytes) in names.iter().zip(&before) {
            assert_eq!(&fs::read(name).unwrap(), bytes, "{name:?} was written over");
        }
        assert_eq!(
            fs::read(dst.path().join("settings.json")).unwrap(),
            b"{\"v\":42}"
        );
    }

    /// An auto-name that is taken gets `-2`, `-3`…: two copies made in one
    /// second are two files.
    #[test]
    fn an_auto_name_never_takes_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let stamp = "20260101-000000";
        assert_eq!(
            unused_name(dir.path(), "p", stamp),
            dir.path().join("p-20260101-000000.zip")
        );
        fs::write(dir.path().join("p-20260101-000000.zip"), b"").unwrap();
        fs::write(dir.path().join("p-20260101-000000-2.zip"), b"").unwrap();
        assert_eq!(
            unused_name(dir.path(), "p", stamp),
            dir.path().join("p-20260101-000000-3.zip")
        );
    }

    #[test]
    fn restore_rejects_corrupt_archive_without_touching_data() {
        let dst = tempfile::tempdir().unwrap();
        seed_data(dst.path());
        let paths = Paths::with_root(dst.path());
        let bad = dst.path().join("bad.zip");
        fs::write(&bad, b"this is not a zip file").unwrap();

        let err = restore_backup(&paths, &bad, Packing::default(), None, None, ru(), |_| {});
        assert!(
            err.is_err(),
            "a corrupted archive should give Err before any cleanup"
        );
        // Data is untouched.
        assert!(dst.path().join("settings.json").exists());
        assert!(dst.path().join("chats").join("a.json").exists());
    }

    #[test]
    fn restore_into_empty_root_makes_no_pre_restore() {
        let src = tempfile::tempdir().unwrap();
        seed_data(src.path());
        let archive = src.path().join("snap.zip");
        create_backup(
            &Paths::with_root(src.path()),
            Some(archive.clone()),
            Packing::at(9),
            None,
            None,
            ru(),
            |_| {},
        )
        .unwrap();

        let dst = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dst.path());
        let outcome = restore_backup(
            &paths,
            &archive,
            Packing::default(),
            None,
            None,
            ru(),
            |_| {},
        )
        .unwrap();
        match outcome {
            RestoreOutcome::Restored { pre_restore } => assert!(pre_restore.is_none()),
            _ => panic!("expected a Restored with no pre-restore"),
        }
        assert!(dst.path().join("settings.json").exists());
    }

    #[test]
    fn restore_rolls_back_on_extraction_failure() {
        use std::io::Write;

        // The archive is valid (passes validate_archive), but the entry
        // `blocker` is a file that will collide with a same-named directory
        // at the target → unpacking will fail.
        let work = tempfile::tempdir().unwrap();
        let archive = work.path().join("evil.zip");
        {
            let f = File::create(&archive).unwrap();
            let mut zip = ZipWriter::new(f);
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zip.start_file("settings.json", opts).unwrap();
            zip.write_all(b"{\"from\":\"archive\"}").unwrap();
            zip.start_file("blocker", opts).unwrap();
            zip.write_all(b"x").unwrap();
            zip.finish().unwrap();
        }

        let dst = tempfile::tempdir().unwrap();
        seed_data(dst.path());
        fs::write(dst.path().join("settings.json"), b"{\"from\":\"original\"}").unwrap();
        // The `blocker` directory isn't in the whitelist → it survives the
        // cleanup and breaks unpacking the same-named file.
        fs::create_dir_all(dst.path().join("blocker")).unwrap();

        let paths = Paths::with_root(dst.path());
        let outcome = restore_backup(
            &paths,
            &archive,
            Packing::default(),
            None,
            None,
            ru(),
            |_| {},
        )
        .unwrap();
        match outcome {
            RestoreOutcome::RolledBack { pre_restore, .. } => assert!(pre_restore.exists()),
            _ => panic!("expected RolledBack on an unpack failure"),
        }
        // The rollback restored the original data from the pre-restore copy.
        assert_eq!(
            fs::read(dst.path().join("settings.json")).unwrap(),
            b"{\"from\":\"original\"}"
        );
        assert!(dst.path().join("chats").join("a.json").exists());
    }

    /// Position of `key`'s message in the progress log, or `None`.
    fn step(log: &[String], key: &str) -> Option<usize> {
        let text = ru().t(key);
        log.iter().position(|line| line == text)
    }

    /// Which step of the log `key` is, failing with the whole log when absent —
    /// a missing phase is otherwise reported as a bare `None`.
    fn step_at(log: &[String], key: &str) -> usize {
        step(log, key).unwrap_or_else(|| panic!("{key} was never reported; log: {log:#?}"))
    }

    #[test]
    fn restore_announces_every_phase_in_order() {
        // The defect this guards: the CLI used to print nothing until the whole
        // restore was over, so a data root that takes seconds to pack looked
        // like a hung program — right after an echo-less password prompt.
        let src = tempfile::tempdir().unwrap();
        seed_data(src.path());
        let archive = src.path().join("snap.zip");
        create_backup(
            &Paths::with_root(src.path()),
            Some(archive.clone()),
            Packing::at(9),
            None,
            None,
            ru(),
            |_| {},
        )
        .unwrap();

        let dst = tempfile::tempdir().unwrap();
        seed_data(dst.path());
        let paths = Paths::with_root(dst.path());
        let mut log = Vec::new();
        let outcome = restore_backup(
            &paths,
            &archive,
            Packing::default(),
            None,
            None,
            ru(),
            |m| log.push(m.to_string()),
        )
        .unwrap();
        assert!(matches!(outcome, RestoreOutcome::Restored { .. }));

        // The order is the contract: each label names the step that follows it.
        let order = [
            "backup.progress.checking",
            "backup.progress.pre_restore",
            "backup.progress.packing",
            "backup.progress.clearing",
            "backup.progress.extracting",
        ]
        .map(|key| step_at(&log, key));
        assert!(
            order.windows(2).all(|w| w[0] < w[1]),
            "phases out of order: {log:#?}"
        );
        // The prior root had a database, so the pre-restore copy's compaction
        // spoke up; what was unpacked is not a database (`seed_data`'s bytes), so
        // there is nothing to compact and nothing is announced after unpacking.
        assert_eq!(
            log.iter()
                .filter(|l| *l == ru().t("backup.progress.compacting"))
                .count(),
            1,
            "{log:#?}"
        );
    }

    #[test]
    fn restore_into_an_empty_root_does_not_announce_a_pre_restore_copy() {
        // Nothing to save — saying otherwise would describe work never done.
        let src = tempfile::tempdir().unwrap();
        seed_data(src.path());
        let archive = src.path().join("snap.zip");
        create_backup(
            &Paths::with_root(src.path()),
            Some(archive.clone()),
            Packing::at(9),
            None,
            None,
            ru(),
            |_| {},
        )
        .unwrap();

        let dst = tempfile::tempdir().unwrap();
        let mut log = Vec::new();
        restore_backup(
            &Paths::with_root(dst.path()),
            &archive,
            Packing::default(),
            None,
            None,
            ru(),
            |m| log.push(m.to_string()),
        )
        .unwrap();

        assert!(
            step(&log, "backup.progress.pre_restore").is_none(),
            "{log:#?}"
        );
        assert!(
            step(&log, "backup.progress.extracting").is_some(),
            "{log:#?}"
        );
    }

    #[test]
    fn a_rollback_says_so() {
        use std::io::Write;

        // Same crafted archive as `restore_rolls_back_on_extraction_failure`: a
        // file entry colliding with a surviving directory of the same name.
        let work = tempfile::tempdir().unwrap();
        let archive = work.path().join("evil.zip");
        {
            let f = File::create(&archive).unwrap();
            let mut zip = ZipWriter::new(f);
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zip.start_file("settings.json", opts).unwrap();
            zip.write_all(b"{}").unwrap();
            zip.start_file("blocker", opts).unwrap();
            zip.write_all(b"x").unwrap();
            zip.finish().unwrap();
        }

        let dst = tempfile::tempdir().unwrap();
        seed_data(dst.path());
        fs::create_dir_all(dst.path().join("blocker")).unwrap();

        let mut log = Vec::new();
        let outcome = restore_backup(
            &Paths::with_root(dst.path()),
            &archive,
            Packing::default(),
            None,
            None,
            ru(),
            |m| log.push(m.to_string()),
        )
        .unwrap();
        assert!(matches!(outcome, RestoreOutcome::RolledBack { .. }));
        // The longest silence of all is the one where the data is being put
        // back: it must not look like the program died mid-restore.
        assert!(
            step_at(&log, "backup.progress.rolling_back")
                > step_at(&log, "backup.progress.extracting"),
            "{log:#?}"
        );
    }

    #[test]
    fn backup_announces_compaction_and_packing() {
        let dir = tempfile::tempdir().unwrap();
        seed_data(dir.path());
        let mut log = Vec::new();
        create_backup(
            &Paths::with_root(dir.path()),
            None,
            Packing::at(9),
            None,
            None,
            ru(),
            |m| log.push(m.to_string()),
        )
        .unwrap();
        assert!(
            step_at(&log, "backup.progress.compacting") < step_at(&log, "backup.progress.packing"),
            "{log:#?}"
        );
    }

    #[test]
    fn the_entry_counter_stays_quiet_until_its_interval_passes() {
        // A small data root packs in milliseconds; counting it out loud would
        // be noise, so the ticker only speaks for a loop that actually drags.
        let mut log = Vec::new();
        let mut ticker = EntryProgress::every(3, Duration::from_secs(3600));
        for _ in 0..3 {
            ticker.tick(ru(), &mut |m: &str| log.push(m.to_string()));
        }
        assert!(log.is_empty(), "{log:#?}");
    }

    #[test]
    fn the_entry_counter_reports_progress_out_of_the_total() {
        let mut log = Vec::new();
        let mut ticker = EntryProgress::every(3, Duration::ZERO);
        for _ in 0..3 {
            ticker.tick(ru(), &mut |m: &str| log.push(m.to_string()));
        }
        assert_eq!(log.len(), 3, "{log:#?}");
        assert!(log[0].contains('1') && log[0].contains('3'), "{}", log[0]);
        assert!(log[2].contains('3'), "{}", log[2]);
        // No unsubstituted placeholder left in either language.
        for lang in Lang::ALL {
            let mut ticker = EntryProgress::every(7, Duration::ZERO);
            let mut line = String::new();
            ticker.tick(locale(*lang), &mut |m: &str| line = m.to_string());
            assert!(!line.contains('{'), "{lang:?}: {line}");
        }
    }

    #[test]
    fn corrupt_archive_error_is_localized() {
        // Regression against a forgotten `loc`: the error context is in the locale's language.
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.zip");
        fs::write(&bad, b"this is not a zip file").unwrap();
        let en = validate_archive(&bad, None, locale(Lang::En))
            .unwrap_err()
            .to_string();
        assert!(en.contains("corrupted"), "{en}");
        assert!(!en.chars().any(|c| ('а'..='я').contains(&c)), "{en}");
        let r = validate_archive(&bad, None, ru()).unwrap_err().to_string();
        assert!(r.contains("повреждён"), "{r}");
    }

    #[test]
    fn store_level_zero_produces_readable_archive() {
        let dir = tempfile::tempdir().unwrap();
        seed_data(dir.path());
        let paths = Paths::with_root(dir.path());
        let out = create_backup(&paths, None, Packing::at(0), None, None, ru(), |_| {}).unwrap();
        // The archive is valid and opens.
        validate_archive(&out, None, ru()).unwrap();
    }

    #[test]
    fn backup_writes_manifest_and_read_manifest_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        seed_data(dir.path());
        let paths = Paths::with_root(dir.path());
        let out = create_backup(&paths, None, Packing::at(9), None, None, ru(), |_| {}).unwrap();

        assert!(archive_names(&out).contains(&MANIFEST_NAME.to_string()));
        let m = read_manifest(&out)
            .unwrap()
            .expect("a manifest should be present");
        assert_eq!(m.app_version, env!("CARGO_PKG_VERSION"));
        // What `seed_data`'s files hold: settings and the chat unstamped (1),
        // a bare profiles array (1), a database that is not SQLite (0).
        let held = SchemaVersions {
            settings: 1,
            profiles: 1,
            chat: 1,
            db: 0,
            story: 0,
        };
        assert_eq!(m.schemas, held);
        assert!(!m.is_newer_than_current());
    }

    #[test]
    fn manifest_detects_newer_schema() {
        let m = BackupManifest {
            app_version: "9.9.9".into(),
            schemas: SchemaVersions {
                settings: SETTINGS_SCHEMA + 1,
                profiles: PROFILES_SCHEMA,
                chat: CHAT_SCHEMA,
                db: DB_SCHEMA,
                story: STORY_SCHEMA,
            },
            created_at: "2030-01-01T00:00:00+00:00".into(),
        };
        assert!(m.is_newer_than_current());
    }

    /// The release before the chats' last step: every schema this build has but
    /// the chats', one lower (0.22.0 to 0.23.0's chats 6 → 7).
    fn reader_before_the_chat_step() -> SchemaVersions {
        SchemaVersions {
            chat: CHAT_SCHEMA - 1,
            ..SchemaVersions::current()
        }
    }

    /// A story the engine's own store wrote, with its call log: the story
    /// database at `STORY_SCHEMA` under its chat (docs/history/fabula.md §6, T5).
    pub(crate) fn seed_story(root: &Path, chat: &str, story: &str) {
        use crate::shared::storage::story::StoryDb;
        let paths = Paths::with_root(root);
        let (db, log) = (paths.story_db(chat, story), paths.story_log(chat, story));
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                let store = StoryDb::open(&db, &log).await.unwrap();
                let mut author = store.author_writer().await.unwrap();
                author
                    .import_canon(&crate::entities::story::fixture_canon())
                    .await
                    .unwrap();
                drop(author);
                store.close().await.unwrap();
            });
    }

    /// A root whose files are at the given schemas: `settings.json` and one
    /// chat stamped, a bare `profiles.json` array (1), a database the app
    /// opened (`DB_SCHEMA`), and a story its store wrote (`STORY_SCHEMA`).
    fn seed_versions(root: &Path, settings: u32, chat: u32) {
        fs::write(
            root.join("settings.json"),
            format!("{{\"schema_version\":{settings}}}"),
        )
        .unwrap();
        fs::write(root.join("profiles.json"), b"[]").unwrap();
        fs::create_dir_all(root.join("chats")).unwrap();
        fs::write(
            root.join("chats").join("a.json"),
            format!("{{\"v\":{chat}}}"),
        )
        .unwrap();
        crate::shared::storage::db::Db::open(&root.join("data.db")).unwrap();
        seed_story(root, "a", "s");
    }

    /// A backup of files no start has migrated yet — a pre-migration backup,
    /// `backup` run by a new version before its first start — records the
    /// schemas they hold, not this build's, and still names the build that
    /// wrote it: the release before the step reads every file in it and is not
    /// warned (spec §12.3; 0.22.0 was, on 0.23.0's).
    #[test]
    fn a_backup_records_what_its_files_hold() {
        let dir = tempfile::tempdir().unwrap();
        seed_versions(dir.path(), SETTINGS_SCHEMA, CHAT_SCHEMA - 1);
        let paths = Paths::with_root(dir.path());
        let out = create_backup(&paths, None, Packing::at(1), None, None, ru(), |_| {}).unwrap();

        let m = read_manifest(&out).unwrap().expect("a manifest");
        assert_eq!(m.schemas, reader_before_the_chat_step());
        assert_eq!(m.app_version, env!("CARGO_PKG_VERSION"), "who wrote it");
        assert_eq!(m.newer_than(&reader_before_the_chat_step()), vec![]);
        assert!(!m.is_newer_than_current());
        assert_eq!(m.restore_warning(locale(Lang::En)), None);
    }

    /// A backup of files at this build's schemas is newer for the release
    /// before a step, and the warning names the file.
    #[test]
    fn a_backup_of_current_files_is_newer_for_the_release_before_a_step() {
        let dir = tempfile::tempdir().unwrap();
        seed_versions(dir.path(), SETTINGS_SCHEMA, CHAT_SCHEMA);
        let paths = Paths::with_root(dir.path());
        let out = create_backup(&paths, None, Packing::at(1), None, None, ru(), |_| {}).unwrap();

        let m = read_manifest(&out).unwrap().expect("a manifest");
        assert_eq!(m.schemas, SchemaVersions::current());
        assert_eq!(
            m.newer_than(&reader_before_the_chat_step()),
            vec![NewerSchema {
                file: "chats/*.json",
                found: CHAT_SCHEMA,
                supported: CHAT_SCHEMA - 1,
            }]
        );
    }

    /// The chats count by the newest — a reader older than one file refuses
    /// that file — and only `chats/*.json`, what the start-up reads: a `.bak`
    /// is not counted, nor a file that is not JSON. A kind with no file, a
    /// settings file that is not JSON and a database that is not SQLite are 0;
    /// the backup is made all the same.
    #[test]
    fn what_a_backup_records_is_what_the_start_up_would_read() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("settings.json"), b"not json").unwrap();
        fs::write(root.join("data.db"), b"SQLITE").unwrap();
        fs::create_dir_all(root.join("chats")).unwrap();
        for (name, body) in [
            ("a.json", "{\"v\":6}"),
            ("b.json", "{\"v\":7}"),
            ("c.json", "{}"),
            ("d.json", "{\"v\":"),
            ("b.bak", "{\"v\":99}"),
        ] {
            fs::write(root.join("chats").join(name), body).unwrap();
        }
        let paths = Paths::with_root(root);
        let out = create_backup(&paths, None, Packing::at(1), None, None, ru(), |_| {}).unwrap();

        let m = read_manifest(&out).unwrap().expect("a manifest");
        let held = SchemaVersions {
            settings: 0,
            profiles: 0,
            chat: 7,
            db: 0,
            story: 0,
        };
        assert_eq!(m.schemas, held);
    }

    /// The copy `restore` makes of a root restored and not yet opened records
    /// what that root holds — here a chat at the schema before this build's.
    #[test]
    fn the_pre_restore_copy_records_what_the_root_holds() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir_all(&root).unwrap();
        seed_versions(&root, SETTINGS_SCHEMA, CHAT_SCHEMA - 1);
        let paths = Paths::with_root(&root);
        let other = dir.path().join("other");
        fs::create_dir_all(&other).unwrap();
        seed_versions(&other, SETTINGS_SCHEMA, CHAT_SCHEMA);
        let archive = dir.path().join("other.zip");
        create_backup(
            &Paths::with_root(&other),
            Some(archive.clone()),
            Packing::at(1),
            None,
            None,
            ru(),
            |_| {},
        )
        .unwrap();

        let outcome = restore_backup(
            &paths,
            &archive,
            Packing::default(),
            None,
            None,
            ru(),
            |_| {},
        )
        .unwrap();

        let RestoreOutcome::Restored {
            pre_restore: Some(pre),
        } = outcome
        else {
            panic!("expected a restore with a pre-restore copy");
        };
        let m = read_manifest(&pre).unwrap().expect("a manifest");
        assert_eq!(m.schemas, reader_before_the_chat_step());
    }

    /// The stories count by the newest, read from each file's header as the
    /// database's is: the one a reader cannot open is the one to warn about.
    /// Only `stories/<chat>/<story>.db` is a story; a call log is not packed
    /// and not counted (spec §15).
    #[test]
    fn a_backup_records_the_newest_story() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        seed_story(root, "a", "older");
        let paths = Paths::with_root(root);
        let newer = paths.story_db("b", "newer");
        fs::create_dir_all(newer.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(&newer).unwrap();
        conn.execute_batch(&format!("PRAGMA user_version = {};", STORY_SCHEMA + 1))
            .unwrap();
        drop(conn);
        let stray = paths.stories_dir().join("b").join("readme.txt");
        fs::write(&stray, b"not a story").unwrap();
        let log = paths.story_log("c", "far-newer");
        fs::create_dir_all(log.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(&log).unwrap();
        conn.execute_batch("PRAGMA user_version = 99;").unwrap();
        drop(conn);

        let out = create_backup(&paths, None, Packing::at(1), None, None, ru(), |_| {}).unwrap();

        let m = read_manifest(&out).unwrap().expect("a manifest");
        assert_eq!(m.schemas.story, STORY_SCHEMA + 1);
        assert_eq!(
            m.newer_schemas(),
            vec![NewerSchema {
                file: "stories/*/*.db",
                found: STORY_SCHEMA + 1,
                supported: STORY_SCHEMA,
            }]
        );
        let names = archive_names(&out);
        assert!(
            names.contains(&"stories/b/readme.txt".to_string()),
            "{names:?}"
        );
        assert!(
            !names.iter().any(|n| n.starts_with("story-logs/")),
            "{names:?}"
        );
    }

    /// A manifest written before stories has no `story`, and reads as holding
    /// none: what such an archive holds.
    #[test]
    fn a_manifest_before_stories_holds_none() {
        let written = r#"{"app_version":"0.23.1",
            "schemas":{"settings":7,"profiles":1,"chat":7,"db":1},
            "created_at":"2026-10-31T01:00:00+00:00"}"#;
        let m: BackupManifest = serde_json::from_str(written).unwrap();
        assert_eq!(m.schemas.story, 0);
        assert!(!m.is_newer_than_current());
    }

    /// The warning names each file the archive holds at a newer schema, both
    /// schemas, and the version that wrote it — every placeholder filled in
    /// every language.
    #[test]
    fn the_restore_warning_names_what_is_newer() {
        let m = BackupManifest {
            app_version: "9.9.9".into(),
            schemas: SchemaVersions {
                chat: CHAT_SCHEMA + 1,
                db: DB_SCHEMA + 2,
                ..SchemaVersions::current()
            },
            created_at: "2030-01-01T00:00:00+00:00".into(),
        };
        let en = m.restore_warning(locale(Lang::En)).expect("a warning");
        let chats = format!(
            "chats/*.json: schema {}, supported {CHAT_SCHEMA}",
            CHAT_SCHEMA + 1
        );
        let db = format!("data.db: schema {}, supported {DB_SCHEMA}", DB_SCHEMA + 2);
        assert!(en.contains(&format!("({chats}; {db})")), "{en}");
        assert!(en.contains("9.9.9"), "{en}");
        assert!(!en.contains("settings.json"), "{en}");
        for lang in Lang::ALL {
            let w = m.restore_warning(locale(*lang)).expect("a warning");
            assert!(!w.contains(['{', '}']), "{lang:?}: {w}");
            assert!(
                w.contains("chats/*.json") && w.contains("9.9.9"),
                "{lang:?}: {w}"
            );
        }
    }

    #[test]
    fn read_manifest_none_for_archive_without_it() {
        // Assemble the archive by hand with no manifest (emulating an old backup).
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("old.zip");
        {
            let mut zip = ZipWriter::new(File::create(&archive).unwrap());
            zip.start_file("settings.json", SimpleFileOptions::default())
                .unwrap();
            io::copy(&mut b"{}".as_slice(), &mut zip).unwrap();
            zip.finish().unwrap();
        }
        assert!(read_manifest(&archive).unwrap().is_none());
    }

    #[test]
    fn backup_packs_a_compacted_database_without_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        seed_data(dir.path());
        let profile = seed_fragmented_db(dir.path());
        // A sidecar left over from a crash: its content is folded into the
        // compacted copy, so it must not be packed alongside it.
        fs::write(dir.path().join("data.db-wal"), b"stale wal").unwrap();
        let live_len = fs::metadata(dir.path().join("data.db")).unwrap().len();

        let paths = Paths::with_root(dir.path());
        let out = create_backup(&paths, None, Packing::at(9), None, None, ru(), |_| {}).unwrap();

        let names = archive_names(&out);
        assert!(names.contains(&"data.db".to_string()), "{names:?}");
        assert!(!names.contains(&"data.db-wal".to_string()), "{names:?}");
        assert!(!names.contains(&"data.db-shm".to_string()), "{names:?}");

        // The packed database is compacted — and still usable, which is the
        // half a size assertion alone would miss.
        let packed = dir.path().join("unpacked.db");
        extract_entry(&out, "data.db", &packed);
        assert!(
            fs::metadata(&packed).unwrap().len() < live_len,
            "the packed copy should be smaller than the live file"
        );
        let db = crate::shared::storage::db::Db::open(&packed).unwrap();
        let hits = db.rag_search(profile, &[1.0, 0.0], 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].chunk_text, "the kept chunk");

        // The scratch copy doesn't outlive the backup.
        let leftovers: Vec<String> = fs::read_dir(paths.backups_dir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn backup_packs_the_raw_database_when_it_cannot_be_compacted() {
        // `seed_data` leaves a placeholder that isn't a SQLite file at all. The
        // fallback is the point: an unreadable database must still be backed up
        // byte for byte, sidecars included.
        let dir = tempfile::tempdir().unwrap();
        seed_data(dir.path());
        fs::write(dir.path().join("data.db-wal"), b"wal bytes").unwrap();
        let paths = Paths::with_root(dir.path());

        let out = create_backup(&paths, None, Packing::at(9), None, None, ru(), |_| {}).unwrap();

        let names = archive_names(&out);
        assert!(names.contains(&"data.db-wal".to_string()), "{names:?}");
        let packed = dir.path().join("unpacked.db");
        extract_entry(&out, "data.db", &packed);
        assert_eq!(fs::read(&packed).unwrap(), b"SQLITE");
    }

    #[test]
    fn restore_compacts_the_database() {
        // An archive from before compaction existed (or made by another tool):
        // a fragmented database packed raw. Restoring it must leave a compacted
        // file on disk.
        let src = tempfile::tempdir().unwrap();
        seed_data(src.path());
        let profile = seed_fragmented_db(src.path());
        let fragmented = fs::read(src.path().join("data.db")).unwrap();

        let archive = src.path().join("old.zip");
        {
            let mut zip = ZipWriter::new(File::create(&archive).unwrap());
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zip.start_file("settings.json", opts).unwrap();
            io::copy(&mut b"{}".as_slice(), &mut zip).unwrap();
            zip.start_file("data.db", opts).unwrap();
            io::copy(&mut fragmented.as_slice(), &mut zip).unwrap();
            zip.finish().unwrap();
        }

        let dst = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dst.path());
        let outcome = restore_backup(
            &paths,
            &archive,
            Packing::default(),
            None,
            None,
            ru(),
            |_| {},
        )
        .unwrap();
        assert!(matches!(outcome, RestoreOutcome::Restored { .. }));

        let restored = dst.path().join("data.db");
        assert!(
            fs::metadata(&restored).unwrap().len() < fragmented.len() as u64,
            "the restored database should be compacted"
        );
        let db = crate::shared::storage::db::Db::open(&restored).unwrap();
        assert_eq!(db.rag_search(profile, &[1.0, 0.0], 5).unwrap().len(), 1);
    }

    #[test]
    fn restore_leaves_an_uncompactable_database_alone() {
        // Compaction is best effort: a corrupt/foreign `data.db` in the archive
        // must be restored as-is, not turned into a failed restore.
        let src = tempfile::tempdir().unwrap();
        seed_data(src.path());
        let archive = src.path().join("snap.zip");
        create_backup(
            &Paths::with_root(src.path()),
            Some(archive.clone()),
            Packing::at(0),
            None,
            None,
            ru(),
            |_| {},
        )
        .unwrap();

        let dst = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dst.path());
        let outcome = restore_backup(
            &paths,
            &archive,
            Packing::default(),
            None,
            None,
            ru(),
            |_| {},
        )
        .unwrap();
        assert!(matches!(outcome, RestoreOutcome::Restored { .. }));
        assert_eq!(fs::read(dst.path().join("data.db")).unwrap(), b"SQLITE");
    }

    #[test]
    fn restore_does_not_extract_manifest_into_root() {
        let src = tempfile::tempdir().unwrap();
        seed_data(src.path());
        let out = create_backup(
            &Paths::with_root(src.path()),
            None,
            Packing::at(9),
            None,
            None,
            ru(),
            |_| {},
        )
        .unwrap();

        let dst = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dst.path());
        let outcome =
            restore_backup(&paths, &out, Packing::default(), None, None, ru(), |_| {}).unwrap();
        assert!(matches!(outcome, RestoreOutcome::Restored { .. }));
        // Data was restored, but the internal manifest didn't land in the root.
        assert!(dst.path().join("settings.json").exists());
        assert!(!dst.path().join(MANIFEST_NAME).exists());
    }

    // ---------- password-protected archives (spec §12.3) ----------

    const PW: &str = "correct horse battery staple";

    /// The point of the feature: the data is unreadable without the password.
    /// Asserted on the archive's **bytes**, not on an API refusal — a refusal
    /// would still pass if the content were sitting there in the clear.
    #[test]
    fn an_encrypted_backup_does_not_carry_readable_data() {
        let dir = tempfile::tempdir().unwrap();
        seed_data(dir.path());
        fs::write(
            dir.path().join("settings.json"),
            b"{\"secret\":\"HUNTER2-MARKER\"}",
        )
        .unwrap();
        let paths = Paths::with_root(dir.path());

        let out =
            create_backup(&paths, None, Packing::at(9), None, Some(PW), ru(), |_| {}).unwrap();

        // Level 0 (store) so the marker would be literally present if unencrypted —
        // deflate could otherwise hide it and make this test lie.
        let plain = create_backup(
            &paths,
            Some(dir.path().join("plain.zip")),
            Packing::at(0),
            None,
            None,
            ru(),
            |_| {},
        )
        .unwrap();
        let has_marker = |p: &Path| {
            fs::read(p)
                .unwrap()
                .windows(15)
                .any(|w| w == b"HUNTER2-MARKER\"".get(..15).unwrap_or(b"HUNTER2-MARKER"))
        };
        assert!(has_marker(&plain), "the control archive should be readable");
        assert!(
            !has_marker(&out),
            "plaintext leaked into the encrypted archive"
        );
        assert_eq!(check_password(&out, Some(PW)).unwrap(), ArchivePassword::Ok);
    }

    /// The four combinations of (archive encrypted?, password given?). The third
    /// row is the requirement's own wording: an unencrypted backup restores while
    /// a password is configured.
    #[test]
    fn password_matrix_covers_both_kinds_of_archive() {
        let dir = tempfile::tempdir().unwrap();
        seed_data(dir.path());
        let paths = Paths::with_root(dir.path());
        let enc = create_backup(
            &paths,
            Some(dir.path().join("enc.zip")),
            Packing::at(9),
            None,
            Some(PW),
            ru(),
            |_| {},
        )
        .unwrap();
        let plain = create_backup(
            &paths,
            Some(dir.path().join("plain.zip")),
            Packing::at(9),
            None,
            None,
            ru(),
            |_| {},
        )
        .unwrap();

        use ArchivePassword::*;
        for (archive, password, expected) in [
            (&enc, Some(PW), Ok),
            (&enc, None, Required),
            (&enc, Some("wrong"), Wrong),
            (&plain, Some(PW), NotNeeded),
            (&plain, None, NotNeeded),
        ] {
            assert_eq!(
                check_password(archive, password).unwrap(),
                expected,
                "archive={} password={password:?}",
                archive.display()
            );
        }
    }

    /// Round trip: an encrypted archive restores with the password, and an
    /// unencrypted one restores *while a password is held* — the second half is
    /// what the user asked for and would silently break if the password were
    /// pushed at the zip layer unconditionally in some future refactor.
    #[test]
    fn restore_accepts_an_encrypted_and_an_unencrypted_archive() {
        for password in [Some(PW), None] {
            let src = tempfile::tempdir().unwrap();
            seed_data(src.path());
            fs::write(src.path().join("settings.json"), b"{\"v\":42}").unwrap();
            let archive = src.path().join("snap.zip");
            create_backup(
                &Paths::with_root(src.path()),
                Some(archive.clone()),
                Packing::at(9),
                None,
                password,
                ru(),
                |_| {},
            )
            .unwrap();

            let dst = tempfile::tempdir().unwrap();
            let paths = Paths::with_root(dst.path());
            // The restore always holds the password — for the plain archive it
            // must simply be ignored.
            let outcome = restore_backup(
                &paths,
                &archive,
                Packing::default(),
                None,
                Some(PW),
                ru(),
                |_| {},
            )
            .unwrap();
            assert!(
                matches!(outcome, RestoreOutcome::Restored { .. }),
                "password={password:?}"
            );
            assert_eq!(
                fs::read(dst.path().join("settings.json")).unwrap(),
                b"{\"v\":42}",
                "password={password:?}"
            );
            assert!(dst.path().join("chats").join("a.json").exists());
        }
    }

    /// A wrong or missing password must be refused **before** anything is
    /// deleted — the transactional guarantee. Without the pre-flight check the
    /// data would already be cleared by the time unpacking failed.
    #[test]
    fn a_bad_password_is_refused_without_touching_data() {
        let src = tempfile::tempdir().unwrap();
        seed_data(src.path());
        let archive = src.path().join("enc.zip");
        create_backup(
            &Paths::with_root(src.path()),
            Some(archive.clone()),
            Packing::at(9),
            None,
            Some(PW),
            ru(),
            |_| {},
        )
        .unwrap();

        for password in [None, Some("wrong")] {
            let dst = tempfile::tempdir().unwrap();
            seed_data(dst.path());
            fs::write(dst.path().join("settings.json"), b"{\"from\":\"original\"}").unwrap();
            let paths = Paths::with_root(dst.path());

            let err = restore_backup(
                &paths,
                &archive,
                Packing::default(),
                None,
                password,
                ru(),
                |_| {},
            );
            assert!(err.is_err(), "password={password:?} should be refused");
            // Nothing was cleared, and no pre-restore copy was even made.
            assert_eq!(
                fs::read(dst.path().join("settings.json")).unwrap(),
                b"{\"from\":\"original\"}"
            );
            assert!(dst.path().join("chats").join("a.json").exists());
            assert!(
                fs::read_dir(paths.backups_dir()).is_ok_and(|mut d| d.next().is_none()),
                "a refused restore should not leave a pre-restore copy"
            );
        }
    }

    /// The manifest stays readable without the password, so the "backup from a
    /// newer version" warning still works on an encrypted archive.
    #[test]
    fn manifest_is_readable_without_the_password() {
        let dir = tempfile::tempdir().unwrap();
        seed_data(dir.path());
        let out = create_backup(
            &Paths::with_root(dir.path()),
            None,
            Packing::at(9),
            None,
            Some(PW),
            ru(),
            |_| {},
        )
        .unwrap();

        let m = read_manifest(&out)
            .unwrap()
            .expect("the manifest should be readable with no password");
        assert_eq!(m.app_version, env!("CARGO_PKG_VERSION"));
        // …while the data entries around it are genuinely encrypted.
        assert_eq!(
            check_password(&out, None).unwrap(),
            ArchivePassword::Required
        );
    }

    /// An empty password means "no encryption" — clearing the setting returns to
    /// plain archives rather than encrypting with an empty string.
    #[test]
    fn an_empty_password_produces_a_plain_archive() {
        let dir = tempfile::tempdir().unwrap();
        seed_data(dir.path());
        let out = create_backup(
            &Paths::with_root(dir.path()),
            None,
            Packing::at(9),
            None,
            Some(""),
            ru(),
            |_| {},
        )
        .unwrap();
        assert_eq!(
            check_password(&out, None).unwrap(),
            ArchivePassword::NotNeeded
        );
    }

    /// The pre-restore copy is encrypted with the run's effective password, so
    /// restoring an encrypted backup can't quietly write the old data out in the
    /// clear beside it (docs/history/backup-password.md §4 F3).
    #[test]
    fn the_pre_restore_copy_inherits_the_password() {
        let src = tempfile::tempdir().unwrap();
        seed_data(src.path());
        let archive = src.path().join("enc.zip");
        create_backup(
            &Paths::with_root(src.path()),
            Some(archive.clone()),
            Packing::at(9),
            None,
            Some(PW),
            ru(),
            |_| {},
        )
        .unwrap();

        let dst = tempfile::tempdir().unwrap();
        seed_data(dst.path());
        let paths = Paths::with_root(dst.path());
        let RestoreOutcome::Restored {
            pre_restore: Some(pre),
        } = restore_backup(
            &paths,
            &archive,
            Packing::default(),
            None,
            Some(PW),
            ru(),
            |_| {},
        )
        .unwrap()
        else {
            panic!("expected a Restored with a pre-restore copy");
        };
        assert_eq!(
            check_password(&pre, None).unwrap(),
            ArchivePassword::Required
        );
        assert_eq!(check_password(&pre, Some(PW)).unwrap(), ArchivePassword::Ok);
    }

    /// Corruption of an encrypted entry is caught rather than silently yielding
    /// wrong data: the AES layer authenticates the ciphertext.
    #[test]
    fn a_corrupted_encrypted_entry_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        seed_data(dir.path());
        let out = create_backup(
            &Paths::with_root(dir.path()),
            None,
            Packing::at(0),
            None,
            Some(PW),
            ru(),
            |_| {},
        )
        .unwrap();

        let mut bytes = fs::read(&out).unwrap();
        // Inside the first entry's payload: past its local header, before the
        // second entry's signature.
        let second = (4..bytes.len() - 4)
            .find(|&i| &bytes[i..i + 4] == b"PK\x03\x04")
            .expect("more than one entry");
        bytes[second - 5] ^= 0xff;
        let corrupt = dir.path().join("corrupt.zip");
        fs::write(&corrupt, &bytes).unwrap();

        let dst = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dst.path());
        // Either the pre-flight check or the extraction rejects it — what must
        // never happen is a silent success with mangled content.
        let refused = match restore_backup(
            &paths,
            &corrupt,
            Packing::default(),
            None,
            Some(PW),
            ru(),
            |_| {},
        ) {
            Err(_) => true,
            Ok(RestoreOutcome::Restored { .. }) => false,
            Ok(_) => true,
        };
        assert!(refused, "corrupted ciphertext was accepted");
    }

    /// An encrypted backup written through `zip` 2.4.2 — the version behind
    /// every archive made up to 0.13.0 — by [`create_backup`] itself, at level 6
    /// with [`PINNED_PW`]: `settings.json`, `chats/a.json`, an empty
    /// `files/c1/empty.txt` and the plain manifest. Base64 of the 778-byte file.
    const ARCHIVE_FROM_ZIP_2_4_2: &str = "\
        UEsDBDMAAQBjAAAAIQAAAAAAJgAAAAgAAAANAAsAc2V0dGluZ3MuanNvbgGZBwACAEFFAwgAWCTv\
        CSTnd4tOkxtLq7kOmWB0IJMQl8CJHSwZBizxgKd9KHG9nNFQSwMEMwABAGMAAAAhAMk5nhlOAAAA\
        NwAAAAwACwBjaGF0cy9hLmpzb24BmQcAAQBBRQMIAPr0x0CYPYyDViCp89HL0xJkZI6/nNyOTySp\
        IJuyzaGmldGVnNgjaIsQ/3tuxHLjOgwGQtfmI35aa2jmScvXMyodxWRhJFSOuBTuCSLmZ1BLAwQz\
        AAEAYwAAACEAAAAAAB4AAAAAAAAAEgALAGZpbGVzL2MxL2VtcHR5LnR4dAGZBwACAEFFAwgA+mpk\
        0JTWBOt2VfKORuc5WkaMMdDr66l5qLOFrLUSUEsDBBQAAAAIAAAAIQAUX3ufgAAAAKkAAAANAAAA\
        bWFuaWZlc3QuanNvbk3MQQ6CMBAF0D2nILMVmpmiWHsO96SWUZooNG3jhnB3W3Thbub9n79WdQ3G\
        ++HNIbplBl0DCuoEQlOiaCd+mZh5zW8BTsnNjyLH5ks+LHf35EL0IzuZ9NcYbyXL97aP2sAm8Tjs\
        HZAo+5awRbpK1NhrOgtF6nSRCvGAnUaEavsAUEsBAjMDMwABAGMAAAAhAAAAAAAmAAAACAAAAA0A\
        CwAAAAAAAAAAAKSBAAAAAHNldHRpbmdzLmpzb24BmQcAAgBBRQMIAFBLAQIzAzMAAQBjAAAAIQDJ\
        OZ4ZTgAAADcAAAAMAAsAAAAAAAAAAACkgVwAAABjaGF0cy9hLmpzb24BmQcAAQBBRQMIAFBLAQIz\
        AzMAAQBjAAAAIQAAAAAAHgAAAAAAAAASAAsAAAAAAAAAAACkgd8AAABmaWxlcy9jMS9lbXB0eS50\
        eHQBmQcAAgBBRQMIAFBLAQIUAxQAAAAIAAAAIQAUX3ufgAAAAKkAAAANAAAAAAAAAAAAAACkgTgB\
        AABtYW5pZmVzdC5qc29uUEsFBgAAAAAEAAQAEQEAAOMBAAAAAA==";
    const PINNED_PW: &str = "pinned pass-phrase";

    /// Gate: a backup the user already has still opens after the `zip` crate
    /// moves.
    ///
    /// Every other test here writes its archive and reads it back inside one
    /// run, through one version of the crate — so a reader that stopped
    /// understanding what an older writer produced would pass all of them and
    /// fail only on the archive a user kept for the day they need it. This one
    /// reads bytes that no longer change: pinned on 2.4.2, read unchanged
    /// across the move to 8.6.0.
    #[test]
    fn a_backup_written_by_an_older_zip_still_restores() {
        use base64::Engine as _;

        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("pinned.zip");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(ARCHIVE_FROM_ZIP_2_4_2)
            .expect("the pinned archive is base64");
        fs::write(&archive, bytes).unwrap();

        use ArchivePassword::*;
        for (password, expected) in [
            (Some(PINNED_PW), Ok),
            (None, Required),
            (Some("wrong"), Wrong),
        ] {
            assert_eq!(
                check_password(&archive, password).unwrap(),
                expected,
                "password={password:?}"
            );
        }
        let manifest = read_manifest(&archive).unwrap().expect("a manifest");
        assert_eq!(manifest.app_version, "0.13.0");

        // Read where it lies — the `stats` road...
        let mut reader = ArchiveReader::open(&archive, Some(PINNED_PW), ru()).unwrap();
        let mut names: Vec<String> = reader.entries().into_iter().map(|e| e.name).collect();
        names.sort();
        assert_eq!(
            names,
            [
                "chats/a.json",
                "files/c1/empty.txt",
                "manifest.json",
                "settings.json"
            ]
        );

        // ...and restored.
        let dst = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dst.path());
        let outcome = restore_backup(
            &paths,
            &archive,
            Packing::default(),
            None,
            Some(PINNED_PW),
            ru(),
            |_| {},
        )
        .unwrap();
        assert!(matches!(outcome, RestoreOutcome::Restored { .. }));
        assert_eq!(
            fs::read(dst.path().join("settings.json")).unwrap(),
            b"{\"v\":42}"
        );
        assert_eq!(
            fs::read(dst.path().join("chats").join("a.json")).unwrap(),
            b"{\"title\":\"pinned\",\"messages\":[\"one\",\"one\",\"one\",\"one\"]}"
        );
        assert_eq!(
            fs::read(dst.path().join("files").join("c1").join("empty.txt")).unwrap(),
            b""
        );
    }

    // ---- the worker pool (spec §12.3, docs/research/backup-speed.md) ----

    /// Every file entry of an archive as (name, bytes), sorted by name, the
    /// manifest left out — what two archives of one root must agree on.
    fn archive_contents(archive: &Path, password: Option<&str>) -> Vec<(String, Vec<u8>)> {
        let mut zip = ZipArchive::new(File::open(archive).unwrap()).unwrap();
        let mut out: Vec<(String, Vec<u8>)> = (0..zip.len())
            .filter_map(|i| {
                let mut entry = match password {
                    Some(pw) => zip.by_index_decrypt(i, pw.as_bytes()).unwrap(),
                    None => zip.by_index(i).unwrap(),
                };
                let name = entry_name(&entry);
                if name == MANIFEST_NAME || entry.is_dir() {
                    return None;
                }
                let mut bytes = Vec::new();
                io::Read::read_to_end(&mut entry, &mut bytes).unwrap();
                Some((name, bytes))
            })
            .collect();
        out.sort();
        out
    }

    /// The names in the archive's own order, read without the password.
    fn raw_names(archive: &Path) -> Vec<String> {
        let mut zip = ZipArchive::new(File::open(archive).unwrap()).unwrap();
        (0..zip.len())
            .map(|i| entry_name(&zip.by_index_raw(i).unwrap()))
            .collect()
    }

    /// Every file under `root` as (relative path, bytes), `backups/` and `logs/`
    /// left out.
    fn tree(root: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
        fn walk(root: &Path, dir: &Path, out: &mut std::collections::BTreeMap<String, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if rel == "backups" || rel == "logs" {
                    continue;
                }
                if path.is_dir() {
                    walk(root, &path, out);
                } else {
                    out.insert(rel, fs::read(&path).unwrap());
                }
            }
        }
        let mut out = std::collections::BTreeMap::new();
        walk(root, root, &mut out);
        out
    }

    /// A root that keeps a pool busy: the usual seed, a real database to compact,
    /// and forty chats of forty sizes.
    fn seed_many(root: &Path) {
        seed_data(root);
        fs::remove_file(root.join("data.db")).unwrap();
        seed_fragmented_db(root);
        for i in 0..40 {
            let body = format!(
                "{{\"v\":1,\"n\":{i},\"text\":\"{}\"}}",
                "word ".repeat(i * 300)
            );
            fs::write(root.join("chats").join(format!("c{i:02}.json")), body).unwrap();
        }
    }

    /// What is left in `dir` besides `keep`: a scratch file the pool did not remove.
    fn leftovers(dir: &Path, keep: &[&str]) -> Vec<String> {
        let mut left: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| !keep.contains(&n.as_str()))
            .collect();
        left.sort();
        left
    }

    /// The pool's archive holds what one worker's does — every name and every
    /// byte, the compacted database included — with a password and without; the
    /// manifest is still the last entry, and still readable without the password.
    #[test]
    fn a_pool_packs_what_one_worker_packs() {
        let dir = tempfile::tempdir().unwrap();
        seed_many(dir.path());
        let paths = Paths::with_root(dir.path());
        let out = tempfile::tempdir().unwrap();
        for password in [None, Some(PW)] {
            let pack = |name: &str, threads| {
                let packing = Packing { level: 6, threads };
                let path = Some(out.path().join(name));
                create_backup(&paths, path, packing, None, password, ru(), |_| {}).unwrap()
            };
            let one = pack("one.zip", 1);
            let four = pack("four.zip", 4);
            let contents = archive_contents(&one, password);
            assert!(contents.len() > 40, "{}", contents.len());
            assert!(contents.iter().any(|(name, _)| name == "data.db"));
            assert_eq!(contents, archive_contents(&four, password), "{password:?}");
            for archive in [&one, &four] {
                assert_eq!(
                    raw_names(archive).last().map(String::as_str),
                    Some(MANIFEST_NAME)
                );
                let manifest = read_manifest(archive).unwrap().expect("a manifest");
                assert_eq!(manifest.schemas.db, schema::DB_SCHEMA, "{password:?}");
            }
            assert_eq!(
                leftovers(out.path(), &["one.zip", "four.zip"]),
                Vec::<String>::new()
            );
        }
    }

    /// An entry whose compressed copy waits in a scratch file packs to the same
    /// bytes as one held in memory, and the scratch file is gone afterwards.
    #[test]
    fn a_spilled_entry_packs_the_same_and_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        seed_many(dir.path());
        let paths = Paths::with_root(dir.path());
        let out = tempfile::tempdir().unwrap();
        let entries = gather_entries(&paths, None, ru()).unwrap();
        for (name, spill_over) in [("memory.zip", u64::MAX), ("spilled.zip", 0)] {
            let spec = ZipSpec {
                level: 6,
                password: Some(PW),
                threads: 4,
                spill_over,
            };
            let archive = out.path().join(name);
            parallel::write_archive(&paths, &archive, &entries, &spec, ru(), &mut |_| {}).unwrap();
        }
        assert_eq!(
            archive_contents(&out.path().join("memory.zip"), Some(PW)),
            archive_contents(&out.path().join("spilled.zip"), Some(PW))
        );
        assert_eq!(
            leftovers(out.path(), &["memory.zip", "spilled.zip"]),
            Vec::<String>::new()
        );
    }

    /// A file that cannot be read fails the whole backup with its name — the
    /// pool stops rather than hangs — and the scratch files of the entries
    /// packed before it are removed.
    #[test]
    fn an_unreadable_file_fails_the_pack_and_leaves_no_scratch() {
        let dir = tempfile::tempdir().unwrap();
        seed_many(dir.path());
        let paths = Paths::with_root(dir.path());
        let out = tempfile::tempdir().unwrap();
        let mut entries = gather_entries(&paths, None, ru()).unwrap();
        // Scheduled in the middle: some entries are spilled before it, some after.
        entries.push(Entry {
            abs: dir.path().join("chats").join("gone.json"),
            name: "chats/gone.json".into(),
            len: 3_000,
            mtime: None,
        });
        let spec = ZipSpec {
            level: 6,
            password: None,
            threads: 4,
            spill_over: 0,
        };
        let archive = out.path().join("x.zip");
        let err = parallel::write_archive(&paths, &archive, &entries, &spec, ru(), &mut |_| {})
            .unwrap_err();
        assert!(format!("{err:#}").contains("gone.json"), "{err:#}");
        assert_eq!(leftovers(out.path(), &["x.zip"]), Vec::<String>::new());
    }

    /// Unpacking on a pool puts back what one worker does, and both are the
    /// source's files (the database aside: restore compacts it in place).
    #[test]
    fn a_pool_unpacks_what_one_worker_unpacks() {
        let src = tempfile::tempdir().unwrap();
        seed_many(src.path());
        let work = tempfile::tempdir().unwrap();
        let archive = create_backup(
            &Paths::with_root(src.path()),
            Some(work.path().join("a.zip")),
            Packing::at(6),
            None,
            Some(PW),
            ru(),
            |_| {},
        )
        .unwrap();
        let restored = [1, 4].map(|threads| {
            let dst = tempfile::tempdir().unwrap();
            let packing = Packing { level: 6, threads };
            let paths = Paths::with_root(dst.path());
            let outcome = restore_backup(&paths, &archive, packing, None, Some(PW), ru(), |_| {});
            assert!(matches!(
                outcome.unwrap(),
                RestoreOutcome::Restored { pre_restore: None }
            ));
            tree(dst.path())
        });
        assert_eq!(restored[0], restored[1]);
        // What a backup leaves out by design (the module doc), and the database,
        // which a restore compacts in place.
        let packed = |name: &String| {
            !name.starts_with("story-logs/")
                && !name.starts_with("data.db")
                && name != "defaults.json"
                && name != "location.json"
        };
        let mut source = tree(src.path());
        source.retain(|name, _| packed(name));
        let mut unpacked = restored[0].clone();
        unpacked.retain(|name, _| packed(name));
        assert!(unpacked.len() > 40, "{}", unpacked.len());
        assert_eq!(unpacked, source);
    }

    /// An entry that cannot be written fails the unpacking with its path, on a
    /// pool as on one worker — the error a restore rolls back on.
    #[test]
    fn an_unwritable_entry_fails_the_pool_with_its_path() {
        let src = tempfile::tempdir().unwrap();
        seed_many(src.path());
        let work = tempfile::tempdir().unwrap();
        let archive = create_backup(
            &Paths::with_root(src.path()),
            Some(work.path().join("a.zip")),
            Packing::at(1),
            None,
            None,
            ru(),
            |_| {},
        )
        .unwrap();
        let dst = tempfile::tempdir().unwrap();
        // A directory where a chat's file should go.
        fs::create_dir_all(dst.path().join("chats").join("c39.json")).unwrap();
        let paths = Paths::with_root(dst.path());
        let err =
            parallel::extract_archive(&paths, &archive, None, 4, ru(), &mut |_| {}).unwrap_err();
        assert!(format!("{err:#}").contains("c39.json"), "{err:#}");
    }

    /// The copy a restore takes first is packed at the level it is given — the
    /// settings' (F3) — not at a level of its own: stored at 0, deflated above.
    #[test]
    fn the_pre_restore_copy_is_packed_at_the_given_level() {
        let src = tempfile::tempdir().unwrap();
        seed_data(src.path());
        let work = tempfile::tempdir().unwrap();
        let archive = create_backup(
            &Paths::with_root(src.path()),
            Some(work.path().join("a.zip")),
            Packing::at(6),
            None,
            None,
            ru(),
            |_| {},
        )
        .unwrap();
        for (level, method) in [
            (0, CompressionMethod::Stored),
            (6, CompressionMethod::Deflated),
        ] {
            let dst = tempfile::tempdir().unwrap();
            seed_data(dst.path());
            let paths = Paths::with_root(dst.path());
            let outcome = restore_backup(
                &paths,
                &archive,
                Packing::at(level),
                None,
                None,
                ru(),
                |_| {},
            );
            let RestoreOutcome::Restored {
                pre_restore: Some(copy),
            } = outcome.unwrap()
            else {
                panic!("expected a pre-restore copy");
            };
            let mut zip = ZipArchive::new(File::open(&copy).unwrap()).unwrap();
            let methods: Vec<_> = (0..zip.len())
                .filter_map(|i| {
                    let e = zip.by_index_raw(i).unwrap();
                    (entry_name(&e) != MANIFEST_NAME && e.size() > 0).then(|| e.compression())
                })
                .collect();
            assert!(!methods.is_empty());
            assert!(
                methods.iter().all(|m| *m == method),
                "level {level}: {methods:?}"
            );
        }
    }

    /// The level and the thread count come out of the raw settings each on its
    /// own: absent — the defaults; a level above 9 — 9; a value that is not a
    /// whole non-negative number — that value's default, the other kept.
    #[test]
    fn the_level_and_threads_are_read_from_the_raw_settings() {
        let read = |json: &str| archive_settings(&serde_json::from_str(json).unwrap());
        assert_eq!(read("{}"), BackupSettings::default());
        assert_eq!(read(r#"{"backup":{}}"#), BackupSettings::default());
        assert_eq!(
            read(r#"{"backup":{"compression":2,"threads":3}}"#),
            BackupSettings {
                compression: 2,
                threads: 3
            }
        );
        assert_eq!(read(r#"{"backup":{"compression":42}}"#).compression, 9);
        assert_eq!(
            read(r#"{"backup":{"compression":"fast","threads":2}}"#),
            BackupSettings {
                compression: 6,
                threads: 2
            }
        );
        assert_eq!(
            read(r#"{"backup":{"compression":1,"threads":-1}}"#),
            BackupSettings {
                compression: 1,
                threads: 0
            }
        );
        assert_eq!(
            read(r#"{"backup":{"threads":99999999999}}"#).threads,
            u32::MAX
        );
    }

    /// `0` is every core, and no number is more than the cores (F4).
    #[test]
    fn the_worker_count_never_passes_the_cores() {
        let cores = machine_cores();
        assert!(cores >= 1);
        assert_eq!(worker_count(0), cores);
        assert_eq!(worker_count(1), 1);
        assert_eq!(worker_count(u32::try_from(cores + 1).unwrap()), cores);
        assert_eq!(worker_count(u32::MAX), cores);
        assert_eq!(Packing::default().threads, cores);
        assert_eq!(Packing::default().level, 6);
    }

    // ---- the entries' dates (spec §12.3, `dates`) ----

    /// Dates `path` at `mtime`, as an editor that saved it then would have.
    fn set_date(path: &Path, mtime: SystemTime) {
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
    }

    fn date_of(path: &Path) -> SystemTime {
        fs::metadata(path).unwrap().modified().unwrap()
    }

    /// Each entry's date, by name, as the archive says it.
    fn entry_dates(archive: &Path) -> std::collections::HashMap<String, Option<SystemTime>> {
        let mut zip = ZipArchive::new(File::open(archive).unwrap()).unwrap();
        (0..zip.len())
            .map(|i| {
                let entry = zip.by_index_raw(i).unwrap();
                (entry_name(&entry), dates::entry_mtime(&entry))
            })
            .collect()
    }

    /// An entry carries its file's date — held in memory or spilled, encrypted
    /// or not — the compacted database carries the newest of the database's
    /// files, the manifest the time it was written; and a restore gives every
    /// file its date back, a stored file's after its origin mark. The date is the
    /// DOS one (`dates`: `zip` drops the exact one from a raw copy), so an odd
    /// second comes back as the even one below it.
    #[test]
    fn an_archive_carries_its_files_dates_and_a_restore_gives_them_back() {
        let dir = tempfile::tempdir().unwrap();
        seed_many(dir.path());
        let odd = SystemTime::UNIX_EPOCH + Duration::from_secs(1_715_000_001);
        let chat = SystemTime::UNIX_EPOCH + Duration::from_secs(1_715_000_000);
        let picture = SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        let database = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        set_date(&dir.path().join("chats").join("c07.json"), odd);
        set_date(
            &dir.path().join("files").join("c1").join("chart.png"),
            picture,
        );
        for name in DB_FILES {
            let path = dir.path().join(name);
            if path.is_file() {
                set_date(&path, database);
            }
        }
        let paths = Paths::with_root(dir.path());
        let entries = gather_entries(&paths, None, ru()).unwrap();
        let out = tempfile::tempdir().unwrap();
        let started = SystemTime::now() - Duration::from_secs(2);
        for (name, spill_over) in [("memory.zip", u64::MAX), ("spilled.zip", 0)] {
            let archive = out.path().join(name);
            let spec = ZipSpec {
                level: 6,
                password: Some(PW),
                threads: 4,
                spill_over,
            };
            parallel::write_archive(&paths, &archive, &entries, &spec, ru(), &mut |_| {}).unwrap();
            let dates = entry_dates(&archive);
            assert_eq!(dates["chats/c07.json"], Some(chat), "{name}");
            assert_eq!(dates["files/c1/chart.png"], Some(picture), "{name}");
            assert_eq!(dates["data.db"], Some(database), "{name}");
            let manifest = dates[MANIFEST_NAME].expect("the manifest is dated");
            assert!(manifest >= started, "{name}: {manifest:?}");
            assert!(
                dates.values().all(Option::is_some),
                "{name}: an undated entry"
            );

            let dst = tempfile::tempdir().unwrap();
            let restored = Paths::with_root(dst.path());
            let packing = Packing {
                level: 6,
                threads: 4,
            };
            restore_backup(&restored, &archive, packing, None, Some(PW), ru(), |_| {}).unwrap();
            assert_eq!(date_of(&dst.path().join("chats").join("c07.json")), chat);
            assert_eq!(
                date_of(&dst.path().join("files").join("c1").join("chart.png")),
                picture
            );
        }
    }

    /// An archive from before entries were dated carries the format's default,
    /// 1980-01-01, which reads as no date: its files are dated when they are
    /// restored, as they always were — not set back to 1980.
    #[test]
    fn an_undated_archive_restores_its_files_dated_now() {
        use std::io::Write;
        let work = tempfile::tempdir().unwrap();
        let archive = work.path().join("old.zip");
        {
            let mut zip = ZipWriter::new(File::create(&archive).unwrap());
            zip.start_file("settings.json", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"{}").unwrap();
            zip.finish().unwrap();
        }
        assert_eq!(entry_dates(&archive)["settings.json"], None);
        let started = SystemTime::now() - Duration::from_secs(2);
        let dst = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dst.path());
        restore_backup(&paths, &archive, Packing::at(6), None, None, ru(), |_| {}).unwrap();
        assert!(date_of(&dst.path().join("settings.json")) >= started);
    }

    // ---- the restore's compaction (docs/research/backup-big-entry.md F3) ----

    /// An archive this app wrote holds a compacted database: no free page, no
    /// sidecar. Its restore leaves the file as it was unpacked — byte for byte
    /// the archive's — and says nothing of compacting after the unpacking.
    #[test]
    fn a_restore_of_our_own_archive_leaves_the_database_as_unpacked() {
        let src = tempfile::tempdir().unwrap();
        seed_data(src.path());
        fs::remove_file(src.path().join("data.db")).unwrap();
        seed_fragmented_db(src.path());
        let archive = create_backup(
            &Paths::with_root(src.path()),
            Some(src.path().join("snap.zip")),
            Packing::at(6),
            None,
            None,
            ru(),
            |_| {},
        )
        .unwrap();
        let mut packed = Vec::new();
        let mut zip = ZipArchive::new(File::open(&archive).unwrap()).unwrap();
        io::Read::read_to_end(&mut zip.by_name("data.db").unwrap(), &mut packed).unwrap();

        let dst = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dst.path());
        let mut log = Vec::new();
        restore_backup(&paths, &archive, Packing::at(6), None, None, ru(), |m| {
            log.push(m.to_string())
        })
        .unwrap();
        assert_eq!(fs::read(paths.data_db()).unwrap(), packed, "rewritten");
        let unpacked = step_at(&log, "backup.progress.extracting");
        assert!(
            !log[unpacked..].contains(&ru().t("backup.progress.compacting").to_string()),
            "{log:#?}"
        );
    }

    /// What decides the in-place `VACUUM`: free pages in the header, or a `-wal`
    /// beside the file whose pages the header does not know of. No database, a
    /// compacted one, and a file that is not a database are each left alone.
    #[test]
    fn a_restored_database_is_compacted_only_when_there_is_something_to_give_back() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        assert!(!restored_db_needs_compaction(&paths), "no database");

        seed_fragmented_db(dir.path());
        assert!(restored_db_needs_compaction(&paths), "free pages");

        let compact = dir.path().join("compact.db");
        db::vacuum_into(&paths.data_db(), &compact).unwrap();
        fs::rename(&compact, paths.data_db()).unwrap();
        assert!(!restored_db_needs_compaction(&paths), "compacted");

        fs::write(dir.path().join("data.db-wal"), b"pages").unwrap();
        assert!(restored_db_needs_compaction(&paths), "a WAL beside it");
        fs::write(dir.path().join("data.db-wal"), b"").unwrap();
        assert!(!restored_db_needs_compaction(&paths), "an empty WAL");

        fs::write(paths.data_db(), b"SQLITE").unwrap();
        assert!(!restored_db_needs_compaction(&paths), "not a database");
    }
}

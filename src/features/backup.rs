//! Backup and restore of user data into a zip archive.
//!
//! Launched via command-line arguments (`--backup` / `--restore`) with no TUI,
//! and ends the process (see `main.rs`). Archive contents (paths in the archive
//! are relative to the data root):
//! - files `settings.json`, `profiles.json`, `data.db` (+ sidecar `-wal`/`-shm`,
//!   if present — see the compaction note below), `personal_dictionary.txt`;
//! - directories `chats/`, `dictionaries/`, and `locales/` (recursively — their
//!   `*.bak` files are pulled in too; `locales/` — user overrides of the
//!   scaffold/UI text);
//! - all `*.bak` at the root (`settings.bak`, `profiles.bak`);
//! - the file-tools "sandbox" directory (`config.tools.fs_root`) — **only if**
//!   it lies inside the data root.
//!
//! Excluded: `backups/`, `logs/`, and the install-defaults files `defaults.json`/
//! `location.json` (they're about the install, not user data). Additionally, a
//! `manifest.json` (schema versions + app version) is written into the archive —
//! metadata for warning on restoring a backup made by a newer version; it is
//! **not** extracted into the root. See [`BackupManifest`].
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

use std::collections::HashSet;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Local;
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{AesMode, CompressionMethod, ZipArchive, ZipWriter};

use crate::shared::i18n::Locale;
use crate::shared::paths::Paths;
use crate::shared::storage::db;
use crate::shared::storage::schema::{CHAT_SCHEMA, DB_SCHEMA, PROFILES_SCHEMA, SETTINGS_SCHEMA};

/// Manifest file name inside the archive (schema-version metadata; not
/// extracted into the root — read separately by [`read_manifest`]). See
/// release-engineering.md §3.4.
const MANIFEST_NAME: &str = "manifest.json";

/// Data schema versions at backup creation time (release-engineering.md, the manifest deliverable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaVersions {
    pub settings: u32,
    pub profiles: u32,
    pub chat: u32,
    pub db: u32,
}

/// Backup manifest (`manifest.json` in the archive): app version, schema
/// versions, and creation time. Needed so that when restoring a backup made
/// by a **newer** mindfork version, the user gets a warning (data is intact;
/// the startup downgrade guard protects it regardless — ADR 0006).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupManifest {
    pub app_version: String,
    pub schemas: SchemaVersions,
    pub created_at: String,
}

impl BackupManifest {
    /// Manifest for the current build (app version + current schema versions).
    fn current() -> Self {
        Self {
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            schemas: SchemaVersions {
                settings: SETTINGS_SCHEMA,
                profiles: PROFILES_SCHEMA,
                chat: CHAT_SCHEMA,
                db: DB_SCHEMA,
            },
            created_at: Local::now().to_rfc3339(),
        }
    }

    /// Does the manifest carry a schema **newer** than current (an archive
    /// from a newer app version) — a signal to warn on restore.
    pub fn is_newer_than_current(&self) -> bool {
        self.schemas.settings > SETTINGS_SCHEMA
            || self.schemas.profiles > PROFILES_SCHEMA
            || self.schemas.chat > CHAT_SCHEMA
            || self.schemas.db > DB_SCHEMA
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
const TOP_DIRS: &[&str] = &["chats", "dictionaries", "locales"];

/// A packing entry: the source's absolute path + its name inside the archive (with `/`).
struct Entry {
    abs: PathBuf,
    name: String,
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

/// Creates a backup of user data.
///
/// `output` — path to the archive to create (`None` → an auto-name in
/// `backups/`). `level` — compression level `0..=9` (`0` → no compression,
/// store). `fs_root` — the file-tool sandbox directory from the config
/// (included only when it lies inside the data root). `password` — encrypts
/// every data entry with AES-256 (`None`/empty → a plain archive). Returns the
/// path to the created archive.
pub fn create_backup(
    paths: &Paths,
    output: Option<PathBuf>,
    level: i64,
    fs_root: Option<&Path>,
    password: Option<&str>,
    loc: &Locale,
) -> Result<PathBuf> {
    let out_path = match output {
        Some(p) => p,
        None => default_backup_path(paths, "mindfork-backup"),
    };
    // A compacted copy is packed in place of the live database (see the module
    // doc); `None` — there is none, or it couldn't be compacted, and the raw
    // files go in instead. The scratch file lives until the archive is written.
    let compact = compacted_db(paths, &out_path);
    let entries = gather_entries(paths, fs_root, compact.as_ref().map(TempDb::path), loc)?;
    write_zip(&out_path, &entries, level, normalize(password), loc).with_context(|| {
        loc.tf(
            "backup.ctx.create_archive",
            &[("path", &out_path.display().to_string())],
        )
    })?;
    Ok(out_path)
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
pub fn restore_backup(
    paths: &Paths,
    archive: &Path,
    fs_root: Option<&Path>,
    password: Option<&str>,
    loc: &Locale,
) -> Result<RestoreOutcome> {
    let password = normalize(password);
    // 1. Validate the archive before any destructive action.
    validate_archive(archive, password, loc).with_context(|| {
        loc.tf(
            "backup.ctx.validate",
            &[("path", &archive.display().to_string())],
        )
    })?;

    // 2. Auto-copy of the prior data, if any.
    let pre_restore = if has_existing_data(paths) {
        let path = create_backup(
            paths,
            Some(default_backup_path(paths, "pre-restore")),
            9,
            fs_root,
            password,
            loc,
        )
        .with_context(|| loc.t("backup.ctx.pre_restore").to_string())?;
        Some(path)
    } else {
        None
    };

    // 3. Clear + unpack.
    let attempt = (|| -> Result<()> {
        clear_user_data(paths, fs_root, loc)?;
        extract_archive(paths, archive, password, loc)
    })();

    match attempt {
        Ok(()) => {
            // 3a. Compact what was unpacked. Deliberately after the attempt
            // rather than inside it: the data is already in place and correct,
            // so a compaction failure must not turn a successful restore into a
            // rollback (the rollback path unpacks a pre-restore copy, which
            // `create_backup` already compacted).
            compact_restored_db(paths);
            Ok(RestoreOutcome::Restored { pre_restore })
        }
        Err(restore_error) => match &pre_restore {
            // 4. Roll back to the just-created pre-restore copy.
            Some(backup) => {
                let rollback = (|| -> Result<()> {
                    clear_user_data(paths, fs_root, loc)?;
                    extract_archive(paths, backup, password, loc)
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

/// Gathers the list of files to pack (deduplicated by archive name).
///
/// `compact_db` — a compacted copy of `data.db` to pack under that name instead
/// of the live file; when it is `Some`, the `-wal`/`-shm` sidecars are skipped
/// too (their content is already folded into the copy).
fn gather_entries(
    paths: &Paths,
    fs_root: Option<&Path>,
    compact_db: Option<&Path>,
    loc: &Locale,
) -> Result<Vec<Entry>> {
    let root = paths.root();
    let mut out: Vec<Entry> = Vec::new();

    let raw_db: &[&str] = if compact_db.is_some() { &[] } else { DB_FILES };
    for f in TOP_FILES.iter().chain(raw_db) {
        let abs = root.join(f);
        if abs.is_file() {
            out.push(Entry {
                abs,
                name: (*f).to_string(),
            });
        }
    }
    if let Some(compact) = compact_db {
        out.push(Entry {
            abs: compact.to_path_buf(),
            name: "data.db".to_string(),
        });
    }

    // All top-level `*.bak` files (settings.bak, profiles.bak, etc.).
    if let Ok(rd) = fs::read_dir(root) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_file()
                && path.extension().is_some_and(|e| e == "bak")
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
            {
                out.push(Entry {
                    abs: path.clone(),
                    name: name.to_string(),
                });
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

/// A compacted copy of `data.db`, packed in place of the live file and removed
/// on drop — including after a failed `VACUUM INTO`, which can leave a partial
/// file behind.
struct TempDb(PathBuf);

impl TempDb {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDb {
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
fn compacted_db(paths: &Paths, out_path: &Path) -> Option<TempDb> {
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
    let temp = TempDb(dir.join(format!("data.db.compact-{}.tmp", std::process::id())));
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
            out.push(Entry {
                abs: child_abs,
                name: child_name,
            });
        }
    }
    Ok(())
}

/// Writes the archive from the entry list at the given compression level,
/// encrypting the data entries when `password` is set (the manifest stays
/// readable — see the module doc).
fn write_zip(
    out_path: &Path,
    entries: &[Entry],
    level: i64,
    password: Option<&str>,
    loc: &Locale,
) -> Result<()> {
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            loc.tf(
                "backup.ctx.create_dir",
                &[("path", &parent.display().to_string())],
            )
        })?;
    }
    let file = File::create(out_path).with_context(|| {
        loc.tf(
            "backup.ctx.create_file",
            &[("path", &out_path.display().to_string())],
        )
    })?;
    let mut zip = ZipWriter::new(file);

    let level = level.clamp(0, 9);
    let plain = if level == 0 {
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored)
    } else {
        SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(level))
    };
    // Data entries: encrypted when a password is set. The manifest always uses
    // `plain` — it carries no user data and stays readable without the password.
    let options = match password {
        Some(pw) => plain.with_aes_encryption(AesMode::Aes256, pw),
        None => plain,
    };

    for e in entries {
        zip.start_file(e.name.as_str(), options)
            .with_context(|| loc.tf("backup.ctx.write_entry", &[("name", &e.name)]))?;
        let mut src = File::open(&e.abs).with_context(|| {
            loc.tf("backup.ctx.open", &[("path", &e.abs.display().to_string())])
        })?;
        io::copy(&mut src, &mut zip).with_context(|| {
            loc.tf("backup.ctx.pack", &[("path", &e.abs.display().to_string())])
        })?;
    }

    // The schema-version manifest (metadata, not a user file) — written last.
    let manifest = serde_json::to_vec_pretty(&BackupManifest::current())
        .context("serializing backup manifest")?;
    zip.start_file(MANIFEST_NAME, plain)
        .with_context(|| loc.tf("backup.ctx.write_entry", &[("name", MANIFEST_NAME)]))?;
    io::copy(&mut manifest.as_slice(), &mut zip)
        .with_context(|| loc.tf("backup.ctx.pack", &[("path", MANIFEST_NAME)]))?;

    zip.finish()
        .with_context(|| loc.t("backup.ctx.finalize").to_string())?;
    Ok(())
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
                loc.tf("backup.err.unsafe_entry", &[("name", entry.name())])
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

/// Unpacks the archive into the data root (entry names are already
/// considered safe — `enclosed_name` rejects escaping outside the root).
///
/// A `password` given for an unencrypted archive is harmlessly discarded by the
/// zip layer, so one code path restores both kinds.
fn extract_archive(
    paths: &Paths,
    archive: &Path,
    password: Option<&str>,
    loc: &Locale,
) -> Result<()> {
    let file = File::open(archive).with_context(|| {
        loc.tf(
            "backup.ctx.open_archive",
            &[("path", &archive.display().to_string())],
        )
    })?;
    let mut zip =
        ZipArchive::new(file).with_context(|| loc.t("backup.ctx.read_archive").to_string())?;
    for i in 0..zip.len() {
        let mut entry = match password {
            Some(pw) => zip.by_index_decrypt(i, pw.as_bytes())?,
            None => zip.by_index(i)?,
        };
        let rel = entry.enclosed_name().ok_or_else(|| {
            anyhow!(
                "{}",
                loc.tf("backup.err.unsafe_entry", &[("name", entry.name())])
            )
        })?;
        // The manifest is archive metadata, not user data: not written into the root.
        if rel == Path::new(MANIFEST_NAME) {
            continue;
        }
        let dest = paths.root().join(&rel);
        if entry.is_dir() {
            fs::create_dir_all(&dest).with_context(|| {
                loc.tf(
                    "backup.ctx.create_dir",
                    &[("path", &dest.display().to_string())],
                )
            })?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).with_context(|| {
                loc.tf(
                    "backup.ctx.create_dir",
                    &[("path", &parent.display().to_string())],
                )
            })?;
        }
        let mut out = File::create(&dest).with_context(|| {
            loc.tf(
                "backup.ctx.create_file",
                &[("path", &dest.display().to_string())],
            )
        })?;
        io::copy(&mut entry, &mut out).with_context(|| {
            loc.tf(
                "backup.ctx.extract",
                &[("path", &dest.display().to_string())],
            )
        })?;
    }
    Ok(())
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

/// Auto-name for an archive in `backups/`: `<prefix>-YYYYMMDD-HHMMSS.zip`.
pub(crate) fn default_backup_path(paths: &Paths, prefix: &str) -> PathBuf {
    let stamp = Local::now().format("%Y%m%d-%H%M%S");
    paths.backups_dir().join(format!("{prefix}-{stamp}.zip"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};

    /// Reference locale for tests (error text isn't checked here — only the
    /// signature matters; ru byte-for-byte with the previous strings).
    fn ru() -> &'static Locale {
        locale(Lang::Ru)
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
        // Must not end up in the backup:
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
        db.rag_delete_by_source(profile, "scratch").unwrap();
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
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect()
    }

    #[test]
    fn backup_includes_expected_and_excludes_logs_marker() {
        let dir = tempfile::tempdir().unwrap();
        seed_data(dir.path());
        let paths = Paths::with_root(dir.path());

        let out = create_backup(&paths, None, 9, None, None, ru()).unwrap();
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
        ] {
            assert!(
                names.contains(&expected.to_string()),
                "missing {expected} in {names:?}"
            );
        }
        // Logs, the backups directory, and the defaults files are not included.
        assert!(!names.iter().any(|n| n.starts_with("logs/")));
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
        let out = create_backup(&paths, None, 9, Some(&inside), None, ru()).unwrap();
        assert!(archive_names(&out).contains(&"sandbox/note.txt".to_string()));

        // Outside the root — not included.
        let outside_root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), b"no").unwrap();
        seed_data(outside_root.path());
        let paths2 = Paths::with_root(outside_root.path());
        let out2 = create_backup(&paths2, None, 0, Some(outside.path()), None, ru()).unwrap();
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
        create_backup(&src_paths, Some(archive_path.clone()), 9, None, None, ru()).unwrap();

        // Target with different data.
        let dst = tempfile::tempdir().unwrap();
        seed_data(dst.path());
        fs::write(dst.path().join("settings.json"), b"{\"v\":999}").unwrap();
        fs::write(dst.path().join("chats").join("stale.json"), b"{}").unwrap();
        let dst_paths = Paths::with_root(dst.path());

        let outcome = restore_backup(&dst_paths, &archive_path, None, None, ru()).unwrap();
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
        // The backups directory is preserved (it holds the pre-restore copy).
        assert!(dst.path().join("backups").exists());
    }

    #[test]
    fn restore_rejects_corrupt_archive_without_touching_data() {
        let dst = tempfile::tempdir().unwrap();
        seed_data(dst.path());
        let paths = Paths::with_root(dst.path());
        let bad = dst.path().join("bad.zip");
        fs::write(&bad, b"this is not a zip file").unwrap();

        let err = restore_backup(&paths, &bad, None, None, ru());
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
            9,
            None,
            None,
            ru(),
        )
        .unwrap();

        let dst = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dst.path());
        let outcome = restore_backup(&paths, &archive, None, None, ru()).unwrap();
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
        let outcome = restore_backup(&paths, &archive, None, None, ru()).unwrap();
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
        let out = create_backup(&paths, None, 0, None, None, ru()).unwrap();
        // The archive is valid and opens.
        validate_archive(&out, None, ru()).unwrap();
    }

    #[test]
    fn backup_writes_manifest_and_read_manifest_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        seed_data(dir.path());
        let paths = Paths::with_root(dir.path());
        let out = create_backup(&paths, None, 9, None, None, ru()).unwrap();

        assert!(archive_names(&out).contains(&MANIFEST_NAME.to_string()));
        let m = read_manifest(&out)
            .unwrap()
            .expect("a manifest should be present");
        assert_eq!(m.app_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(m.schemas.settings, SETTINGS_SCHEMA);
        assert_eq!(m.schemas.db, DB_SCHEMA);
        assert!(
            !m.is_newer_than_current(),
            "current schemas are not newer than themselves"
        );
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
            },
            created_at: "2030-01-01T00:00:00+00:00".into(),
        };
        assert!(m.is_newer_than_current());
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
        let out = create_backup(&paths, None, 9, None, None, ru()).unwrap();

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

        let out = create_backup(&paths, None, 9, None, None, ru()).unwrap();

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
        let outcome = restore_backup(&paths, &archive, None, None, ru()).unwrap();
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
            0,
            None,
            None,
            ru(),
        )
        .unwrap();

        let dst = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dst.path());
        let outcome = restore_backup(&paths, &archive, None, None, ru()).unwrap();
        assert!(matches!(outcome, RestoreOutcome::Restored { .. }));
        assert_eq!(fs::read(dst.path().join("data.db")).unwrap(), b"SQLITE");
    }

    #[test]
    fn restore_does_not_extract_manifest_into_root() {
        let src = tempfile::tempdir().unwrap();
        seed_data(src.path());
        let out = create_backup(&Paths::with_root(src.path()), None, 9, None, None, ru()).unwrap();

        let dst = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dst.path());
        let outcome = restore_backup(&paths, &out, None, None, ru()).unwrap();
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

        let out = create_backup(&paths, None, 9, None, Some(PW), ru()).unwrap();

        // Level 0 (store) so the marker would be literally present if unencrypted —
        // deflate could otherwise hide it and make this test lie.
        let plain = create_backup(
            &paths,
            Some(dir.path().join("plain.zip")),
            0,
            None,
            None,
            ru(),
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
            9,
            None,
            Some(PW),
            ru(),
        )
        .unwrap();
        let plain = create_backup(
            &paths,
            Some(dir.path().join("plain.zip")),
            9,
            None,
            None,
            ru(),
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
                9,
                None,
                password,
                ru(),
            )
            .unwrap();

            let dst = tempfile::tempdir().unwrap();
            let paths = Paths::with_root(dst.path());
            // The restore always holds the password — for the plain archive it
            // must simply be ignored.
            let outcome = restore_backup(&paths, &archive, None, Some(PW), ru()).unwrap();
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
            9,
            None,
            Some(PW),
            ru(),
        )
        .unwrap();

        for password in [None, Some("wrong")] {
            let dst = tempfile::tempdir().unwrap();
            seed_data(dst.path());
            fs::write(dst.path().join("settings.json"), b"{\"from\":\"original\"}").unwrap();
            let paths = Paths::with_root(dst.path());

            let err = restore_backup(&paths, &archive, None, password, ru());
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
        let out =
            create_backup(&Paths::with_root(dir.path()), None, 9, None, Some(PW), ru()).unwrap();

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
        let out =
            create_backup(&Paths::with_root(dir.path()), None, 9, None, Some(""), ru()).unwrap();
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
            9,
            None,
            Some(PW),
            ru(),
        )
        .unwrap();

        let dst = tempfile::tempdir().unwrap();
        seed_data(dst.path());
        let paths = Paths::with_root(dst.path());
        let RestoreOutcome::Restored {
            pre_restore: Some(pre),
        } = restore_backup(&paths, &archive, None, Some(PW), ru()).unwrap()
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
        let out =
            create_backup(&Paths::with_root(dir.path()), None, 0, None, Some(PW), ru()).unwrap();

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
        let refused = match restore_backup(&paths, &corrupt, None, Some(PW), ru()) {
            Err(_) => true,
            Ok(RestoreOutcome::Restored { .. }) => false,
            Ok(_) => true,
        };
        assert!(refused, "corrupted ciphertext was accepted");
    }
}

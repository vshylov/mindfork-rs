//! Backup and restore of user data into a zip archive.
//!
//! Launched via command-line arguments (`--backup` / `--restore`) with no TUI,
//! and ends the process (see `main.rs`). Archive contents (paths in the archive
//! are relative to the data root):
//! - files `settings.json`, `profiles.json`, `data.db` (+ sidecar `-wal`/`-shm`,
//!   if present), `personal_dictionary.txt`;
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
//! **Restore is transactional.** The archive is validated first (before any
//! destructive action). If the root already has data, it is automatically
//! saved into `backups/` (a pre-restore copy), and only then is the root
//! cleared and the given archive unpacked. If unpacking fails and a
//! pre-restore copy was created, a rollback to it is performed. See spec §12.3.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Local;
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::shared::i18n::Locale;
use crate::shared::paths::Paths;
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
const TOP_FILES: &[&str] = &[
    "settings.json",
    "profiles.json",
    "data.db",
    "data.db-wal",
    "data.db-shm",
    "personal_dictionary.txt",
];

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

/// Creates a backup of user data.
///
/// `output` — path to the archive to create (`None` → an auto-name in
/// `backups/`). `level` — compression level `0..=9` (`0` → no compression,
/// store). `fs_root` — the file-tool sandbox directory from the config
/// (included only when it lies inside the data root). Returns the path to the
/// created archive.
pub fn create_backup(
    paths: &Paths,
    output: Option<PathBuf>,
    level: i64,
    fs_root: Option<&Path>,
    loc: &Locale,
) -> Result<PathBuf> {
    let entries = gather_entries(paths, fs_root, loc)?;
    let out_path = match output {
        Some(p) => p,
        None => default_backup_path(paths, "mindfork-backup"),
    };
    write_zip(&out_path, &entries, level, loc).with_context(|| {
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
/// file, a corrupted/unsafe archive). Once the replacement has started it
/// always returns `Ok(RestoreOutcome)` describing the outcome (including a
/// rollback). `fs_root` — the current sandbox (cleared if inside the root).
pub fn restore_backup(
    paths: &Paths,
    archive: &Path,
    fs_root: Option<&Path>,
    loc: &Locale,
) -> Result<RestoreOutcome> {
    // 1. Validate the archive before any destructive action.
    validate_archive(archive, loc).with_context(|| {
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
        extract_archive(paths, archive, loc)
    })();

    match attempt {
        Ok(()) => Ok(RestoreOutcome::Restored { pre_restore }),
        Err(restore_error) => match &pre_restore {
            // 4. Roll back to the just-created pre-restore copy.
            Some(backup) => {
                let rollback = (|| -> Result<()> {
                    clear_user_data(paths, fs_root, loc)?;
                    extract_archive(paths, backup, loc)
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
fn gather_entries(paths: &Paths, fs_root: Option<&Path>, loc: &Locale) -> Result<Vec<Entry>> {
    let root = paths.root();
    let mut out: Vec<Entry> = Vec::new();

    for f in TOP_FILES {
        let abs = root.join(f);
        if abs.is_file() {
            out.push(Entry {
                abs,
                name: (*f).to_string(),
            });
        }
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

/// Writes the archive from the entry list at the given compression level.
fn write_zip(out_path: &Path, entries: &[Entry], level: i64, loc: &Locale) -> Result<()> {
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
    let options = if level == 0 {
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored)
    } else {
        SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(level))
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
    zip.start_file(MANIFEST_NAME, options)
        .with_context(|| loc.tf("backup.ctx.write_entry", &[("name", MANIFEST_NAME)]))?;
    io::copy(&mut manifest.as_slice(), &mut zip)
        .with_context(|| loc.tf("backup.ctx.pack", &[("path", MANIFEST_NAME)]))?;

    zip.finish()
        .with_context(|| loc.t("backup.ctx.finalize").to_string())?;
    Ok(())
}

/// Checks that the archive opens and all of its entries are safe relative
/// paths (no `..`/absolute paths — zip-slip protection).
fn validate_archive(archive: &Path, loc: &Locale) -> Result<()> {
    let file = File::open(archive).with_context(|| {
        loc.tf(
            "backup.ctx.open_archive",
            &[("path", &archive.display().to_string())],
        )
    })?;
    let mut zip = ZipArchive::new(file).with_context(|| loc.t("backup.ctx.corrupt").to_string())?;
    for i in 0..zip.len() {
        let entry = zip.by_index(i)?;
        if entry.enclosed_name().is_none() {
            bail!(
                "{}",
                loc.tf("backup.err.unsafe_entry", &[("name", entry.name())])
            );
        }
    }
    Ok(())
}

/// Unpacks the archive into the data root (entry names are already
/// considered safe — `enclosed_name` rejects escaping outside the root).
fn extract_archive(paths: &Paths, archive: &Path, loc: &Locale) -> Result<()> {
    let file = File::open(archive).with_context(|| {
        loc.tf(
            "backup.ctx.open_archive",
            &[("path", &archive.display().to_string())],
        )
    })?;
    let mut zip =
        ZipArchive::new(file).with_context(|| loc.t("backup.ctx.read_archive").to_string())?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
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

    for f in TOP_FILES {
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

        let out = create_backup(&paths, None, 9, None, ru()).unwrap();
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
        let out = create_backup(&paths, None, 9, Some(&inside), ru()).unwrap();
        assert!(archive_names(&out).contains(&"sandbox/note.txt".to_string()));

        // Outside the root — not included.
        let outside_root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), b"no").unwrap();
        seed_data(outside_root.path());
        let paths2 = Paths::with_root(outside_root.path());
        let out2 = create_backup(&paths2, None, 0, Some(outside.path()), ru()).unwrap();
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
        create_backup(&src_paths, Some(archive_path.clone()), 9, None, ru()).unwrap();

        // Target with different data.
        let dst = tempfile::tempdir().unwrap();
        seed_data(dst.path());
        fs::write(dst.path().join("settings.json"), b"{\"v\":999}").unwrap();
        fs::write(dst.path().join("chats").join("stale.json"), b"{}").unwrap();
        let dst_paths = Paths::with_root(dst.path());

        let outcome = restore_backup(&dst_paths, &archive_path, None, ru()).unwrap();
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

        let err = restore_backup(&paths, &bad, None, ru());
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
            ru(),
        )
        .unwrap();

        let dst = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dst.path());
        let outcome = restore_backup(&paths, &archive, None, ru()).unwrap();
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
        let outcome = restore_backup(&paths, &archive, None, ru()).unwrap();
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
        let en = validate_archive(&bad, locale(Lang::En))
            .unwrap_err()
            .to_string();
        assert!(en.contains("corrupted"), "{en}");
        assert!(!en.chars().any(|c| ('а'..='я').contains(&c)), "{en}");
        let r = validate_archive(&bad, ru()).unwrap_err().to_string();
        assert!(r.contains("повреждён"), "{r}");
    }

    #[test]
    fn store_level_zero_produces_readable_archive() {
        let dir = tempfile::tempdir().unwrap();
        seed_data(dir.path());
        let paths = Paths::with_root(dir.path());
        let out = create_backup(&paths, None, 0, None, ru()).unwrap();
        // The archive is valid and opens.
        validate_archive(&out, ru()).unwrap();
    }

    #[test]
    fn backup_writes_manifest_and_read_manifest_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        seed_data(dir.path());
        let paths = Paths::with_root(dir.path());
        let out = create_backup(&paths, None, 9, None, ru()).unwrap();

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
    fn restore_does_not_extract_manifest_into_root() {
        let src = tempfile::tempdir().unwrap();
        seed_data(src.path());
        let out = create_backup(&Paths::with_root(src.path()), None, 9, None, ru()).unwrap();

        let dst = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dst.path());
        let outcome = restore_backup(&paths, &out, None, ru()).unwrap();
        assert!(matches!(outcome, RestoreOutcome::Restored { .. }));
        // Data was restored, but the internal manifest didn't land in the root.
        assert!(dst.path().join("settings.json").exists());
        assert!(!dst.path().join(MANIFEST_NAME).exists());
    }
}

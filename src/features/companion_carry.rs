//! `mindfork companion export|import` — carrying one companion to another
//! machine (spec §12.5; docs/research/companion-platform.md C8 (b)): its
//! profile and its whole memory as one archive, and back as a new profile,
//! without moving the rest of the data directory — `restore` replaces all of
//! it.
//!
//! **The archive** is a zip with three entries:
//!
//! * `companion.json` — what it is (`kind`), the format, the app and schema
//!   versions, when. **Unencrypted and free of user data**, like a backup's
//!   manifest, so "this is a companion" and "this is from a newer version" can
//!   be said before a password is asked for;
//! * `profile.json` — the profile, every setting;
//! * `memory.db` — a small SQLite database of the backup's own schema holding
//!   that profile's memory alone: the notes with their links, scars and
//!   citations, the knowledge base's sources and chunks, the self-model and
//!   every version of it, the language-model history. Built and read by
//!   [`Db::profile_copy_from`](crate::shared::storage::Db::profile_copy_from),
//!   the fork's copy across files. **No vectors**: the other machine may run
//!   another embedder, so the notes are embedded there again on the next
//!   semantic path and the knowledge base waits for `/reindex`.
//!
//! The two data entries are encrypted with the backup's machinery when a
//! password is given (WinZip AES-256, spec §12.3), and the import settles the
//! password as `restore` does.
//!
//! **What does not travel**: the chats — a companion is carried to start anew
//! somewhere, its conversations are the backup's — and so nothing a chat owns
//! (its follow-ups, its attachments' indexes).
//!
//! **An import is always a new profile**, under a new id: importing twice
//! gives two companions that share a past, as a fork does, and an import can
//! never overwrite one already there.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::Local;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{AesMode, CompressionMethod, ZipWriter};

use crate::entities::profile::Profile;
use crate::features::backup::{self, ArchiveReader, SchemaVersions};
use crate::shared::i18n::Locale;
use crate::shared::paths::Paths;
use crate::shared::storage::db::ForkCopy;
use crate::shared::storage::schema::{CHAT_SCHEMA, DB_SCHEMA, PROFILES_SCHEMA, SETTINGS_SCHEMA};
use crate::shared::storage::{Db, JsonStore, Storage};

/// What `companion.json` says the archive is.
pub const CARRY_KIND: &str = "mindfork-companion";
/// The version of the archive's shape. A newer one is refused by name.
pub const CARRY_FORMAT: u32 = 1;

const MANIFEST_ENTRY: &str = "companion.json";
const PROFILE_ENTRY: &str = "profile.json";
const MEMORY_ENTRY: &str = "memory.db";

/// `companion.json`: no user data, only what the archive is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarryManifest {
    pub kind: String,
    pub format: u32,
    pub app_version: String,
    pub schemas: SchemaVersions,
    pub created_at: String,
}

impl CarryManifest {
    fn current() -> Self {
        Self {
            kind: CARRY_KIND.into(),
            format: CARRY_FORMAT,
            app_version: env!("CARGO_PKG_VERSION").into(),
            schemas: SchemaVersions {
                settings: SETTINGS_SCHEMA,
                profiles: PROFILES_SCHEMA,
                chat: CHAT_SCHEMA,
                db: DB_SCHEMA,
            },
            created_at: Local::now().to_rfc3339(),
        }
    }

    /// Made by a newer build: a newer archive format, or a profile or database
    /// schema this build cannot read.
    fn is_newer_than_current(&self) -> bool {
        self.format > CARRY_FORMAT
            || self.schemas.profiles > PROFILES_SCHEMA
            || self.schemas.db > DB_SCHEMA
    }
}

/// What an export or an import did.
#[derive(Debug, Clone, PartialEq)]
pub struct Carried {
    /// The profile's name.
    pub name: String,
    /// The exported profile's id, or the imported one's new id.
    pub profile_id: Uuid,
    /// The archive written, or read.
    pub archive: PathBuf,
    pub copy: ForkCopy,
}

/// A temporary database file, removed with its journal when dropped.
struct TempDb(PathBuf);

impl TempDb {
    fn beside(dir: &Path, stem: &str) -> Self {
        Self(dir.join(format!(".{stem}-{}.db", Uuid::new_v4().simple())))
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        for suffix in ["", "-journal", "-wal", "-shm"] {
            let mut path = self.0.clone().into_os_string();
            path.push(suffix);
            let _ = fs::remove_file(PathBuf::from(path));
        }
    }
}

/// The visible profile `query` names: its id, its name (any case), or the
/// unambiguous start of its name. `Err` — already localized: none matches, or
/// several do (each named).
pub fn find_profile<'a>(profiles: &'a [Profile], query: &str, loc: &Locale) -> Result<&'a Profile> {
    let visible: Vec<&Profile> = profiles.iter().filter(|p| !p.is_hidden).collect();
    let q = query.trim();
    if let Some(p) = visible.iter().find(|p| p.id.to_string() == q) {
        return Ok(p);
    }
    let lower = q.to_lowercase();
    let exact: Vec<&&Profile> = visible
        .iter()
        .filter(|p| p.name.to_lowercase() == lower)
        .collect();
    let candidates: Vec<&&Profile> = if exact.is_empty() {
        visible
            .iter()
            .filter(|p| p.name.to_lowercase().starts_with(&lower))
            .collect()
    } else {
        exact
    };
    match candidates.as_slice() {
        [one] => Ok(one),
        [] => {
            let names: Vec<&str> = visible.iter().map(|p| p.name.as_str()).collect();
            bail!(
                "{}",
                loc.tf(
                    "cli.companion.err.not_found",
                    &[("query", q), ("names", &names.join(", "))]
                )
            )
        }
        many => {
            let names: Vec<String> = many
                .iter()
                .map(|p| format!("{} ({})", p.name, p.id))
                .collect();
            bail!(
                "{}",
                loc.tf(
                    "cli.companion.err.ambiguous",
                    &[("query", q), ("names", &names.join(", "))]
                )
            )
        }
    }
}

/// Writes the companion `query` names to `output` (by default a new archive in
/// `backups/`), its data entries encrypted when `password` is given. Only
/// reads the data: the live database is attached read-only. Refuses to write
/// over a file.
pub fn export(
    paths: &Paths,
    query: &str,
    output: Option<PathBuf>,
    password: Option<&str>,
    loc: &Locale,
) -> Result<Carried> {
    let profiles = JsonStore::new(paths.clone())
        .load_profiles()
        .context(loc.t("cli.companion.ctx.profiles").to_string())?;
    let profile = find_profile(&profiles, query, loc)?.clone();
    let archive =
        output.unwrap_or_else(|| backup::default_backup_path(paths, "mindfork-companion"));
    if archive.exists() {
        bail!(
            "{}",
            loc.tf(
                "cli.companion.err.exists",
                &[("path", &archive.display().to_string())]
            )
        );
    }
    let dir = archive
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf();
    fs::create_dir_all(&dir).with_context(|| {
        loc.tf(
            "cli.companion.ctx.write",
            &[("path", &archive.display().to_string())],
        )
    })?;

    // The profile's memory, copied into a database of its own.
    let memory = TempDb::beside(&dir, "companion-memory");
    let copy = {
        let db = Db::open(&memory.0)?;
        let live = paths.data_db();
        if live.is_file() {
            db.profile_copy_from(&live, profile.id, profile.id)
                .context(loc.t("cli.companion.ctx.copy").to_string())?
        } else {
            ForkCopy::default()
        }
    };

    let write = || -> Result<()> {
        let mut zip = ZipWriter::new(File::create_new(&archive)?);
        let plain = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(6));
        let options = match password.filter(|p| !p.is_empty()) {
            Some(pw) => plain.with_aes_encryption(AesMode::Aes256, pw),
            None => plain,
        };
        zip.start_file(MANIFEST_ENTRY, plain)?;
        zip.write_all(&serde_json::to_vec_pretty(&CarryManifest::current())?)?;
        zip.start_file(PROFILE_ENTRY, options)?;
        zip.write_all(&serde_json::to_vec_pretty(&profile)?)?;
        zip.start_file(MEMORY_ENTRY, options)?;
        io::copy(&mut File::open(&memory.0)?, &mut zip)?;
        zip.finish()?;
        Ok(())
    };
    if let Err(err) = write() {
        let _ = fs::remove_file(&archive);
        return Err(err.context(loc.tf(
            "cli.companion.ctx.write",
            &[("path", &archive.display().to_string())],
        )));
    }
    Ok(Carried {
        name: profile.name,
        profile_id: profile.id,
        archive,
        copy,
    })
}

/// Reads a companion archive into `storage` as a **new** profile. `password`
/// is already settled by the caller (the `restore` rule). The profile is
/// written first and taken back if its memory cannot be copied, so an import
/// is whole or not there. A knowledge base that came along is marked to be
/// re-embedded (`/reindex`).
pub fn import(
    paths: &Paths,
    storage: &Storage,
    archive: &Path,
    password: Option<&str>,
    loc: &Locale,
) -> Result<Carried> {
    let mut reader = ArchiveReader::open(archive, password, loc)?;
    let manifest: Option<CarryManifest> = reader
        .with_entry(MANIFEST_ENTRY, |stream, _| read_json(stream))
        .ok()
        .flatten()
        .filter(|m: &CarryManifest| m.kind == CARRY_KIND);
    let Some(manifest) = manifest else {
        let is_backup = reader.entries().iter().any(|e| e.name == "manifest.json");
        let key = if is_backup {
            "cli.companion.err.is_backup"
        } else {
            "cli.companion.err.not_companion"
        };
        bail!(
            "{}",
            loc.tf(key, &[("path", &archive.display().to_string())])
        );
    };
    if manifest.is_newer_than_current() {
        bail!(
            "{}",
            loc.tf(
                "cli.companion.err.newer",
                &[("version", &manifest.app_version)]
            )
        );
    }
    let carried: Profile = reader
        .with_entry(PROFILE_ENTRY, |stream, _| read_json(stream))?
        .context(loc.t("cli.companion.err.incomplete").to_string())?;

    let memory = TempDb::beside(paths.root(), "companion-import");
    let found = reader.with_entry(MEMORY_ENTRY, |stream, _| {
        let mut out = File::create_new(&memory.0)?;
        io::copy(stream, &mut out)?;
        out.flush()?;
        Ok(())
    })?;
    if found.is_none() {
        bail!("{}", loc.t("cli.companion.err.incomplete"));
    }

    let mut profile = carried.clone();
    profile.id = Uuid::new_v4();
    profile.is_hidden = false;
    storage
        .json()
        .upsert_profile(&profile)
        .context(loc.t("cli.companion.ctx.profiles").to_string())?;
    let copy = match storage
        .db()
        .profile_copy_from(&memory.0, carried.id, profile.id)
    {
        Ok(copy) => copy,
        Err(err) => {
            if let Ok(mut all) = storage.json().load_profiles() {
                all.retain(|p| p.id != profile.id);
                let _ = storage.json().save_profiles(&all);
            }
            return Err(err.context(loc.t("cli.companion.ctx.copy").to_string()));
        }
    };
    if copy.knowledge_chunks > 0 {
        let db = storage.db();
        let mut stale = db.rag_stale_profiles()?;
        stale.push(profile.id);
        db.set_rag_stale_profiles(&stale)?;
    }
    Ok(Carried {
        name: profile.name,
        profile_id: profile.id,
        archive: archive.to_path_buf(),
        copy,
    })
}

fn read_json<T: serde::de::DeserializeOwned>(stream: &mut dyn Read) -> Result<T> {
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::note::Note;
    use crate::entities::rag::RagDocument;
    use crate::entities::self_model_history::VersionAuthor;
    use crate::shared::i18n::{Lang, locale};

    fn en() -> &'static Locale {
        locale(Lang::En)
    }

    /// A data root with a companion that has a little of everything, and a
    /// second profile beside it.
    fn root_with_companion() -> (tempfile::TempDir, Paths, Profile) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path().join("a"));
        let storage = Storage::open(paths.clone()).unwrap();
        let mut sol = Profile::new("Sol", "You are Sol.");
        sol.companion.followups = true;
        storage.json().upsert_profile(&sol).unwrap();
        storage
            .json()
            .upsert_profile(&Profile::new("Solveig", ""))
            .unwrap();
        let db = storage.db();
        let note = Note::new(sol.id, "Alex plays chess on Sundays", vec![]);
        db.note_insert(&note).unwrap();
        db.note_vector_upsert(note.id, sol.id, &[0.1, 0.2, 0.3])
            .unwrap();
        db.rag_source_upsert(sol.id, "chess.md", "Openings.", chrono::Utc::now())
            .unwrap();
        db.rag_insert(&RagDocument {
            id: Uuid::new_v4(),
            profile_id: sol.id,
            source: "chess.md".into(),
            chunk_text: "Openings".into(),
            embedding: vec![0.2, 0.8, 0.5],
            created_at: chrono::Utc::now(),
        })
        .unwrap();
        for text in ["I am Sol.", "I am Sol, Alex's friend."] {
            db.self_model_update(sol.id, &VersionAuthor::Person, |m| {
                m.summary = text.into();
                true
            })
            .unwrap();
        }
        drop(storage);
        (dir, paths, sol)
    }

    fn fresh_root(dir: &tempfile::TempDir, name: &str) -> (Paths, Storage) {
        let paths = Paths::with_root(dir.path().join(name));
        let storage = Storage::open(paths.clone()).unwrap();
        (paths, storage)
    }

    #[test]
    fn a_companion_travels_whole_and_lands_as_a_new_profile() {
        let (dir, paths, sol) = root_with_companion();
        let out = dir.path().join("sol.zip");
        let exported = export(&paths, "sol", Some(out.clone()), None, en()).unwrap();
        assert_eq!(
            (exported.name.as_str(), exported.profile_id),
            ("Sol", sol.id)
        );
        assert_eq!(
            (
                exported.copy.notes,
                exported.copy.knowledge_chunks,
                exported.copy.self_model_versions
            ),
            (1, 1, 2)
        );

        let (there, storage) = fresh_root(&dir, "b");
        let imported = import(&there, &storage, &out, None, en()).unwrap();
        assert_ne!(imported.profile_id, sol.id, "always a new profile");
        let profiles = storage.json().load_profiles().unwrap();
        let landed = profiles
            .iter()
            .find(|p| p.id == imported.profile_id)
            .unwrap();
        assert_eq!(landed.name, "Sol");
        assert!(landed.companion.followups, "the settings came along");
        let db = storage.db();
        assert_eq!(db.note_list(landed.id, None, &[], None).unwrap().len(), 1);
        let model = db.self_model_get(landed.id).unwrap().unwrap();
        assert_eq!(model.summary, "I am Sol, Alex's friend.");
        assert_eq!(db.self_model_history(landed.id).unwrap().len(), 2);
        // No vector came: the note waits to be embedded, the knowledge base
        // to be re-embedded — and says so.
        assert_eq!(db.notes_missing_vectors(landed.id).unwrap().len(), 1);
        assert!(db.rag_is_stale(landed.id).unwrap());
        // Nothing of the temporary database is left behind.
        let leftovers: Vec<_> = fs::read_dir(there.root())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".companion"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");

        // A second import is a second companion, not an overwrite.
        let again = import(&there, &storage, &out, None, en()).unwrap();
        assert_ne!(again.profile_id, imported.profile_id);
    }

    #[test]
    fn a_password_locks_the_data_but_not_the_manifest() {
        let (dir, paths, _) = root_with_companion();
        let out = dir.path().join("sol.zip");
        export(&paths, "Sol", Some(out.clone()), Some("s3cret"), en()).unwrap();
        let (there, storage) = fresh_root(&dir, "b");
        let err = import(&there, &storage, &out, None, en()).unwrap_err();
        assert!(
            format!("{err:#}").to_lowercase().contains("password"),
            "{err:#}"
        );
        assert!(import(&there, &storage, &out, Some("wrong"), en()).is_err());
        assert_eq!(
            storage.json().load_profiles().unwrap().len(),
            0,
            "nothing landed"
        );
        import(&there, &storage, &out, Some("s3cret"), en()).unwrap();
        // The manifest reads without the password.
        let mut zip = zip::ZipArchive::new(File::open(&out).unwrap()).unwrap();
        let manifest: CarryManifest =
            serde_json::from_reader(zip.by_name(MANIFEST_ENTRY).unwrap()).unwrap();
        assert_eq!(manifest.kind, CARRY_KIND);
        assert!(
            zip.by_name(PROFILE_ENTRY).is_err(),
            "the profile is encrypted"
        );
    }

    #[test]
    fn an_export_names_its_profile_and_never_overwrites() {
        let (dir, paths, _) = root_with_companion();
        let out = dir.path().join("sol.zip");
        // "Sol" is exact even though "Solveig" starts with it too.
        export(&paths, "SOL", Some(out.clone()), None, en()).unwrap();
        let err = export(&paths, "sol", Some(out.clone()), None, en()).unwrap_err();
        assert!(format!("{err:#}").contains("sol.zip"), "{err:#}");
        // A prefix two names share is refused with both named.
        let err = export(&paths, "so", Some(dir.path().join("x.zip")), None, en()).unwrap_err();
        let text = format!("{err:#}");
        assert!(
            text.contains("Sol (") && text.contains("Solveig ("),
            "{text}"
        );
        let err = export(&paths, "Ada", Some(dir.path().join("y.zip")), None, en()).unwrap_err();
        assert!(
            format!("{err:#}").contains("Solveig"),
            "the names it knows: {err:#}"
        );
        assert!(!dir.path().join("x.zip").exists() && !dir.path().join("y.zip").exists());
    }

    #[test]
    fn a_backup_or_a_foreign_zip_is_refused_by_name() {
        let (dir, paths, _) = root_with_companion();
        let backup_zip = dir.path().join("all.zip");
        backup::create_backup(
            &paths,
            Some(backup_zip.clone()),
            1,
            None,
            None,
            en(),
            |_| {},
        )
        .unwrap();
        let (there, storage) = fresh_root(&dir, "b");
        let err = import(&there, &storage, &backup_zip, None, en()).unwrap_err();
        assert!(format!("{err:#}").contains("mindfork restore"), "{err:#}");

        let foreign = dir.path().join("foreign.zip");
        let mut zip = ZipWriter::new(File::create(&foreign).unwrap());
        zip.start_file("readme-mf.txt", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"hello").unwrap();
        zip.finish().unwrap();
        let err = import(&there, &storage, &foreign, None, en()).unwrap_err();
        assert!(format!("{err:#}").contains("foreign.zip"), "{err:#}");
    }

    #[test]
    fn an_archive_from_a_newer_build_is_refused() {
        let (dir, _, _) = root_with_companion();
        let newer = dir.path().join("newer.zip");
        let mut manifest = CarryManifest::current();
        manifest.format = CARRY_FORMAT + 1;
        manifest.app_version = "9.9.9".into();
        let mut zip = ZipWriter::new(File::create(&newer).unwrap());
        zip.start_file(MANIFEST_ENTRY, SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        zip.finish().unwrap();
        let (there, storage) = fresh_root(&dir, "b");
        let err = import(&there, &storage, &newer, None, en()).unwrap_err();
        assert!(format!("{err:#}").contains("9.9.9"), "{err:#}");
    }
}

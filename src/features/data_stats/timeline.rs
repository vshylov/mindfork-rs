//! `mindfork stats --self-model` — the self-model's timeline as JSON (spec
//! §12.4, §17.9; docs/research/companion-platform.md C9): for every profile,
//! each version of its self-model with who wrote it, its observations with
//! what superseded them, and the language models it talked to — from the live
//! data or a backup archive, read the way the rest of `stats` reads, changing
//! nothing.
//!
//! Unlike the `--json` snapshot, which carries digests so it can travel, this
//! carries the **text**: it is the record a researcher reads to see how a self
//! changed and under which model. It goes to stdout, so where it lands is the
//! person's choice. The rows come out as stored ([`db::timeline_of_file`]);
//! this layer adds what the database cannot say — each profile's name, from
//! `profiles.json`, and the title of each conversation a version names, from
//! that chat's file.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{BACKUP_MARKERS, CHATS_PREFIX, DB_ENTRY, PROFILES_ENTRY, Source, read_all};
use crate::features::backup::{self, ArchiveReader, BackupManifest};
use crate::shared::i18n::Locale;
use crate::shared::paths::Paths;
use crate::shared::storage::db::{self, DbTimeline, ProfileTimeline};

/// The version of the export's shape. Additive fields do not bump it.
pub const TIMELINE_FORMAT: u32 = 1;

/// Everything `mindfork stats --self-model` prints.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Timeline {
    pub format: u32,
    /// The version of the binary that produced the export.
    pub app_version: String,
    pub taken_at: DateTime<Utc>,
    pub source: Source,
    pub profiles: Vec<ProfileEntry>,
    /// The title of every conversation a version names, by chat id; a chat
    /// whose file is gone or unreadable is left out.
    pub chats: BTreeMap<String, String>,
}

/// One profile's timeline under its name.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProfileEntry {
    /// `None` — the profile is not in `profiles.json` (the database outlived
    /// it, or the file is unreadable).
    pub name: Option<String>,
    /// Soft-deleted (`is_hidden`).
    pub deleted: bool,
    /// Where a fork came from (spec §10), as `profiles.json` holds it: the
    /// profile, its name then, when, and the self-model version the two share
    /// up to. `None` — not a fork.
    pub forked_from: Option<serde_json::Value>,
    #[serde(flatten)]
    pub timeline: ProfileTimeline,
}

/// What `profiles.json` is read for here.
#[derive(Deserialize)]
struct ProfileLite {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    is_hidden: bool,
    #[serde(default)]
    forked_from: Option<serde_json::Value>,
}

/// What a chat file is read for here.
#[derive(Deserialize)]
struct ChatTitle {
    #[serde(default)]
    title: String,
}

/// The timeline of the live data root, which is only read. A root with no
/// database has an empty timeline.
pub fn collect_timeline_root(paths: &Paths) -> Result<Timeline> {
    let db_path = paths.data_db();
    let database = if db_path.is_file() {
        db::timeline_of_file(&db_path)?
    } else {
        DbTimeline::default()
    };
    let profiles = fs::read(paths.profiles_file()).unwrap_or_default();
    let chats_dir = paths.chats_dir();
    let titles = |id: &str| fs::read(chats_dir.join(format!("{id}.json"))).ok();
    let source = Source::DataRoot {
        path: paths.root().display().to_string(),
    };
    Ok(assemble(source, database, &profiles, titles))
}

/// The timeline in a backup archive, nothing of it unpacked. `password` —
/// settled by the caller as `restore` settles it.
pub fn collect_timeline_archive(
    archive: &Path,
    password: Option<&str>,
    loc: &Locale,
) -> Result<Timeline> {
    let mut reader = ArchiveReader::open(archive, password, loc)?;
    let entries = reader.entries();
    let is_backup = entries
        .iter()
        .any(|e| BACKUP_MARKERS.contains(&e.name.as_str()) || e.name.starts_with(CHATS_PREFIX));
    if !is_backup {
        bail!(
            "{}",
            loc.tf(
                "cli.stats.err.not_a_backup",
                &[("path", &archive.display().to_string())]
            )
        );
    }
    let database = match entries.iter().find(|e| e.name == DB_ENTRY) {
        None => DbTimeline::default(),
        Some(_) => reader
            .with_entry(DB_ENTRY, |stream, size| {
                db::timeline_of_image(stream, usize::try_from(size)?)
            })?
            .context("the archive lists data.db and does not hold it")?,
    };
    let profiles = reader
        .with_entry(PROFILES_ENTRY, |stream, _| read_all(stream))
        .ok()
        .flatten()
        .unwrap_or_default();
    // The chats a version names, read before the reader is lent to `titles`.
    let named = named_chats(&database);
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for id in named {
        let entry = format!("{CHATS_PREFIX}{id}.json");
        if let Ok(Some(content)) = reader.with_entry(&entry, |stream, _| read_all(stream)) {
            files.insert(id, content);
        }
    }
    let manifest = backup::read_manifest(archive).ok().flatten();
    let source = Source::Archive {
        path: archive.display().to_string(),
        newer_than_this_app: manifest
            .as_ref()
            .is_some_and(BackupManifest::is_newer_than_current),
        manifest,
    };
    Ok(assemble(source, database, &profiles, |id| {
        files.get(id).cloned()
    }))
}

/// The ids of the conversations the versions name.
fn named_chats(database: &DbTimeline) -> BTreeSet<String> {
    database
        .profiles
        .iter()
        .flat_map(|p| &p.versions)
        .filter_map(|v| v.chat_id.clone())
        .collect()
}

fn assemble(
    source: Source,
    database: DbTimeline,
    profiles_json: &[u8],
    chat_file: impl Fn(&str) -> Option<Vec<u8>>,
) -> Timeline {
    let known: Vec<ProfileLite> = serde_json::from_slice(profiles_json).unwrap_or_default();
    let chats = named_chats(&database)
        .into_iter()
        .filter_map(|id| {
            let content = chat_file(&id)?;
            let chat: ChatTitle = serde_json::from_slice(&content).ok()?;
            Some((id, chat.title))
        })
        .collect();
    let profiles = database
        .profiles
        .into_iter()
        .map(|timeline| {
            let profile = known.iter().find(|p| p.id == timeline.profile_id);
            ProfileEntry {
                name: profile.map(|p| p.name.clone()),
                deleted: profile.is_some_and(|p| p.is_hidden),
                forked_from: profile.and_then(|p| p.forked_from.clone()),
                timeline,
            }
        })
        .collect();
    Timeline {
        format: TIMELINE_FORMAT,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        taken_at: Utc::now(),
        source,
        profiles,
        chats,
    }
}

/// The export: pretty-printed, profiles by id and each list oldest first, so
/// two exports line up under any `diff`.
pub fn render_timeline(timeline: &Timeline) -> String {
    serde_json::to_string_pretty(timeline).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::chat::Chat;
    use crate::entities::profile::Profile;
    use crate::entities::self_model_history::VersionAuthor;
    use crate::shared::config::ServerMode;
    use crate::shared::storage::{JsonStore, Storage};

    fn en() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::En)
    }

    /// A data root the app wrote: a profile, a chat, two versions of its
    /// self-model — one written in that chat.
    fn root() -> (tempfile::TempDir, Paths, Profile, Chat) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        let json = JsonStore::new(paths.clone());
        let profile = Profile::new("Sol", "You are Sol.");
        json.upsert_profile(&profile).unwrap();
        let chat = Chat::from_profile(&profile, "Chess openings");
        json.save_chat(&chat).unwrap();
        let storage = Storage::open(paths.clone()).unwrap();
        let turn = VersionAuthor::Model {
            name: Some("gemma-4-31b".into()),
            mode: ServerMode::External,
            chat_id: Some(chat.id),
            background: false,
        };
        storage
            .db()
            .self_model_update(profile.id, &turn, |m| {
                m.summary = "I am Sol.".into();
                true
            })
            .unwrap();
        storage
            .db()
            .self_model_update(profile.id, &VersionAuthor::Person, |m| {
                m.summary = "I am Sol, Alex's friend.".into();
                true
            })
            .unwrap();
        drop(storage);
        (dir, paths, profile, chat)
    }

    #[test]
    fn the_export_names_profiles_and_conversations() {
        let (_dir, paths, profile, chat) = root();
        let timeline = collect_timeline_root(&paths).unwrap();
        assert_eq!(timeline.profiles.len(), 1);
        let p = &timeline.profiles[0];
        assert_eq!(p.name.as_deref(), Some("Sol"));
        assert!(!p.deleted);
        assert!(p.forked_from.is_none());
        assert_eq!(p.timeline.profile_id, profile.id.to_string());
        assert_eq!(p.timeline.versions.len(), 2);
        assert_eq!(
            timeline.chats.get(&chat.id.to_string()).map(String::as_str),
            Some("Chess openings")
        );

        // The JSON carries the text, flat under each profile.
        let text = render_timeline(&timeline);
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let first = &value["profiles"][0];
        assert_eq!(first["name"], "Sol");
        assert_eq!(first["versions"][0]["author"], "turn");
        assert_eq!(first["versions"][0]["model"], "gemma-4-31b");
        assert_eq!(
            first["versions"][1]["self_model"]["summary"],
            "I am Sol, Alex's friend."
        );
        assert_eq!(value["format"], TIMELINE_FORMAT);
    }

    #[test]
    fn a_root_with_no_data_has_an_empty_timeline() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path().join("none"));
        let timeline = collect_timeline_root(&paths).unwrap();
        assert!(timeline.profiles.is_empty());
        assert!(!paths.root().exists(), "nothing is created");
    }

    /// The same timeline out of a backup of the root, its chat titles read
    /// from the archive's entries.
    #[test]
    fn an_archive_exports_like_its_root() {
        let (_dir, paths, _, _) = root();
        let out = tempfile::tempdir().unwrap();
        let archive = out.path().join("mf-backup.zip");
        backup::create_backup(&paths, Some(archive.clone()), 6, None, None, en(), |_| {}).unwrap();
        let from_archive = collect_timeline_archive(&archive, None, en()).unwrap();
        let from_root = collect_timeline_root(&paths).unwrap();
        assert_eq!(from_archive.profiles, from_root.profiles);
        assert_eq!(from_archive.chats, from_root.chats);
    }

    #[test]
    fn a_zip_that_is_not_a_backup_is_refused() {
        let out = tempfile::tempdir().unwrap();
        let archive = out.path().join("other.zip");
        let file = fs::File::create(&archive).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("readme-mf.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut zip, b"hello").unwrap();
        zip.finish().unwrap();
        let err = collect_timeline_archive(&archive, None, en()).unwrap_err();
        assert!(format!("{err:#}").contains("other.zip"), "{err:#}");
    }

    /// A fork's entry says where it came from, and it shares its parent's
    /// versions up to there.
    #[test]
    fn a_fork_says_where_it_came_from() {
        let (_dir, paths, profile, _) = root();
        let storage = Storage::open(paths.clone()).unwrap();
        let fork =
            crate::features::profiles::fork(&profile, "Ada", chrono::Utc::now(), Some(2)).unwrap();
        storage.json().upsert_profile(&fork).unwrap();
        storage.db().profile_fork_copy(profile.id, fork.id).unwrap();
        drop(storage);
        let timeline = collect_timeline_root(&paths).unwrap();
        let entry = timeline
            .profiles
            .iter()
            .find(|p| p.timeline.profile_id == fork.id.to_string())
            .unwrap();
        let origin = entry.forked_from.as_ref().unwrap();
        assert_eq!(origin["profile_id"], profile.id.to_string());
        assert_eq!(origin["self_model_version"], 2);
        assert_eq!(entry.timeline.versions.len(), 2);
    }
}

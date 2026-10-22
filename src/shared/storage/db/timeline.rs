//! The self-model's timeline over a `data.db`, read-only, for
//! `mindfork stats --self-model` (spec §12.4, §17.9;
//! docs/research/companion-platform.md C9): per profile, every version of the
//! self-model with who wrote it, its observations with what superseded them,
//! and which language model the profile talked to when.
//!
//! The same sources and rules as [`super::stats`]: the live file opened
//! read-only behind the header check, or a backup's image in memory — nothing
//! is migrated, created or unpacked — and a table this binary knows and the
//! database lacks reads as empty, so an older copy exports what it has.
//!
//! **Rows go out as stored.** The model of a version is the stored JSON, its
//! author the stored kind, a time the stored text: this is an export for a
//! researcher to read with their own tools, and a copy written by another
//! build must come out whole rather than through this build's types. Like the
//! history itself, a current model newer than its newest recorded version is
//! the last version, of unknown authorship.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use serde::Serialize;

use super::stats::{has_table, open_file, open_image};

/// The timeline of every profile the database holds a self-model, an
/// observation or a model history for, ordered by profile id.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DbTimeline {
    pub profiles: Vec<ProfileTimeline>,
}

/// One profile's timeline. Each list is oldest first.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ProfileTimeline {
    pub profile_id: String,
    pub versions: Vec<TimelineVersion>,
    pub observations: Vec<TimelineObservation>,
    pub llm_history: Vec<TimelineLlm>,
}

/// One version of the self-model.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimelineVersion {
    pub version: i64,
    pub written_at: String,
    /// Who wrote it: `turn`, `background`, `person`, `app` or `unknown`.
    pub author: String,
    /// The model that wrote it, its engine mode and the conversation — when a
    /// model did.
    pub model: Option<String>,
    pub mode: Option<String>,
    pub chat_id: Option<String>,
    /// The self-model as stored; the raw text when it is not JSON.
    pub self_model: serde_json::Value,
}

/// One observation (an `@self` note), superseded ones included.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimelineObservation {
    pub id: String,
    pub text: String,
    pub created_at: String,
    pub updated_at: String,
    pub superseded_by: Option<String>,
    pub superseded_at: Option<String>,
}

/// One change of the profile's language model.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimelineLlm {
    pub changed_at: String,
    pub model: String,
    pub mode: String,
}

/// The timeline in the database file at `path`, which is left untouched.
pub fn timeline_of_file(path: &Path) -> Result<DbTimeline> {
    collect(&open_file(path)?)
}

/// The timeline in a database image of exactly `size` bytes — a backup's
/// `data.db` as it comes out of the archive.
pub fn timeline_of_image(image: impl Read, size: usize) -> Result<DbTimeline> {
    collect(&open_image(image, size)?)
}

fn collect(conn: &Connection) -> Result<DbTimeline> {
    let mut by_profile: BTreeMap<String, ProfileTimeline> = BTreeMap::new();
    for (pid, v) in versions(conn)? {
        entry(&mut by_profile, &pid).versions.push(v);
    }
    for (pid, current) in current_models(conn)? {
        let timeline = entry(&mut by_profile, &pid);
        if timeline
            .versions
            .last()
            .is_none_or(|v| v.version < current.version)
        {
            timeline.versions.push(current);
        }
    }
    for (pid, o) in observations(conn)? {
        entry(&mut by_profile, &pid).observations.push(o);
    }
    for (pid, rec) in llm_history(conn)? {
        entry(&mut by_profile, &pid).llm_history.push(rec);
    }
    Ok(DbTimeline {
        profiles: by_profile.into_values().collect(),
    })
}

/// The profile's timeline in `map`, started empty on first use.
fn entry<'a>(map: &'a mut BTreeMap<String, ProfileTimeline>, pid: &str) -> &'a mut ProfileTimeline {
    map.entry(pid.to_string())
        .or_insert_with(|| ProfileTimeline {
            profile_id: pid.to_string(),
            ..Default::default()
        })
}

/// The stored model text as JSON, or as the text itself when it is not.
fn model_value(data: String) -> serde_json::Value {
    serde_json::from_str(&data).unwrap_or(serde_json::Value::String(data))
}

fn versions(conn: &Connection) -> Result<Vec<(String, TimelineVersion)>> {
    if !has_table(conn, "self_model_versions")? {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT profile_id, version, written_at, author, model, mode, chat_id, data
         FROM self_model_versions ORDER BY rowid",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                TimelineVersion {
                    version: r.get(1)?,
                    written_at: r.get(2)?,
                    author: r.get(3)?,
                    model: r.get(4)?,
                    mode: r.get(5)?,
                    chat_id: r.get(6)?,
                    self_model: model_value(r.get(7)?),
                },
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("listing self_model_versions")?;
    Ok(rows)
}

/// Each profile's model as it stands, as a version of unknown authorship —
/// kept by [`collect`] only where the history has not recorded it.
fn current_models(conn: &Connection) -> Result<Vec<(String, TimelineVersion)>> {
    if !has_table(conn, "self_models")? {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare("SELECT profile_id, version, updated_at, data FROM self_models")?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                TimelineVersion {
                    version: r.get(1)?,
                    written_at: r.get(2)?,
                    author: "unknown".into(),
                    model: None,
                    mode: None,
                    chat_id: None,
                    self_model: model_value(r.get(3)?),
                },
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("listing self_models")?;
    Ok(rows)
}

/// The `@self` notes, oldest first, with what superseded each. A database
/// older than supersession has plain notes.
fn observations(conn: &Connection) -> Result<Vec<(String, TimelineObservation)>> {
    if !has_table(conn, "notes")? {
        return Ok(Vec::new());
    }
    // Tags are a JSON array of strings, so the quoted tag matches the tag
    // itself and never a longer one that begins with it.
    let sql = if has_table(conn, "note_superseded")? {
        "SELECT n.profile_id, n.id, n.content, n.created_at, n.updated_at,
                s.superseded_by, s.superseded_at
         FROM notes n LEFT JOIN note_superseded s ON s.note_id = n.id
         WHERE n.tags LIKE ?1 ORDER BY n.created_at, n.id"
    } else {
        "SELECT profile_id, id, content, created_at, updated_at, NULL, NULL
         FROM notes WHERE tags LIKE ?1 ORDER BY created_at, id"
    };
    let tag = format!("%{}%", serde_json::to_string(SELF_TAG)?);
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params![tag], |r| {
            Ok((
                r.get::<_, String>(0)?,
                TimelineObservation {
                    id: r.get(1)?,
                    text: r.get(2)?,
                    created_at: r.get(3)?,
                    updated_at: r.get(4)?,
                    superseded_by: r.get(5)?,
                    superseded_at: r.get(6)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("listing the observations")?;
    Ok(rows)
}

/// The tag of an observation note — `features::tools::notes::SELF_NOTE_TAG`,
/// which `shared` cannot name; a test below keeps the two equal.
const SELF_TAG: &str = "@self";

fn llm_history(conn: &Connection) -> Result<Vec<(String, TimelineLlm)>> {
    if !has_table(conn, "llm_history")? {
        return Ok(Vec::new());
    }
    let mut stmt =
        conn.prepare("SELECT profile_id, changed_at, model, mode FROM llm_history ORDER BY rowid")?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                TimelineLlm {
                    changed_at: r.get(1)?,
                    model: r.get(2)?,
                    mode: r.get(3)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("listing llm_history")?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::note::Note;
    use crate::entities::profile::LlmChange;
    use crate::entities::self_model::SelfModel;
    use crate::entities::self_model_history::VersionAuthor;
    use crate::shared::config::ServerMode;
    use crate::shared::storage::Db;
    use uuid::Uuid;

    #[test]
    fn the_self_tag_is_the_notes_one() {
        assert_eq!(SELF_TAG, crate::features::tools::notes::SELF_NOTE_TAG);
    }

    /// A database the app wrote: two versions of one profile's model, an
    /// observation superseded by another, a plain note, a model history —
    /// and a second profile whose model predates the history.
    fn seeded() -> (tempfile::TempDir, std::path::PathBuf, Uuid, Uuid) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.db");
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        {
            let db = Db::open(&path).unwrap();
            let turn = VersionAuthor::Model {
                name: Some("gemma-4-31b".into()),
                mode: ServerMode::External,
                chat_id: Some(Uuid::nil()),
                background: false,
            };
            db.self_model_update(a, &turn, |m| {
                m.summary = "I am Sol.".into();
                true
            })
            .unwrap();
            db.self_model_update(a, &VersionAuthor::Person, |m| {
                m.summary = "I am Sol, Alex's friend.".into();
                true
            })
            .unwrap();
            let old = Note::new(a, "first thought", vec![SELF_TAG.into()]);
            let new = Note::new(a, "a better thought", vec![SELF_TAG.into()]);
            db.note_insert(&old).unwrap();
            db.note_insert(&new).unwrap();
            db.note_insert(&Note::new(a, "Alex likes chess", vec![]))
                .unwrap();
            db.note_supersede_mark(a, old.id, new.id).unwrap();
            db.llm_history_note(
                a,
                &LlmChange {
                    changed_at: chrono::Utc::now(),
                    model: "gemma-4-31b".into(),
                    mode: ServerMode::External,
                },
            )
            .unwrap();
            // Profile B: a model written before the history was kept.
            let mut m = SelfModel::new(b);
            m.summary = "old".into();
            db.self_model_upsert(&m, &VersionAuthor::App).unwrap();
        }
        // Erase B's history row, as a database from before the table had none.
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "DELETE FROM self_model_versions WHERE profile_id = ?1",
            params![b.to_string()],
        )
        .unwrap();
        (dir, path, a, b)
    }

    #[test]
    fn the_timeline_holds_versions_observations_and_models() {
        let (_dir, path, a, b) = seeded();
        let timeline = timeline_of_file(&path).unwrap();
        let of = |pid: Uuid| {
            timeline
                .profiles
                .iter()
                .find(|p| p.profile_id == pid.to_string())
                .unwrap()
        };

        let pa = of(a);
        let versions: Vec<(i64, &str, Option<&str>)> = pa
            .versions
            .iter()
            .map(|v| (v.version, v.author.as_str(), v.model.as_deref()))
            .collect();
        assert_eq!(
            versions,
            vec![(1, "turn", Some("gemma-4-31b")), (2, "person", None)]
        );
        assert_eq!(pa.versions[0].self_model["summary"], "I am Sol.");
        assert_eq!(pa.versions[0].mode.as_deref(), Some("external"));
        // Observations only — the plain note stays out — oldest first, the
        // superseded one saying by what.
        let texts: Vec<&str> = pa.observations.iter().map(|o| o.text.as_str()).collect();
        assert_eq!(texts, vec!["first thought", "a better thought"]);
        assert_eq!(
            pa.observations[0].superseded_by.as_deref(),
            Some(pa.observations[1].id.as_str())
        );
        assert!(pa.observations[1].superseded_by.is_none());
        assert_eq!(pa.llm_history.len(), 1);

        // B's model predates the history: it is the one version, unknown.
        let pb = of(b);
        assert_eq!(pb.versions.len(), 1);
        assert_eq!(
            (pb.versions[0].version, pb.versions[0].author.as_str()),
            (1, "unknown")
        );
        assert_eq!(pb.versions[0].self_model["summary"], "old");
    }

    /// The export only reads: the file is byte-identical afterwards.
    #[test]
    fn reading_the_timeline_changes_nothing() {
        let (_dir, path, _, _) = seeded();
        let before = std::fs::read(&path).unwrap();
        timeline_of_file(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn an_image_reads_like_the_file() {
        let (_dir, path, _, _) = seeded();
        let bytes = std::fs::read(&path).unwrap();
        let from_image = timeline_of_image(bytes.as_slice(), bytes.len()).unwrap();
        assert_eq!(from_image, timeline_of_file(&path).unwrap());
    }

    /// A database from before the history (and before notes' supersession)
    /// exports what it has.
    #[test]
    fn an_older_database_exports_what_it_has() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.db");
        let pid = Uuid::new_v4();
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE self_models (profile_id TEXT PRIMARY KEY, data TEXT NOT NULL,
                 version INTEGER NOT NULL, updated_at TEXT NOT NULL);
             CREATE TABLE notes (id TEXT PRIMARY KEY, profile_id TEXT NOT NULL,
                 content TEXT NOT NULL, tags TEXT NOT NULL, created_at TEXT NOT NULL,
                 updated_at TEXT NOT NULL);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO self_models VALUES (?1, 'not json', 7, '2026-01-01T00:00:00+00:00')",
            params![pid.to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO notes VALUES ('n1', ?1, 'seen', '[\"@self\"]',
                 '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')",
            params![pid.to_string()],
        )
        .unwrap();
        drop(conn);
        let timeline = timeline_of_file(&path).unwrap();
        assert_eq!(timeline.profiles.len(), 1);
        let p = &timeline.profiles[0];
        assert_eq!(p.versions.len(), 1);
        assert_eq!(p.versions[0].version, 7);
        assert_eq!(p.versions[0].self_model, "not json");
        assert_eq!(p.observations.len(), 1);
        assert!(p.llm_history.is_empty());
    }
}

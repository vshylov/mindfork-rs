//! Storage (SQLite) — copying a profile's memory into a fork (spec §10,
//! `/profile fork`; docs/research/companion-platform.md C8 (a)), and across
//! databases to carry a companion (`mindfork companion export|import`, C8 (b) —
//! [`Db::profile_copy_from`]). Part of the [`super`] module.
//!
//! A fork gets the profile's whole memory under its own id: the notes with
//! their vectors, links, supersession scars and knowledge-base citations; the
//! knowledge base, sources and chunks with their vectors; the self-model and
//! every version of it; the language-model history. Not the chats — and so not
//! what belongs to a chat: the follow-ups and the attachments' indexes.
//!
//! **One savepoint**: a copy that fails halfway leaves nothing behind, so a
//! fork is either whole or not there. **New ids** where an id is the key —
//! notes and knowledge-base chunks — with every reference remapped (a link, a
//! scar, a citation names a note by id); a reference to a note the profile no
//! longer has is dropped with it. **The vectors are copied, not recomputed**:
//! same machine, same embedder, and each keeps the embedding generation it was
//! made under, so a vector that was foreign before the fork is foreign after it
//! and healed the same way. A profile whose knowledge base is waiting for a
//! re-embed passes the mark on to its fork.

use std::collections::HashMap;
use std::path::Path;

use super::*;
use crate::entities::self_model_history::VersionAuthor;

/// What a copy carried, counted — for the notice that answers `/profile fork`
/// and the lines `mindfork companion export|import` print.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForkCopy {
    pub notes: usize,
    pub links: usize,
    pub knowledge_sources: usize,
    pub knowledge_chunks: usize,
    /// The self-model's version the copy begins from; `None` — there was none.
    pub self_model_version: Option<u64>,
    pub self_model_versions: usize,
}

/// The schema a carry's other database is attached under.
const CARRY_SCHEMA: &str = "carry_src";

impl Db {
    /// Copies `from`'s memory to the profile `to` (which holds none yet),
    /// in one savepoint, vectors included. See the module doc for what is
    /// copied and what is not.
    pub fn profile_fork_copy(&self, from: Uuid, to: Uuid) -> Result<ForkCopy> {
        let conn = self.conn.lock().unwrap();
        let route = Route {
            src: "main",
            dst: "main",
            from: from.to_string(),
            to: to.to_string(),
            vectors: true,
        };
        in_savepoint(&conn, || {
            let counts = copy(&conn, &route)?;
            let stale = super::rag::read_stale_profiles(&conn)?;
            if stale.contains(&from) {
                let mut marks = stale;
                marks.push(to);
                super::rag::write_stale_profiles(&conn, &marks)?;
            }
            Ok(counts)
        })
    }

    /// Copies the profile `from` of **another database file** into this one as
    /// `to` — what carrying a companion is (`mindfork companion export|import`,
    /// docs/research/companion-platform.md C8 (b)): the export copies the live
    /// database's profile into an archive's own small one, the import copies
    /// that one's into the live database. The source is attached read-only.
    ///
    /// **No vectors cross**: the two databases need not share an embedder, and
    /// an embedding generation is a number only its own database can read. The
    /// notes are embedded again on the next semantic path, as any note missing
    /// its vector is; the knowledge base's chunks keep their text and wait for
    /// `/reindex` (the importer marks them so).
    pub fn profile_copy_from(&self, source: &Path, from: Uuid, to: Uuid) -> Result<ForkCopy> {
        if !super::is_sqlite_file(source) {
            bail!("{} is not a SQLite database", source.display());
        }
        let conn = self.conn.lock().unwrap();
        conn.execute(
            &format!("ATTACH DATABASE ?1 AS {CARRY_SCHEMA}"),
            params![read_only_uri(source)],
        )
        .with_context(|| format!("attaching {}", source.display()))?;
        let route = Route {
            src: CARRY_SCHEMA,
            dst: "main",
            from: from.to_string(),
            to: to.to_string(),
            vectors: false,
        };
        let copied = in_savepoint(&conn, || copy(&conn, &route));
        let detached = conn.execute_batch(&format!("DETACH DATABASE {CARRY_SCHEMA}"));
        let copied = copied?;
        detached?;
        Ok(copied)
    }
}

/// A file as a read-only SQLite URI (`ATTACH` reads URIs: rusqlite opens with
/// `SQLITE_OPEN_URI`). The path's own `%`, `?` and `#` are escaped so they stay
/// part of the name, and a Windows path gets the slashes and the extra `/` a
/// drive letter needs.
fn read_only_uri(path: &Path) -> String {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut text = absolute.to_string_lossy().replace('\\', "/");
    if let Some(stripped) = text.strip_prefix("//?/") {
        text = stripped.to_string();
    }
    let escaped: String = text
        .chars()
        .map(|c| match c {
            '%' => "%25".to_string(),
            '?' => "%3F".to_string(),
            '#' => "%23".to_string(),
            c => c.to_string(),
        })
        .collect();
    let lead = if escaped.starts_with('/') {
        "file://"
    } else {
        "file:///"
    };
    format!("{lead}{escaped}?mode=ro")
}

/// Runs `f` inside a savepoint — released when it succeeds, rolled back when
/// it fails, so a copy that fails halfway leaves nothing behind.
fn in_savepoint<T>(conn: &Connection, f: impl FnOnce() -> Result<T>) -> Result<T> {
    conn.execute_batch("SAVEPOINT profile_copy")?;
    match f() {
        Ok(value) => {
            conn.execute_batch("RELEASE profile_copy")?;
            Ok(value)
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK TO profile_copy; RELEASE profile_copy");
            Err(err)
        }
    }
}

/// Where a copy reads and writes: two schemas of one connection — `main`
/// twice for a fork, an attached file into `main` for a carry — and whether
/// the vectors come along.
struct Route<'a> {
    src: &'a str,
    dst: &'a str,
    from: String,
    to: String,
    vectors: bool,
}

impl Route<'_> {
    /// Whether the source has `table` — a database from an older build may
    /// not, and then it has nothing of that kind to copy.
    fn has(&self, conn: &Connection, table: &str) -> Result<bool> {
        let found: i64 = conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM {}.sqlite_master WHERE type = 'table' AND name = ?1",
                self.src
            ),
            params![table],
            |r| r.get(0),
        )?;
        Ok(found > 0)
    }
}

fn copy(conn: &Connection, route: &Route<'_>) -> Result<ForkCopy> {
    let mut counts = ForkCopy::default();
    let notes = copy_notes(conn, route)?;
    counts.notes = notes.len();
    counts.links = copy_note_edges(conn, route, &notes)?;
    let (sources, chunks) = copy_knowledge(conn, route)?;
    counts.knowledge_sources = sources;
    counts.knowledge_chunks = chunks;
    let (version, versions) = copy_self_model(conn, route)?;
    counts.self_model_version = version;
    counts.self_model_versions = versions;
    if route.has(conn, "llm_history")? {
        conn.execute(
            &format!(
                "INSERT INTO {dst}.llm_history(profile_id, changed_at, model, mode)
                 SELECT ?2, changed_at, model, mode FROM {src}.llm_history
                 WHERE profile_id = ?1 ORDER BY rowid",
                src = route.src,
                dst = route.dst
            ),
            params![route.from, route.to],
        )?;
    }
    Ok(counts)
}

/// The notes, each under a new id — with their vectors on a fork. Answers the
/// map from each old id to its new one.
fn copy_notes(conn: &Connection, route: &Route<'_>) -> Result<HashMap<String, String>> {
    let (src, dst) = (route.src, route.dst);
    let mut map = HashMap::new();
    let notes: Vec<(String, String, String, String, String)> = conn
        .prepare(&format!(
            "SELECT id, content, tags, created_at, updated_at FROM {src}.notes
             WHERE profile_id = ?1 ORDER BY rowid"
        ))?
        .query_map(params![route.from], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    for (id, content, tags, created_at, updated_at) in notes {
        let new = Uuid::new_v4().to_string();
        conn.execute(
            &format!(
                "INSERT INTO {dst}.notes(id, profile_id, content, tags, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
            ),
            params![new, route.to, content, tags, created_at, updated_at],
        )?;
        if route.vectors {
            conn.execute(
                &format!(
                    "INSERT INTO {dst}.note_vectors(note_id, profile_id, embedding, embed_gen)
                     SELECT ?2, ?3, embedding, embed_gen FROM {src}.note_vectors
                     WHERE note_id = ?1"
                ),
                params![id, new, route.to],
            )?;
        }
        map.insert(id, new);
    }
    Ok(map)
}

/// The notes' links, scars and citations, their note ids remapped. Answers
/// how many links were copied.
fn copy_note_edges(
    conn: &Connection,
    route: &Route<'_>,
    map: &HashMap<String, String>,
) -> Result<usize> {
    let (src, dst) = (route.src, route.dst);
    let mut copied = 0;
    if route.has(conn, "note_links")? {
        let links: Vec<(String, String, String, String)> = conn
            .prepare(&format!(
                "SELECT from_id, to_id, relation, created_at FROM {src}.note_links
                 WHERE profile_id = ?1"
            ))?
            .query_map(params![route.from], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        for (a, b, relation, created_at) in links {
            let (Some(a), Some(b)) = (map.get(&a), map.get(&b)) else {
                continue;
            };
            conn.execute(
                &format!(
                    "INSERT INTO {dst}.note_links(profile_id, from_id, to_id, relation, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)"
                ),
                params![route.to, a, b, relation, created_at],
            )?;
            copied += 1;
        }
    }
    if route.has(conn, "note_superseded")? {
        let scars: Vec<(String, String, String)> = conn
            .prepare(&format!(
                "SELECT note_id, superseded_by, superseded_at FROM {src}.note_superseded
                 WHERE profile_id = ?1"
            ))?
            .query_map(params![route.from], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        for (note, by, at) in scars {
            let (Some(note), Some(by)) = (map.get(&note), map.get(&by)) else {
                continue;
            };
            conn.execute(
                &format!(
                    "INSERT INTO {dst}.note_superseded(note_id, profile_id, superseded_by, superseded_at)
                     VALUES (?1, ?2, ?3, ?4)"
                ),
                params![note, route.to, by, at],
            )?;
        }
    }
    if route.has(conn, "note_rag_links")? {
        let citations: Vec<(String, String, String)> = conn
            .prepare(&format!(
                "SELECT note_id, source, created_at FROM {src}.note_rag_links
                 WHERE profile_id = ?1"
            ))?
            .query_map(params![route.from], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        for (note, source, created_at) in citations {
            let Some(note) = map.get(&note) else { continue };
            conn.execute(
                &format!(
                    "INSERT INTO {dst}.note_rag_links(profile_id, note_id, source, created_at)
                     VALUES (?1, ?2, ?3, ?4)"
                ),
                params![route.to, note, source, created_at],
            )?;
        }
    }
    Ok(copied)
}

/// The knowledge base: its sources, and its chunks under new ids and rowids —
/// with their vectors on a fork, and with no generation on a carry, which is
/// what puts them in the re-embed queue. Answers (sources, chunks).
fn copy_knowledge(conn: &Connection, route: &Route<'_>) -> Result<(usize, usize)> {
    let (src, dst) = (route.src, route.dst);
    let sources = if route.has(conn, "rag_sources")? {
        conn.execute(
            &format!(
                "INSERT INTO {dst}.rag_sources(profile_id, source, content, created_at)
                 SELECT ?2, source, content, created_at FROM {src}.rag_sources
                 WHERE profile_id = ?1"
            ),
            params![route.from, route.to],
        )?
    } else {
        0
    };
    let with_vectors = route.vectors && table_exists(conn, "rag_vectors")?;
    let chunks: Vec<(i64, String, String, String, Option<i64>)> = conn
        .prepare(&format!(
            "SELECT rowid, source, chunk_text, created_at, embed_gen FROM {src}.rag_documents
             WHERE profile_id = ?1 ORDER BY rowid"
        ))?
        .query_map(params![route.from], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    for (rowid, source, text, created_at, generation) in &chunks {
        let generation = if route.vectors { *generation } else { None };
        conn.execute(
            &format!(
                "INSERT INTO {dst}.rag_documents
                     (id, profile_id, source, chunk_text, created_at, embed_gen)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
            ),
            params![
                Uuid::new_v4().to_string(),
                route.to,
                source,
                text,
                created_at,
                generation
            ],
        )?;
        let new_rowid = conn.last_insert_rowid();
        if !with_vectors {
            continue;
        }
        let vector: Option<Vec<u8>> = conn
            .query_row(
                "SELECT embedding FROM rag_vectors WHERE rowid = ?1",
                params![rowid],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(vector) = vector {
            conn.execute(
                "INSERT INTO rag_vectors(rowid, profile_id, embedding) VALUES (?1, ?2, ?3)",
                params![new_rowid, route.to, vector],
            )?;
        }
    }
    Ok((sources, chunks.len()))
}

/// The self-model and every version of it, the profile id inside each model
/// rewritten to the copy's. Answers the version the copy begins from and how
/// many versions were copied. A model the history has not recorded yet is
/// recorded first, as the store's own write would (`unknown`), so the copy's
/// history ends at the model it was given.
fn copy_self_model(conn: &Connection, route: &Route<'_>) -> Result<(Option<u64>, usize)> {
    let (src, dst) = (route.src, route.dst);
    if !route.has(conn, "self_models")? {
        return Ok((None, 0));
    }
    let current: Option<(String, i64, String)> = conn
        .query_row(
            &format!(
                "SELECT data, version, updated_at FROM {src}.self_models WHERE profile_id = ?1"
            ),
            params![route.from],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((data, version, updated_at)) = current else {
        return Ok((None, 0));
    };
    let mut versions: Vec<StoredRow> = Vec::new();
    if route.has(conn, "self_model_versions")? {
        versions = conn
            .prepare(&format!(
                "SELECT version, data, written_at, author, model, mode, chat_id
                 FROM {src}.self_model_versions WHERE profile_id = ?1 ORDER BY rowid"
            ))?
            .query_map(params![route.from], |r| {
                Ok(StoredRow {
                    version: r.get(0)?,
                    data: r.get(1)?,
                    written_at: r.get(2)?,
                    author: r.get(3)?,
                    model: r.get(4)?,
                    mode: r.get(5)?,
                    chat_id: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
    }
    if versions.last().is_none_or(|v| v.version < version) {
        versions.push(StoredRow {
            version,
            data: data.clone(),
            written_at: updated_at.clone(),
            author: VersionAuthor::Unknown.kind_key().to_string(),
            model: None,
            mode: None,
            chat_id: None,
        });
    }
    conn.execute(
        &format!(
            "INSERT INTO {dst}.self_models(profile_id, data, version, updated_at)
             VALUES (?1, ?2, ?3, ?4)"
        ),
        params![route.to, owned_by(&data, &route.to)?, version, updated_at],
    )?;
    for v in &versions {
        conn.execute(
            &format!(
                "INSERT INTO {dst}.self_model_versions
                     (profile_id, version, data, written_at, author, model, mode, chat_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
            ),
            params![
                route.to,
                v.version,
                owned_by(&v.data, &route.to)?,
                v.written_at,
                v.author,
                v.model,
                v.mode,
                v.chat_id
            ],
        )?;
    }
    Ok((Some(version.max(0) as u64), versions.len()))
}

/// A history row as stored, copied verbatim apart from its owner.
struct StoredRow {
    version: i64,
    data: String,
    written_at: String,
    author: String,
    model: Option<String>,
    mode: Option<String>,
    chat_id: Option<String>,
}

/// A stored self-model with its `profile_id` set to `owner` — the model names
/// its profile inside its JSON. Everything else stays as written.
fn owned_by(data: &str, owner: &str) -> Result<String> {
    let mut value: serde_json::Value =
        serde_json::from_str(data).context("a stored self-model is not JSON")?;
    if let Some(object) = value.as_object_mut() {
        object.insert("profile_id".into(), serde_json::Value::String(owner.into()));
    }
    Ok(serde_json::to_string(&value)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::note::Note;
    use crate::entities::profile::LlmChange;
    use crate::entities::rag::RagDocument;
    use crate::shared::config::ServerMode;

    fn chunk(profile: Uuid, source: &str, text: &str, v: f32) -> RagDocument {
        RagDocument {
            id: Uuid::new_v4(),
            profile_id: profile,
            source: source.into(),
            chunk_text: text.into(),
            embedding: vec![v, 1.0 - v, 0.5],
            created_at: Utc::now(),
        }
    }

    /// A profile with a little of everything: two notes, one superseding the
    /// other and linked to it, one citing a source; a knowledge base of two
    /// chunks; a self-model written twice; a model history.
    fn seeded(db: &Db) -> (Uuid, Note, Note) {
        let p = Uuid::new_v4();
        let old = Note::new(p, "Alex likes chess", vec!["@self".into()]);
        let new = Note::new(p, "Alex plays chess on Sundays", vec![]);
        db.note_insert(&old).unwrap();
        db.note_insert(&new).unwrap();
        db.note_vector_upsert(new.id, p, &[0.1, 0.2, 0.3]).unwrap();
        db.note_link_insert(p, new.id, old.id, "refines").unwrap();
        db.note_supersede_mark(p, old.id, new.id).unwrap();
        db.rag_source_upsert(p, "chess.md", "Openings and endgames.", Utc::now())
            .unwrap();
        db.rag_insert(&chunk(p, "chess.md", "Openings", 0.2))
            .unwrap();
        db.rag_insert(&chunk(p, "chess.md", "Endgames", 0.8))
            .unwrap();
        db.note_cite_source_insert(p, new.id, "chess.md").unwrap();
        for text in ["I am Sol.", "I am Sol, Alex's friend."] {
            db.self_model_update(p, &VersionAuthor::Person, |m| {
                m.summary = text.into();
                true
            })
            .unwrap();
        }
        db.llm_history_note(
            p,
            &LlmChange {
                changed_at: Utc::now(),
                model: "gemma-4-31b".into(),
                mode: ServerMode::External,
            },
        )
        .unwrap();
        (p, old, new)
    }

    #[test]
    fn a_fork_gets_the_whole_memory_under_its_own_ids() {
        let db = Db::open_in_memory().unwrap();
        let (from, old, new) = seeded(&db);
        let to = Uuid::new_v4();
        let counts = db.profile_fork_copy(from, to).unwrap();
        assert_eq!(
            counts,
            ForkCopy {
                notes: 2,
                links: 1,
                knowledge_sources: 1,
                knowledge_chunks: 2,
                self_model_version: Some(2),
                self_model_versions: 2,
            }
        );

        // The notes: the same texts under new ids; the superseded one hidden
        // as it was, by the fork's own copy of its successor.
        let notes = db.note_list(to, None, &[], None).unwrap();
        assert_eq!(notes.len(), 1, "the superseded note stays out of listings");
        let kept = &notes[0];
        assert_eq!(kept.content, new.content);
        assert_ne!(kept.id, new.id);
        let scarred = db.note_superseded_by(to, kept.id).unwrap();
        assert_eq!(scarred.len(), 1);
        assert_eq!(scarred[0].content, old.content);
        assert_ne!(scarred[0].id, old.id);
        // Its link and vector came along, remapped onto the fork's notes (a
        // superseded neighbour is hidden from `note_neighbors`, so the raw
        // edge is read).
        assert_eq!(
            db.note_links_all(to).unwrap(),
            vec![(kept.id, scarred[0].id, "refines".to_string())]
        );
        assert!(
            db.notes_missing_vectors(to)
                .unwrap()
                .iter()
                .all(|(id, _)| *id != kept.id)
        );
        assert_eq!(db.notes_citing_source(to, "chess.md").unwrap().len(), 1);

        // The knowledge base answers a query in the fork, from its own rows.
        let hits = db.rag_search(to, &[0.8, 0.2, 0.5], 2).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].chunk_text, "Endgames");
        assert_eq!(db.rag_list_sources(to).unwrap().len(), 1);

        // The self-model and its history, owned by the fork.
        let model = db.self_model_get(to).unwrap().unwrap();
        assert_eq!((model.profile_id, model.version), (to, 2));
        let history = db.self_model_history(to).unwrap();
        assert_eq!(history.len(), 2);
        assert!(history.iter().all(|v| v.model.profile_id == to));
        assert_eq!(db.llm_history(to).unwrap().len(), 1);

        // The source is untouched, and the two diverge from here.
        assert_eq!(db.note_list(from, None, &[], None).unwrap().len(), 1);
        assert_eq!(db.rag_search(from, &[0.8, 0.2, 0.5], 5).unwrap().len(), 2);
        db.self_model_update(to, &VersionAuthor::Person, |m| {
            m.summary = "I am Ada.".into();
            true
        })
        .unwrap();
        assert_eq!(
            db.self_model_get(from).unwrap().unwrap().summary,
            "I am Sol, Alex's friend."
        );
        assert_eq!(db.self_model_history(to).unwrap().len(), 3);
        assert_eq!(db.self_model_history(from).unwrap().len(), 2);
    }

    /// A model the history has not recorded (written before it was kept) is
    /// the fork's last version, as it is the source's.
    #[test]
    fn an_unrecorded_model_is_the_forks_last_version() {
        let db = Db::open_in_memory().unwrap();
        let from = Uuid::new_v4();
        db.self_model_update(from, &VersionAuthor::Person, |m| {
            m.summary = "old".into();
            true
        })
        .unwrap();
        db.conn
            .lock()
            .unwrap()
            .execute_batch("DELETE FROM self_model_versions")
            .unwrap();
        let to = Uuid::new_v4();
        let counts = db.profile_fork_copy(from, to).unwrap();
        assert_eq!(
            (counts.self_model_version, counts.self_model_versions),
            (Some(1), 1)
        );
        let history = db.self_model_history(to).unwrap();
        assert_eq!(history[0].author, VersionAuthor::Unknown);
        assert_eq!(history[0].model.summary, "old");
    }

    #[test]
    fn an_empty_profile_forks_to_an_empty_one() {
        let db = Db::open_in_memory().unwrap();
        let counts = db
            .profile_fork_copy(Uuid::new_v4(), Uuid::new_v4())
            .unwrap();
        assert_eq!(counts, ForkCopy::default());
    }

    #[test]
    fn a_stale_knowledge_base_passes_its_mark_on() {
        let db = Db::open_in_memory().unwrap();
        let (from, _, _) = seeded(&db);
        db.set_rag_stale_profiles(&[from]).unwrap();
        let to = Uuid::new_v4();
        db.profile_fork_copy(from, to).unwrap();
        assert!(db.rag_is_stale(to).unwrap());
        assert!(db.rag_is_stale(from).unwrap());
    }

    /// A copy that fails halfway leaves nothing of the fork.
    #[test]
    fn a_failed_fork_leaves_nothing() {
        let db = Db::open_in_memory().unwrap();
        let (from, _, _) = seeded(&db);
        db.conn
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER boom BEFORE INSERT ON llm_history
                 BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            )
            .unwrap();
        let to = Uuid::new_v4();
        assert!(db.profile_fork_copy(from, to).is_err());
        assert!(db.note_list(to, None, &[], None).unwrap().is_empty());
        assert!(db.self_model_get(to).unwrap().is_none());
        assert!(db.rag_list_sources(to).unwrap().is_empty());
    }

    /// Carrying a profile from another database file: the memory crosses under
    /// new ids, owned by the profile it lands in — without vectors, which wait
    /// to be made again — and the source file is left byte for byte as it was,
    /// even under a path with characters a URI would read otherwise.
    #[test]
    fn a_carry_copies_from_another_file_without_vectors() {
        let dir = tempfile::tempdir().unwrap();
        let odd = dir.path().join("a b%c#d");
        std::fs::create_dir_all(&odd).unwrap();
        let source_path = odd.join("data.db");
        let from;
        {
            let source = Db::open(&source_path).unwrap();
            (from, _, _) = seeded(&source);
        }
        let before = std::fs::read(&source_path).unwrap();
        let dest = Db::open(&dir.path().join("dest.db")).unwrap();
        let to = Uuid::new_v4();
        let counts = dest.profile_copy_from(&source_path, from, to).unwrap();
        assert_eq!(
            (
                counts.notes,
                counts.links,
                counts.knowledge_chunks,
                counts.self_model_versions
            ),
            (2, 1, 2, 2)
        );
        assert_eq!(
            std::fs::read(&source_path).unwrap(),
            before,
            "the source is read only"
        );

        // The memory is the profile's own; nothing of the source's profile id.
        assert_eq!(dest.note_list(to, None, &[], None).unwrap().len(), 1);
        assert!(dest.note_list(from, None, &[], None).unwrap().is_empty());
        let model = dest.self_model_get(to).unwrap().unwrap();
        assert_eq!((model.profile_id, model.version), (to, 2));
        assert_eq!(dest.llm_history(to).unwrap().len(), 1);
        // No vector crossed: the note in use waits to be embedded again (the
        // superseded one is never embedded), and the knowledge base answers
        // nothing until it is re-embedded.
        assert_eq!(dest.notes_missing_vectors(to).unwrap().len(), 1);
        assert!(dest.rag_search(to, &[0.8, 0.2, 0.5], 5).unwrap().is_empty());
        assert_eq!(dest.rag_list_sources(to).unwrap().len(), 1);
        // The attachment is gone once the copy is done.
        let attached: i64 = dest
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM pragma_database_list", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(attached, 1);
    }

    #[test]
    fn a_carry_refuses_a_file_that_is_no_database() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("memory.db");
        std::fs::write(&junk, b"not a database").unwrap();
        let dest = Db::open_in_memory().unwrap();
        let err = dest
            .profile_copy_from(&junk, Uuid::new_v4(), Uuid::new_v4())
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("not a SQLite database"),
            "{err:#}"
        );
    }

    #[test]
    fn a_read_only_uri_keeps_the_path_whole() {
        let uri = read_only_uri(Path::new("/srv/a b/x%y#z?.db"));
        assert!(uri.starts_with("file://"), "{uri}");
        assert!(uri.ends_with("a b/x%25y%23z%3F.db?mode=ro"), "{uri}");
    }
}

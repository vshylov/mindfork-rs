//! A story's summary for `mindfork stats` (docs/fabula.md §4, PR 7; spec §12.4):
//! read-only, and readable whatever wrote the file.
//!
//! The file is opened read-only, or loaded from a backup's entry into memory,
//! and never through [`super::StoryDb`]: that would run the DDL and take the
//! file's claim, and a summary must leave the copy it summarizes as it was. A
//! table the file lacks counts as empty, so an older or a newer story is still
//! counted.
//!
//! **The identity a comparison works on** is a list of *units*, one per row,
//! each `<table>/<key>:<digest>`. Every table of a story is append-only (spec
//! §6) and its ids are derived from position, so two copies of one story share
//! the rows written before they parted, and a row is never changed after: a
//! copy whose units include all of another's holds everything that one does.
//! Ids alone would not do, since two machines continuing one story write the
//! same ids with different content, and that is what the digest tells apart.
//! Two tables are read differently: a witness belongs to its event's unit, as
//! the two are written together, and only a step that is done counts, without
//! its time, since a failed step is retried and a done one may be recorded
//! again.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;
use rusqlite::types::ValueRef;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::entities::chat_file::sha256_hex;
use crate::shared::storage::db::{has_table, open_file, open_image};

/// How many hex digits of a row's SHA-256 its unit carries.
const UNIT_DIGEST_HEX: usize = 12;

/// The tables a story's identity is drawn from, with what keys a row
/// (`CAST … AS TEXT`, so a number keys as text) and which rows count.
const UNIT_TABLES: &[(&str, &str, &str)] = &[
    ("run", "id", ""),
    ("step", "run_id || '/' || address", " WHERE status = 'done'"),
    ("canon_doc", "id", ""),
    ("canon_approval", "canon_doc_id", ""),
    ("scene", "id", ""),
    ("branch", "id", ""),
    ("proposal", "id", ""),
    ("event", "id", ""),
    ("reflection", "id", ""),
    ("outline", "chapter_no", ""),
    ("chapter", "id", ""),
    ("chapter_summary", "chapter_id", ""),
    ("review_note", "id", ""),
    ("pick", "scene_id", ""),
    ("chapter_acceptance", "chapter_id", ""),
    ("note_decision", "note_id", ""),
    ("question", "id", ""),
    ("answer", "question_id", ""),
    ("assumption", "id", ""),
];

/// What `mindfork stats` reports of one story.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoryStats {
    /// The file's `PRAGMA user_version`.
    pub schema: u32,
    /// Chapters with at least one version, and of them those accepted at P2.
    pub chapters: u64,
    pub accepted_chapters: u64,
    /// Every version of every chapter: drafts, revisions, the human's.
    pub chapter_versions: u64,
    pub scenes: u64,
    pub branches: u64,
    pub events: u64,
    /// The engine's runs, a resume being a run of its own (spec §9).
    pub runs: u64,
    /// The questions asked, and of them those not answered yet.
    pub questions: u64,
    pub pending_questions: u64,
    /// One per row, sorted: what a comparison lines two copies up by.
    pub units: Vec<String>,
}

/// The story database file at `path`, which is left untouched.
pub fn story_stats_of_file(path: &Path) -> Result<StoryStats> {
    collect(&open_file(path)?)
}

/// A story database **image** of exactly `size` bytes, as it comes out of a
/// backup archive.
pub fn story_stats_of_image(image: impl Read, size: usize) -> Result<StoryStats> {
    collect(&open_image(image, size)?)
}

fn collect(conn: &Connection) -> Result<StoryStats> {
    let schema: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    Ok(StoryStats {
        schema,
        chapters: count(
            conn,
            "chapter",
            "SELECT COUNT(DISTINCT number) FROM chapter",
        )?,
        accepted_chapters: if has_table(conn, "chapter_acceptance")? {
            count(
                conn,
                "chapter",
                "SELECT COUNT(DISTINCT c.number) FROM chapter c
                 JOIN chapter_acceptance a ON a.chapter_id = c.id",
            )?
        } else {
            0
        },
        chapter_versions: count(conn, "chapter", "SELECT COUNT(*) FROM chapter")?,
        scenes: count(conn, "scene", "SELECT COUNT(*) FROM scene")?,
        branches: count(conn, "branch", "SELECT COUNT(*) FROM branch")?,
        events: count(conn, "event", "SELECT COUNT(*) FROM event")?,
        runs: count(conn, "run", "SELECT COUNT(*) FROM run")?,
        questions: count(conn, "question", "SELECT COUNT(*) FROM question")?,
        pending_questions: if has_table(conn, "answer")? {
            count(
                conn,
                "question",
                "SELECT COUNT(*) FROM question q
                 WHERE NOT EXISTS (SELECT 1 FROM answer a WHERE a.question_id = q.id)",
            )?
        } else {
            count(conn, "question", "SELECT COUNT(*) FROM question")?
        },
        units: units(conn)?,
    })
}

/// Runs the counting `sql`, or answers zero when `table` does not exist.
fn count(conn: &Connection, table: &str, sql: &str) -> Result<u64> {
    if !has_table(conn, table)? {
        return Ok(0);
    }
    let n: i64 = conn
        .query_row(sql, [], |row| row.get(0))
        .with_context(|| format!("counting {table}"))?;
    Ok(u64::try_from(n).unwrap_or(0))
}

/// Every unit of the story, sorted ([`StoryStats::units`]).
fn units(conn: &Connection) -> Result<Vec<String>> {
    let witnesses = witnesses(conn)?;
    let mut out = Vec::new();
    for (table, key, filter) in UNIT_TABLES {
        if !has_table(conn, table)? {
            continue;
        }
        // A step's unit is its key alone: done is all it says.
        let columns = if *table == "step" { "status" } else { "*" };
        let sql = format!("SELECT CAST({key} AS TEXT), {columns} FROM {table}{filter}");
        let mut stmt = conn
            .prepare(&sql)
            .with_context(|| format!("reading {table}"))?;
        let width = stmt.column_count();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let key: String = row.get(0)?;
            let mut values = (1..width)
                .map(|i| row.get_ref(i).map(value))
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if *table == "event"
                && let Some(seen) = witnesses.get(&key)
            {
                values.push(Value::from(seen.clone()));
            }
            out.push(unit(table, &key, &values));
        }
    }
    out.sort();
    Ok(out)
}

/// Each event's witnesses, in key order.
fn witnesses(conn: &Connection) -> Result<BTreeMap<String, Vec<String>>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if !has_table(conn, "event_witness")? {
        return Ok(out);
    }
    let mut stmt = conn.prepare(
        "SELECT event_id, character_key FROM event_witness ORDER BY event_id, character_key",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (event, witness) = row?;
        out.entry(event).or_default().push(witness);
    }
    Ok(out)
}

/// `<table>/<key>:<digest>` over the row's values.
fn unit(table: &str, key: &str, values: &[Value]) -> String {
    // Strings, numbers and nulls: nothing in it can fail to serialize.
    let material = serde_json::to_vec(&(table, key, values)).unwrap_or_default();
    let mut digest = sha256_hex(&material);
    digest.truncate(UNIT_DIGEST_HEX);
    format!("{table}/{key}:{digest}")
}

/// A stored value as JSON; a blob as its hex, which no story table holds.
fn value(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(n) => Value::from(n),
        ValueRef::Real(x) => Value::from(x),
        ValueRef::Text(t) => Value::from(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Value::from(
            b.iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
        ),
    }
}

#[cfg(test)]
#[path = "stats_tests.rs"]
mod stats_tests;

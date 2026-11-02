//! A story's two files and their schemas (docs/research/fabula-engine.md §6,
//! ADR 0006).
//!
//! The story database holds every store but the call log; the call log is a
//! sibling file kept outside backups (spec §15), which `run_id` reaches by
//! value. Each file carries its own `PRAGMA user_version`
//! ([`STORY_SCHEMA`], [`STORY_CALLS_SCHEMA`]), read **before** any DDL runs,
//! so a newer file is refused without a byte written into it. The DDL itself
//! is `CREATE … IF NOT EXISTS` and runs on every open: an additive table needs
//! no step (the bump policy in `shared::storage::schema`).
//!
//! What differs from the spec's sketch, and why:
//!
//! - **Append-only beyond `event`.** "Nothing is updated in place" (spec §6)
//!   is enforced by triggers on every table but `run` and `step`: a step may
//!   move from `failed` to `done` when a run resumes (spec §9).
//! - **`event` stores text, not JSON, where the content is text**:
//!   `observable` and `private` (null when the actor had no thought), and its
//!   visibility and cause as JSON of their own (`visibility_json`,
//!   `cause_json`).
//! - **`canon_doc` keeps the author's order** (`position`) and marks a removed
//!   document by a version whose `body_json` is null.
//! - **`llm_call.cost_usd`**: the dollar cap applies where a provider reports
//!   cost (docs/fabula.md §6, T1).
//! - **The manuscript's own tables** (docs/fabula.md §4, PR 6): `outline`
//!   holds a chapter's goals; `chapter` records each version's `origin`
//!   (draft, revision, the human's revision), and a version's summary is a
//!   row of `chapter_summary`, since it is written only once the version is
//!   final; `question`, `answer` and `assumption` are the pauses (spec §14).
//!   The file was in no release when they came, so the sketch's `chapter`
//!   changed in place.
//! - **The default rollback journal, not WAL.** Every read and write goes
//!   through the database actor, so WAL's concurrent readers buy nothing, and
//!   a backup can take the file as `data.db`'s is taken (`VACUUM INTO` from a
//!   read-only connection), with no `-wal` beside it to handle.

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use super::StoryDbError;
use crate::shared::storage::db::is_sqlite_file;
use crate::shared::storage::schema::{STORY_CALLS_SCHEMA, STORY_SCHEMA};

/// The story database's tables (spec §6). Foreign keys are on for every
/// connection ([`open_file`]).
const STORY_DDL: &str = "
CREATE TABLE IF NOT EXISTS run (
    id          TEXT PRIMARY KEY,
    config_json TEXT NOT NULL,
    started_at  TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS step (
    run_id      TEXT NOT NULL REFERENCES run(id),
    address     TEXT NOT NULL,
    status      TEXT NOT NULL CHECK (status IN ('done', 'failed')),
    finished_at TEXT NOT NULL,
    PRIMARY KEY (run_id, address)
);
CREATE TABLE IF NOT EXISTS canon_doc (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL CHECK (kind IN ('world', 'location', 'character', 'style', 'arc')),
    key        TEXT NOT NULL,
    version    INTEGER NOT NULL,
    position   INTEGER NOT NULL,
    body_json  TEXT,
    created_at TEXT NOT NULL,
    UNIQUE (kind, key, version)
);
CREATE TABLE IF NOT EXISTS canon_approval (
    canon_doc_id TEXT PRIMARY KEY REFERENCES canon_doc(id),
    approved_at  TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS scene (
    id            TEXT PRIMARY KEY,
    chapter_no    INTEGER NOT NULL,
    scene_no      INTEGER NOT NULL,
    rerun         INTEGER NOT NULL,
    setup_json    TEXT NOT NULL,
    author_intent TEXT NOT NULL,
    UNIQUE (chapter_no, scene_no, rerun)
);
CREATE TABLE IF NOT EXISTS branch (
    id        TEXT PRIMARY KEY,
    parent_id TEXT REFERENCES branch(id),
    fork_seq  INTEGER,
    scene_id  TEXT REFERENCES scene(id),
    k         INTEGER NOT NULL,
    seed      INTEGER NOT NULL,
    CHECK ((parent_id IS NULL) = (fork_seq IS NULL))
);
CREATE TABLE IF NOT EXISTS proposal (
    id            TEXT PRIMARY KEY,
    branch_id     TEXT NOT NULL REFERENCES branch(id),
    round         INTEGER NOT NULL,
    character_key TEXT NOT NULL,
    body_json     TEXT NOT NULL,
    verdict       TEXT NOT NULL CHECK (verdict IN ('committed', 'refused', 'lapsed')),
    reason        TEXT
);
CREATE INDEX IF NOT EXISTS proposal_by_branch ON proposal(branch_id);
CREATE TABLE IF NOT EXISTS event (
    seq             INTEGER PRIMARY KEY AUTOINCREMENT,
    id              TEXT NOT NULL UNIQUE,
    branch_id       TEXT NOT NULL REFERENCES branch(id),
    round           INTEGER NOT NULL,
    tick            INTEGER NOT NULL,
    location_key    TEXT NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN
                        ('opening', 'speech', 'action', 'movement', 'world_event', 'settle')),
    actor_key       TEXT,
    visibility_json TEXT NOT NULL,
    observable      TEXT NOT NULL,
    private         TEXT,
    effects_json    TEXT NOT NULL,
    proposal_id     TEXT REFERENCES proposal(id),
    cause_json      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS event_by_branch ON event(branch_id, seq);
CREATE TABLE IF NOT EXISTS event_witness (
    event_id      TEXT NOT NULL REFERENCES event(id),
    character_key TEXT NOT NULL,
    PRIMARY KEY (event_id, character_key)
);
CREATE TABLE IF NOT EXISTS reflection (
    id                TEXT PRIMARY KEY,
    branch_id         TEXT NOT NULL REFERENCES branch(id),
    character_key     TEXT NOT NULL,
    after_seq         INTEGER NOT NULL,
    body              TEXT NOT NULL,
    goal_changes_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS outline (
    chapter_no INTEGER PRIMARY KEY,
    body_json  TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS chapter (
    id      TEXT PRIMARY KEY,
    number  INTEGER NOT NULL,
    version INTEGER NOT NULL,
    origin  TEXT NOT NULL CHECK (origin IN ('draft', 'revision', 'human')),
    text_md TEXT NOT NULL,
    UNIQUE (number, version)
);
CREATE TABLE IF NOT EXISTS chapter_summary (
    chapter_id TEXT PRIMARY KEY REFERENCES chapter(id),
    summary    TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS review_note (
    id            TEXT PRIMARY KEY,
    chapter_id    TEXT NOT NULL REFERENCES chapter(id),
    reviewer      TEXT NOT NULL CHECK (reviewer IN ('reader', 'continuity')),
    anchor        TEXT,
    kind          TEXT NOT NULL,
    severity      TEXT NOT NULL CHECK (severity IN ('low', 'high')),
    body          TEXT NOT NULL,
    evidence_json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS review_note_by_chapter ON review_note(chapter_id);
CREATE TABLE IF NOT EXISTS pick (
    scene_id  TEXT PRIMARY KEY REFERENCES scene(id),
    branch_id TEXT REFERENCES branch(id),
    rationale TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS chapter_acceptance (
    chapter_id  TEXT PRIMARY KEY REFERENCES chapter(id),
    accepted_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS note_decision (
    note_id  TEXT PRIMARY KEY REFERENCES review_note(id),
    decision TEXT NOT NULL CHECK (decision IN ('accepted', 'rejected', 'deferred')),
    reason   TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS question (
    id         TEXT PRIMARY KEY,
    run_id     TEXT NOT NULL REFERENCES run(id),
    step       TEXT NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('canon', 'chapter', 'scene')),
    body_json  TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS answer (
    question_id   TEXT PRIMARY KEY REFERENCES question(id),
    decision      TEXT NOT NULL CHECK (decision IN ('approve', 'change', 'answer')),
    notes         TEXT NOT NULL,
    evidence_json TEXT NOT NULL,
    created_at    TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS assumption (
    id     TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES run(id),
    step   TEXT NOT NULL,
    role   TEXT NOT NULL,
    doubt  TEXT NOT NULL,
    choice TEXT NOT NULL
);
";

/// The story database's tables whose rows are never updated or deleted.
pub(super) const STORY_APPEND_ONLY: &[&str] = &[
    "canon_doc",
    "canon_approval",
    "scene",
    "branch",
    "proposal",
    "event",
    "event_witness",
    "reflection",
    "outline",
    "chapter",
    "chapter_summary",
    "review_note",
    "pick",
    "chapter_acceptance",
    "note_decision",
    "question",
    "answer",
    "assumption",
];

/// The call log: every model call, keyed by its logical address so a replay
/// is exact (spec §6, §9).
const CALLS_DDL: &str = "
CREATE TABLE IF NOT EXISTS llm_call (
    id            TEXT PRIMARY KEY,
    run_id        TEXT NOT NULL,
    address       TEXT NOT NULL,
    attempt       INTEGER NOT NULL,
    role          TEXT NOT NULL,
    model         TEXT NOT NULL,
    params_json   TEXT NOT NULL,
    mechanism     TEXT NOT NULL CHECK (mechanism IN ('schema', 'fallback', 'text')),
    request_json  TEXT NOT NULL,
    prompt_sha256 TEXT NOT NULL,
    response_json TEXT,
    status        TEXT NOT NULL CHECK (status IN ('ok', 'cut', 'refused', 'error')),
    input_tokens  INTEGER,
    output_tokens INTEGER,
    cached_tokens INTEGER,
    cost_usd      REAL,
    latency_ms    INTEGER,
    created_at    TEXT NOT NULL,
    UNIQUE (run_id, address, attempt)
);
";

/// The call log's append-only tables.
pub(super) const CALLS_APPEND_ONLY: &[&str] = &["llm_call"];

/// One of a story's two files.
pub(super) struct FileSpec {
    /// Its name in errors and logs.
    pub name: &'static str,
    /// Its tables.
    pub ddl: &'static str,
    /// Its tables that refuse `UPDATE` and `DELETE`.
    pub append_only: &'static [&'static str],
    /// The schema this build writes.
    pub current: u32,
}

pub(super) const STORY_FILE: FileSpec = FileSpec {
    name: "story database",
    ddl: STORY_DDL,
    append_only: STORY_APPEND_ONLY,
    current: STORY_SCHEMA,
};

pub(super) const CALLS_FILE: FileSpec = FileSpec {
    name: "story call log",
    ddl: CALLS_DDL,
    append_only: CALLS_APPEND_ONLY,
    current: STORY_CALLS_SCHEMA,
};

/// The triggers that make `table` append-only.
fn append_only_ddl(table: &str) -> String {
    format!(
        "CREATE TRIGGER IF NOT EXISTS {table}_no_update BEFORE UPDATE ON {table}
             BEGIN SELECT RAISE(ABORT, '{table} is append-only'); END;
         CREATE TRIGGER IF NOT EXISTS {table}_no_delete BEFORE DELETE ON {table}
             BEGIN SELECT RAISE(ABORT, '{table} is append-only'); END;"
    )
}

/// Opens one of a story's files read-write, refusing a file that is not a
/// database (by its header, before SQLite touches it) and one a newer build
/// wrote (by its version, before any DDL), then brings it to this build's
/// schema.
pub(super) fn open_file(path: &Path, spec: &FileSpec) -> Result<Connection, StoryDbError> {
    let exists_with_content = std::fs::metadata(path).is_ok_and(|m| m.len() > 0);
    if exists_with_content && !is_sqlite_file(path) {
        return Err(StoryDbError::NotADatabase {
            file: spec.name,
            path: path.to_owned(),
        });
    }
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", true)?;
    let found: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if found > spec.current {
        return Err(StoryDbError::NewerSchema {
            file: spec.name,
            path: path.to_owned(),
            found,
            supported: spec.current,
        });
    }
    let mut ddl = spec.ddl.to_owned();
    for table in spec.append_only {
        ddl.push_str(&append_only_ddl(table));
    }
    conn.execute_batch(&ddl)?;
    if found == 0 {
        // `PRAGMA user_version` takes no bound parameter; the value is a u32.
        conn.execute_batch(&format!("PRAGMA user_version = {};", spec.current))?;
    }
    // No breaking step exists yet: the first one adds a registry here, as
    // `data.db` has one (`shared::storage::db`).
    Ok(conn)
}

/// The key a story's database is registered under while it is open: its
/// directory, canonicalized, and its file name.
pub(super) fn registry_key(path: &Path) -> Result<PathBuf, StoryDbError> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .ok_or_else(|| StoryDbError::BadPath(path.to_owned()))?;
    Ok(std::fs::canonicalize(parent)?.join(name))
}

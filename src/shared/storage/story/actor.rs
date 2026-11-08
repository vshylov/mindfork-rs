//! The database actor: the thread that owns a story's connections, the
//! registry of the capabilities it issued, and every query
//! (docs/research/fabula-engine.md §6, §9).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, mpsc};

use chrono::Utc;
use rusqlite::{Connection, params};
use serde_json::Value;
use tokio::sync::oneshot;
use uuid::Uuid;

use super::schema::{self, CALLS_FILE, STORY_FILE};
use super::{CallStatus, CallUsage, Mechanism, NewCall, StoredCall, StoryDbError};
use crate::entities::story::{Canon, RunId};

#[path = "actor_manuscript.rs"]
mod manuscript;

/// A job for the actor's thread.
pub(super) type Job = Box<dyn FnOnce(&mut Actor) + Send>;

/// The namespace of call-log ids: a call's id is the UUIDv5 of its run,
/// address and attempt in it (spec §6), so a replay finds it by name.
const CALL_NAMESPACE: Uuid = Uuid::from_u128(0x6d69_6e64_666f_726b_6661_6275_6c61_0001);

/// Who holds which capability. A token names one issue of a capability; a
/// write carries it and must match, and a release frees only its own issue.
#[derive(Default)]
struct Writers {
    next: u64,
    author: Option<u64>,
    run_log: Option<u64>,
}

impl Writers {
    fn fresh(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
}

fn check(held: Option<&u64>, token: u64) -> Result<(), StoryDbError> {
    match held {
        Some(&t) if t == token => Ok(()),
        _ => Err(StoryDbError::WriterRevoked),
    }
}

/// A canon document as stored: the canon split by kind and key.
struct Doc {
    kind: &'static str,
    key: String,
    position: i64,
    body: String,
}

/// The latest stored version of one document.
struct Latest {
    version: i64,
    position: i64,
    body: Option<String>,
}

fn docs_of(canon: &Canon) -> Result<Vec<Doc>, serde_json::Error> {
    let single = |kind: &'static str, body: String| Doc {
        kind,
        key: kind.to_owned(),
        position: 0,
        body,
    };
    let mut docs = vec![
        single("world", serde_json::to_string(&canon.world)?),
        single("style", serde_json::to_string(&canon.style)?),
        single("arc", serde_json::to_string(&canon.arc)?),
    ];
    for (i, location) in canon.locations.iter().enumerate() {
        docs.push(Doc {
            kind: "location",
            key: location.key.to_string(),
            position: i as i64,
            body: serde_json::to_string(location)?,
        });
    }
    for (i, character) in canon.characters.iter().enumerate() {
        docs.push(Doc {
            kind: "character",
            key: character.key.to_string(),
            position: i as i64,
            body: serde_json::to_string(character)?,
        });
    }
    Ok(docs)
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

/// The actor's state: the connections, the issued capabilities, the
/// assembled canon.
pub(super) struct Actor {
    story: Connection,
    calls: Connection,
    writers: Writers,
    canon: Option<Arc<Canon>>,
    closing: Option<oneshot::Sender<()>>,
}

impl Actor {
    pub(super) fn open(story: &Path, calls: &Path) -> Result<Self, StoryDbError> {
        Ok(Self {
            story: schema::open_file(story, &STORY_FILE)?,
            calls: schema::open_file(calls, &CALLS_FILE)?,
            writers: Writers::default(),
            canon: None,
            closing: None,
        })
    }

    /// An actor over read-only connections, for a summary that must leave the
    /// files as they were ([`super::status`]): it serves reads alone, on the
    /// caller's thread, and no writer is ever issued by it. A story whose call
    /// log is gone — the log stays outside backups — reads with an empty one.
    pub(super) fn read_only(story: &Path, calls: &Path) -> Result<Self, StoryDbError> {
        let calls = if calls.exists() {
            schema::open_read_only(calls, &CALLS_FILE)?
        } else {
            schema::open_file(Path::new(":memory:"), &CALLS_FILE)?
        };
        Ok(Self {
            story: schema::open_read_only(story, &STORY_FILE)?,
            calls,
            writers: Writers::default(),
            canon: None,
            closing: None,
        })
    }

    /// Runs jobs until every sender is gone or a close arrives; the close's
    /// acknowledgement, sent by the caller once the claim on the file is
    /// given back. The connections close when `self` drops, on return.
    pub(super) fn run(mut self, rx: mpsc::Receiver<Job>) -> Option<oneshot::Sender<()>> {
        while let Ok(job) = rx.recv() {
            job(&mut self);
            if self.closing.is_some() {
                break;
            }
        }
        self.closing.take()
    }

    pub(super) fn close_with(&mut self, ack: oneshot::Sender<()>) {
        self.closing = Some(ack);
    }

    // ---------- capabilities ----------

    pub(super) fn issue_author(&mut self) -> Result<u64, StoryDbError> {
        if self.writers.author.is_some() {
            return Err(StoryDbError::WriterHeld("the Author's writer".into()));
        }
        let token = self.writers.fresh();
        self.writers.author = Some(token);
        Ok(token)
    }

    pub(super) fn release_author(&mut self, token: u64) {
        if self.writers.author == Some(token) {
            self.writers.author = None;
        }
    }

    pub(super) fn issue_run_log(&mut self) -> Result<u64, StoryDbError> {
        if self.writers.run_log.is_some() {
            return Err(StoryDbError::WriterHeld("the run log's writer".into()));
        }
        let token = self.writers.fresh();
        self.writers.run_log = Some(token);
        Ok(token)
    }

    pub(super) fn release_run_log(&mut self, token: u64) {
        if self.writers.run_log == Some(token) {
            self.writers.run_log = None;
        }
    }

    // ---------- the canon ----------

    /// The canon assembled from the latest version of every document, cached
    /// until the next import.
    pub(super) fn canon(&mut self) -> Result<Option<Arc<Canon>>, StoryDbError> {
        if self.canon.is_none() {
            self.canon = self.assemble_canon()?.map(Arc::new);
        }
        Ok(self.canon.clone())
    }

    fn latest_docs(&self) -> Result<BTreeMap<(String, String), Latest>, StoryDbError> {
        let mut stmt = self.story.prepare_cached(
            "SELECT kind, key, version, position, body_json FROM canon_doc d
             WHERE version = (SELECT MAX(version) FROM canon_doc m
                              WHERE m.kind = d.kind AND m.key = d.key)",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                (r.get::<_, String>(0)?, r.get::<_, String>(1)?),
                Latest {
                    version: r.get(2)?,
                    position: r.get(3)?,
                    body: r.get(4)?,
                },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn assemble_canon(&self) -> Result<Option<Canon>, StoryDbError> {
        let docs = self.latest_docs()?;
        let body = |kind: &str| {
            docs.get(&(kind.to_owned(), kind.to_owned()))
                .and_then(|d| d.body.as_deref())
        };
        let (Some(world), Some(arc)) = (body("world"), body("arc")) else {
            return Ok(None);
        };
        let style = match body("style") {
            Some(style) => serde_json::from_str(style)?,
            None => Default::default(),
        };
        let mut locations = Vec::new();
        let mut characters = Vec::new();
        for ((kind, _), doc) in &docs {
            let Some(body) = doc.body.as_deref() else {
                continue;
            };
            match kind.as_str() {
                "location" => locations.push((doc.position, serde_json::from_str(body)?)),
                "character" => characters.push((doc.position, serde_json::from_str(body)?)),
                _ => {}
            }
        }
        locations.sort_by_key(|(position, _)| *position);
        characters.sort_by_key(|(position, _)| *position);
        Ok(Some(Canon {
            world: serde_json::from_str(world)?,
            locations: locations.into_iter().map(|(_, l)| l).collect(),
            characters: characters.into_iter().map(|(_, c)| c).collect(),
            style,
            arc: serde_json::from_str(arc)?,
        }))
    }

    pub(super) fn import_canon(
        &mut self,
        token: u64,
        canon: &Canon,
    ) -> Result<usize, StoryDbError> {
        check(self.writers.author.as_ref(), token)?;
        let problems = canon.check();
        if !problems.is_empty() {
            return Err(StoryDbError::Canon(problems));
        }
        if self.canon_approved()? {
            return Err(StoryDbError::CanonFrozen);
        }
        let docs = docs_of(canon)?;
        let latest = self.latest_docs()?;
        let created_at = now();
        let tx = self.story.transaction()?;
        let mut written = 0;
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO canon_doc (id, kind, key, version, position, body_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            let mut put =
                |kind: &str, key: &str, version: i64, position: i64, body: Option<&str>| {
                    insert
                        .execute(params![
                            format!("{kind}/{key}/v{version}"),
                            kind,
                            key,
                            version,
                            position,
                            body,
                            created_at,
                        ])
                        .map(|_| ())
                };
            for doc in &docs {
                let previous = latest.get(&(doc.kind.to_owned(), doc.key.clone()));
                let unchanged = previous.is_some_and(|p| {
                    p.body.as_deref() == Some(doc.body.as_str()) && p.position == doc.position
                });
                if unchanged {
                    continue;
                }
                let version = previous.map_or(1, |p| p.version + 1);
                put(doc.kind, &doc.key, version, doc.position, Some(&doc.body))?;
                written += 1;
            }
            for ((kind, key), previous) in &latest {
                let gone = !docs.iter().any(|d| d.kind == kind && &d.key == key);
                if gone && previous.body.is_some() {
                    put(kind, key, previous.version + 1, previous.position, None)?;
                    written += 1;
                }
            }
        }
        tx.commit()?;
        self.canon = None;
        Ok(written)
    }

    // ---------- the run log ----------

    fn require_run(&self, run: &RunId) -> Result<(), StoryDbError> {
        let exists: bool = self.story.query_row(
            "SELECT EXISTS (SELECT 1 FROM run WHERE id = ?1)",
            [run.as_str()],
            |r| r.get(0),
        )?;
        if exists {
            Ok(())
        } else {
            Err(StoryDbError::UnknownRun(run.clone()))
        }
    }

    pub(super) fn start_run(&mut self, token: u64, config: &Value) -> Result<RunId, StoryDbError> {
        check(self.writers.run_log.as_ref(), token)?;
        let id = RunId::nth(self.last_run()? + 1);
        self.story.execute(
            "INSERT INTO run (id, config_json, started_at) VALUES (?1, ?2, ?3)",
            params![id.as_str(), serde_json::to_string(config)?, now()],
        )?;
        Ok(id)
    }

    /// The number of the last run of **either** file. A backup carries the
    /// story and leaves its call log out (spec §15), so a restore can put
    /// back a story behind its log; a run id the log already holds would
    /// collide with that run's calls (docs/history/fabula.md §4, PR 7).
    pub(super) fn last_run(&self) -> Result<u64, StoryDbError> {
        let last = |conn: &rusqlite::Connection, sql: &str| -> Result<u64, StoryDbError> {
            Ok(conn
                .query_row(sql, [], |r| r.get::<_, Option<i64>>(0))?
                .unwrap_or(0) as u64)
        };
        let in_story = last(
            &self.story,
            "SELECT MAX(CAST(substr(id, 4) AS INTEGER)) FROM run WHERE id GLOB 'run[1-9]*'",
        )?;
        let in_log = last(
            &self.calls,
            "SELECT MAX(CAST(substr(run_id, 4) AS INTEGER)) FROM llm_call
             WHERE run_id GLOB 'run[1-9]*'",
        )?;
        Ok(in_story.max(in_log))
    }

    pub(super) fn record_call(
        &mut self,
        token: u64,
        call: &NewCall,
    ) -> Result<String, StoryDbError> {
        check(self.writers.run_log.as_ref(), token)?;
        self.require_run(&call.run)?;
        let id = call_id(&call.run, &call.address, call.attempt);
        let exists: bool = self.calls.query_row(
            "SELECT EXISTS (SELECT 1 FROM llm_call WHERE id = ?1)",
            [id.as_str()],
            |r| r.get(0),
        )?;
        if exists {
            return Err(StoryDbError::CallExists {
                run: call.run.clone(),
                address: call.address.clone(),
                attempt: call.attempt,
            });
        }
        let response = call
            .response
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        self.calls.execute(
            "INSERT INTO llm_call (id, run_id, address, attempt, role, model, params_json,
                 mechanism, request_json, prompt_sha256, response_json, status, input_tokens,
                 output_tokens, cached_tokens, cost_usd, latency_ms, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                id,
                call.run.as_str(),
                call.address,
                call.attempt,
                call.role,
                call.model,
                serde_json::to_string(&call.params)?,
                call.mechanism.as_str(),
                serde_json::to_string(&call.request)?,
                call.prompt_sha256,
                response,
                call.status.as_str(),
                call.usage.input_tokens.map(|n| n as i64),
                call.usage.output_tokens.map(|n| n as i64),
                call.usage.cached_tokens.map(|n| n as i64),
                call.usage.cost_usd,
                call.latency_ms.map(|n| n as i64),
                now(),
            ],
        )?;
        Ok(id)
    }

    #[cfg(test)]
    pub(super) fn call(
        &self,
        run: &RunId,
        address: &str,
        attempt: u32,
    ) -> Result<Option<StoredCall>, StoryDbError> {
        use rusqlite::OptionalExtension;
        let row = self
            .calls
            .query_row(
                &format!(
                    "SELECT {CALL_COLUMNS} FROM llm_call
                     WHERE run_id = ?1 AND address = ?2 AND attempt = ?3"
                ),
                params![run.as_str(), address, attempt],
                CallRow::read,
            )
            .optional()?;
        row.map(|row| row.into_call(run)).transpose()
    }

    pub(super) fn calls(&self, run: &RunId) -> Result<Vec<StoredCall>, StoryDbError> {
        let mut select = self.calls.prepare(&format!(
            "SELECT {CALL_COLUMNS} FROM llm_call WHERE run_id = ?1 ORDER BY rowid"
        ))?;
        let rows = select
            .query_map([run.as_str()], CallRow::read)?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter().map(|row| row.into_call(run)).collect()
    }

    #[cfg(test)]
    pub(super) fn execute_raw(&self, sql: &str, calls: bool) -> Result<usize, StoryDbError> {
        let conn = if calls { &self.calls } else { &self.story };
        Ok(conn.execute(sql, [])?)
    }
}

/// A call's id: the UUIDv5 of its run, address and attempt.
fn call_id(run: &RunId, address: &str, attempt: u32) -> String {
    let name = format!("{run}\n{address}\n{attempt}");
    Uuid::new_v5(&CALL_NAMESPACE, name.as_bytes()).to_string()
}

/// The columns [`CallRow::read`] reads, in its order.
const CALL_COLUMNS: &str = "id, role, model, params_json, mechanism, request_json, prompt_sha256,
    response_json, status, input_tokens, output_tokens, cached_tokens, cost_usd, latency_ms,
    created_at, address, attempt";

/// A call-log row as selected.
struct CallRow {
    id: String,
    role: String,
    model: String,
    params: String,
    mechanism: String,
    request: String,
    prompt_sha256: String,
    response: Option<String>,
    status: String,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cached_tokens: Option<i64>,
    cost_usd: Option<f64>,
    latency_ms: Option<i64>,
    created_at: String,
    address: String,
    attempt: u32,
}

impl CallRow {
    fn read(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(CallRow {
            id: r.get(0)?,
            role: r.get(1)?,
            model: r.get(2)?,
            params: r.get(3)?,
            mechanism: r.get(4)?,
            request: r.get(5)?,
            prompt_sha256: r.get(6)?,
            response: r.get(7)?,
            status: r.get(8)?,
            input_tokens: r.get(9)?,
            output_tokens: r.get(10)?,
            cached_tokens: r.get(11)?,
            cost_usd: r.get(12)?,
            latency_ms: r.get(13)?,
            created_at: r.get(14)?,
            address: r.get(15)?,
            attempt: r.get(16)?,
        })
    }

    fn into_call(self, run: &RunId) -> Result<StoredCall, StoryDbError> {
        let corrupt = |what: &str, value: &str| {
            StoryDbError::Corrupt(format!("call {} has {what} {value:?}", self.id))
        };
        let mechanism = Mechanism::parse(&self.mechanism)
            .ok_or_else(|| corrupt("mechanism", &self.mechanism))?;
        let status =
            CallStatus::parse(&self.status).ok_or_else(|| corrupt("status", &self.status))?;
        let count = |n: Option<i64>| n.map(|n| n as u64);
        Ok(StoredCall {
            call: NewCall {
                run: run.clone(),
                address: self.address,
                attempt: self.attempt,
                role: self.role,
                model: self.model,
                params: serde_json::from_str(&self.params)?,
                mechanism,
                request: serde_json::from_str(&self.request)?,
                prompt_sha256: self.prompt_sha256,
                response: self
                    .response
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()?,
                status,
                usage: CallUsage {
                    input_tokens: count(self.input_tokens),
                    output_tokens: count(self.output_tokens),
                    cached_tokens: count(self.cached_tokens),
                    cost_usd: self.cost_usd,
                },
                latency_ms: count(self.latency_ms),
            },
            id: self.id,
            created_at: self.created_at,
        })
    }
}

//! M3's hidden runner (docs/fabula.md §4, PR 13; docs/research/fabula-engine.md
//! §11, §13, §15): one command, `mindfork story-eval`, which `--help` does not
//! list.
//!
//! - **Its own directory.** A story is written under the run's output
//!   directory — its database, its call log, the markdown and the trace —
//!   and never into the data, so the single-instance lock is not taken and a
//!   running app is not disturbed.
//! - **B0 alone** (docs/fabula.md §6, T28): one writer in one session
//!   ([`super::single`]), on a model the command's own options name, to the
//!   spend `--target-usd` or `--target-tokens` names, else to its revisions as
//!   the story tools' (T25). F and the ablations A1–A4 were removed after M3;
//!   their names stay, so that M3's stories on disk can still be judged.
//! - **The premises** are frozen in the repository with their SHA-256
//!   (`premises/SHA256SUMS`), and every trace names its premise's hash.
//! - **Resumable.** A story resumes from its stores (spec §9), and a batch
//!   run again skips each story whose trace says it ended, so the batch can
//!   run for days as a service that is restarted (docs/fabula.md §4, *Where
//!   it runs*).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::author_prompts;
use super::engine::{EngineAdapter, ListPrice, LongPrompt, SharedLog};
use super::export;
use super::judge::{self, Judge, Judging, Matchup};
use super::run::writer_sampling;
use super::single::{self, Ended, SingleConfig, Target};
use super::single_story::{Budget, MOST_REVISIONS};
use crate::entities::sampling::ReasoningEffort;
use crate::entities::story::Canon;
use crate::shared::api::contract::EngineBackend;
use crate::shared::api::retry::RetryBackend;
use crate::shared::api::{AnthropicClient, GeminiClient, OpenAiClient, ResponsesClient};
use crate::shared::config::ToolSettings;
use crate::shared::storage::story::{Spend, StoryDb, StoryDbError};

/// A system of spec §11's evaluation: the names M3's stories are filed
/// under. Only B0 is written now (docs/fabula.md §6, T28).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum System {
    /// Fabula's simulation, removed after M3.
    F,
    /// One writer, one session.
    B0,
    /// The Author and the reviewers, no simulation; removed after M3.
    A1,
    /// One branch a scene; removed.
    A2,
    /// No reviewers; removed.
    A3,
    /// Every character sees every event; removed.
    A4,
}

impl System {
    /// Every system, F first.
    pub const ALL: [System; 6] = [
        System::F,
        System::B0,
        System::A1,
        System::A2,
        System::A3,
        System::A4,
    ];

    /// Its name, as spec §11 spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            System::F => "F",
            System::B0 => "B0",
            System::A1 => "A1",
            System::A2 => "A2",
            System::A3 => "A3",
            System::A4 => "A4",
        }
    }

    /// The system a name spells, in any case.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|s| s.as_str().eq_ignore_ascii_case(name.trim()))
    }
}

// ---------- the premises ----------

/// A story's premise: its canon, and the hash of the file it was read from.
#[derive(Debug, Clone, PartialEq)]
pub struct Premise {
    /// Its id: a frozen premise's name, or a file's stem.
    pub id: String,
    /// The canon.
    pub canon: Canon,
    /// The SHA-256 of the file's bytes.
    pub sha256: String,
    /// Whether it is one of M3's frozen premises.
    pub frozen: bool,
}

/// M3's premises, frozen before any run (spec §11, §15): each file as the
/// repository holds it. Three in each of four genres: a mystery, a clash of
/// goals, a villain, a quiet story of a relationship; chosen by the owner
/// from twenty candidates on 2026-11-05.
const FROZEN: &[(&str, &str)] = &[
    ("mystery-2", include_str!("premises/mystery-2.json")),
    ("mystery-4", include_str!("premises/mystery-4.json")),
    ("mystery-5", include_str!("premises/mystery-5.json")),
    ("clash-3", include_str!("premises/clash-3.json")),
    ("clash-4", include_str!("premises/clash-4.json")),
    ("clash-5", include_str!("premises/clash-5.json")),
    ("villain-1", include_str!("premises/villain-1.json")),
    ("villain-3", include_str!("premises/villain-3.json")),
    ("villain-4", include_str!("premises/villain-4.json")),
    ("quiet-2", include_str!("premises/quiet-2.json")),
    ("quiet-3", include_str!("premises/quiet-3.json")),
    ("quiet-5", include_str!("premises/quiet-5.json")),
];

/// Their hashes, `sha256sum`'s format: what the frozen files must hash to.
const SUMS: &str = include_str!("premises/SHA256SUMS");

/// The SHA-256 of `bytes`, in hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The canon a premise's text holds, checked.
fn canon_of(id: &str, text: &str) -> Result<Canon, String> {
    let canon: Canon =
        serde_json::from_str(text).map_err(|e| format!("premise {id} is not a canon: {e}"))?;
    let problems = canon.check();
    if !problems.is_empty() {
        let listed: Vec<String> = problems.iter().map(ToString::to_string).collect();
        return Err(format!("premise {id}: {}", listed.join("; ")));
    }
    Ok(canon)
}

/// The hash `sums` records for `id`'s file.
fn recorded(sums: &str, id: &str) -> Option<String> {
    let file = format!("{id}.json");
    sums.lines().find_map(|line| {
        let (hash, name) = line.split_once(char::is_whitespace)?;
        (name.trim().trim_start_matches('*') == file).then(|| hash.trim().to_owned())
    })
}

/// The frozen premises, each held to its recorded hash.
pub fn frozen() -> Result<Vec<Premise>, String> {
    frozen_of(FROZEN, SUMS)
}

fn frozen_of(files: &[(&str, &str)], sums: &str) -> Result<Vec<Premise>, String> {
    files
        .iter()
        .map(|(id, text)| {
            let sha256 = sha256_hex(text.as_bytes());
            match recorded(sums, id) {
                Some(hash) if hash == sha256 => {}
                Some(hash) => {
                    return Err(format!(
                        "premise {id} hashes to {sha256}, but SHA256SUMS records {hash}: \
                         a frozen premise was changed"
                    ));
                }
                None => return Err(format!("premise {id} has no hash in SHA256SUMS")),
            }
            Ok(Premise {
                id: (*id).to_owned(),
                canon: canon_of(id, text)?,
                sha256,
                frozen: true,
            })
        })
        .collect()
}

/// The premise `name` names: a frozen one by its id, or a canon file.
pub fn premise(name: &str) -> Result<Premise, String> {
    if let Some(found) = frozen()?.into_iter().find(|p| p.id == name) {
        return Ok(found);
    }
    let path = Path::new(name);
    let bytes = std::fs::read(path)
        .map_err(|e| format!("{name} is neither a frozen premise nor a readable file: {e}"))?;
    let text = String::from_utf8(bytes.clone()).map_err(|_| format!("{name} is not UTF-8"))?;
    let id = path
        .file_stem()
        .map_or_else(|| name.to_owned(), |s| s.to_string_lossy().into_owned());
    Ok(Premise {
        canon: canon_of(&id, &text)?,
        id,
        sha256: sha256_hex(&bytes),
        frozen: false,
    })
}

// ---------- the engine ----------

/// Which wire the model is reached on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    /// OpenRouter's gateway, its dialect, a provider optionally pinned.
    OpenRouter,
    /// Any OpenAI-compatible server at `--url`: llama.cpp, vLLM, a proxy.
    Compatible,
    /// Anthropic's Messages API.
    Anthropic,
    /// OpenAI's Responses API.
    OpenAi,
    /// Gemini's native API.
    Gemini,
}

impl Wire {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "openrouter" => Wire::OpenRouter,
            "compatible" => Wire::Compatible,
            "anthropic" => Wire::Anthropic,
            "openai" => Wire::OpenAi,
            "gemini" => Wire::Gemini,
            _ => return None,
        })
    }

    /// The variable its key is read from by default.
    fn key_env(self) -> &'static str {
        match self {
            Wire::OpenRouter => "OPENROUTER_API_KEY",
            Wire::Compatible => "MINDFORK_ENGINE_KEY",
            Wire::Anthropic => "ANTHROPIC_API_KEY",
            Wire::OpenAi => "OPENAI_API_KEY",
            Wire::Gemini => "GEMINI_API_KEY",
        }
    }
}

/// The model every system of a run is written by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// The wire.
    pub wire: Wire,
    /// The model's name on it.
    pub model: String,
    /// The server, for [`Wire::Compatible`]; the wire's own otherwise.
    pub url: Option<String>,
    /// The variable the key is read from; never the key itself.
    pub key_env: String,
    /// OpenRouter's providers the requests are pinned to.
    pub pin: Vec<String>,
}

impl Route {
    /// The backend, with the app's retries; an unset key is refused, by the
    /// variable's name.
    pub fn backend(&self) -> Result<Arc<dyn EngineBackend>, String> {
        let key = std::env::var(&self.key_env).ok().filter(|k| !k.is_empty());
        let need = || format!("set {} to the key", self.key_env);
        let model = self.model.clone();
        let client: Arc<dyn EngineBackend> = match self.wire {
            Wire::OpenRouter => Arc::new(
                OpenAiClient::new(
                    self.url
                        .clone()
                        .unwrap_or_else(|| "https://openrouter.ai/api/v1".to_owned()),
                )
                .with_api_key(Some(key.ok_or_else(need)?))
                .with_model(Some(model))
                .for_openrouter(false)
                .pinned_to(self.pin.clone()),
            ),
            Wire::Compatible => Arc::new(
                OpenAiClient::new(self.url.clone().ok_or("--engine compatible needs --url")?)
                    .with_api_key(key)
                    .with_model(Some(model)),
            ),
            Wire::Anthropic => Arc::new(AnthropicClient::new(
                "https://api.anthropic.com",
                key.ok_or_else(need)?,
                model,
            )),
            Wire::OpenAi => Arc::new(ResponsesClient::new(
                "https://api.openai.com/v1",
                key.ok_or_else(need)?,
                model,
            )),
            Wire::Gemini => Arc::new(GeminiClient::new(
                "https://generativelanguage.googleapis.com/v1beta",
                key.ok_or_else(need)?,
                model,
            )),
        };
        Ok(RetryBackend::wrap(client))
    }

    /// The list price its calls are counted at where the provider names
    /// none: Anthropic reports no price, and B0's target is F's dollars
    /// (T21), so a Claude model M3 may run on is priced here. Any other model
    /// on that wire is matched in tokens, as before.
    pub fn list_price(&self) -> Option<ListPrice> {
        if self.wire != Wire::Anthropic {
            return None;
        }
        match self.model.as_str() {
            // $10 and $50 a million, the whole window alike: a judge of
            // M3's panel (PR 14).
            "claude-fable-5-1" => Some(ListPrice {
                input: 10.0,
                output: 50.0,
                long: None,
            }),
            // $2 and $10 a million, the whole window alike.
            "claude-sonnet-5-5" => Some(ListPrice {
                input: 2.0,
                output: 10.0,
                long: None,
            }),
            // $0.10 and $0.50 to a prompt of 100,000 tokens, $0.50 and $2.50
            // past it.
            "claude-haiku-5-5" => Some(ListPrice {
                input: 0.10,
                output: 0.50,
                long: Some(LongPrompt {
                    above: 100_000,
                    input: 0.50,
                    output: 2.50,
                }),
            }),
            _ => None,
        }
    }

    fn to_json(&self) -> Value {
        let price = self.list_price().map(|p| {
            json!({
                "input": p.input,
                "output": p.output,
                "long": p.long.map(|l| json!({"above": l.above, "input": l.input, "output": l.output})),
            })
        });
        json!({
            "wire": format!("{:?}", self.wire).to_lowercase(),
            "model": self.model,
            "url": self.url,
            "pin": self.pin,
            "list_price": price,
        })
    }
}

// ---------- one story ----------

/// What every story of a run shares.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// The language the stories are written in.
    pub language: String,
    /// The story's length, in words, as asked.
    pub words: (u32, u32),
    /// The story's budget: its ceiling, as the story tools' chapter's.
    pub budget: Budget,
    /// A story's time limit; a story cut by it resumes on the next run.
    pub timeout: Duration,
    /// The model's window.
    pub window: u64,
}

impl Default for Options {
    /// The app's own: the chapter's length and budget, its six hours (T2,
    /// T11), and the stories in Russian (docs/fabula.md §6, T20).
    fn default() -> Self {
        let tools = ToolSettings::default();
        Self {
            language: "Russian".to_owned(),
            words: SingleConfig::default().words,
            budget: Budget {
                tokens: Some(tools.story_budget_tokens),
                usd: None,
            },
            timeout: Duration::from_secs(tools.story_run_timeout_secs),
            window: SingleConfig::default().window,
        }
    }
}

/// One story to write.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    /// Its premise.
    pub premise: Premise,
    /// Its system.
    pub system: System,
    /// Its seed, from 1.
    pub seed: u64,
    /// Where it is written.
    pub dir: PathBuf,
    /// The spend to reach, when the command names one; none ends the
    /// session on its revisions.
    pub target: Option<Target>,
}

/// How a story ended, as its trace records it.
#[derive(Debug, Clone, PartialEq)]
pub enum Ending {
    /// B0's session written to its end.
    Finished(String),
    /// Stopped at a step: a call cancelled or that gave nothing.
    Stopped {
        /// The step.
        step: String,
        /// Why, in words.
        why: String,
        /// Whether running it again may get further: a call that failed.
        transient: bool,
    },
    /// Its time limit cut it at the step in flight.
    TimedOut(String),
}

impl Ending {
    /// Whether a batch run again skips it.
    pub fn is_final(&self) -> bool {
        match self {
            Ending::Finished(_) => true,
            Ending::Stopped { transient, .. } => !transient,
            Ending::TimedOut(_) => false,
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Ending::Finished(how) => json!({"kind": "finished", "how": how}),
            Ending::Stopped {
                step,
                why,
                transient,
            } => json!({"kind": "stopped", "step": step, "why": why, "transient": transient}),
            Ending::TimedOut(step) => json!({"kind": "timed_out", "step": step}),
        }
    }
}

/// Writes `job`'s story on `backend` (the model `route` names), from where
/// its stores leave it, and its markdown and trace beside it. The trace is
/// returned too.
pub async fn run_story(
    job: &Job,
    route: &Route,
    backend: Arc<dyn EngineBackend>,
    options: &Options,
) -> Result<Value, StoryDbError> {
    tokio::fs::create_dir_all(&job.dir).await?;
    let db = StoryDb::open(&job.dir.join("story.db"), &job.dir.join("calls.db")).await?;
    let started = Instant::now();
    let written = write_open(job, route, &db, backend, options).await;
    let trace = match written {
        Ok((ending, config)) => {
            let trace = trace(job, route, &db, &ending, config, started.elapsed()).await?;
            tokio::fs::write(job.dir.join("story.md"), export::markdown(&db).await?).await?;
            let text = serde_json::to_string_pretty(&trace).unwrap_or_default();
            tokio::fs::write(job.dir.join("trace.json"), text).await?;
            Ok(trace)
        }
        Err(e) => Err(e),
    };
    db.close().await?;
    trace
}

/// The story written on the open database: how it ended, and the
/// configuration it ran under.
async fn write_open(
    job: &Job,
    route: &Route,
    db: &StoryDb,
    backend: Arc<dyn EngineBackend>,
    options: &Options,
) -> Result<(Ending, Value), StoryDbError> {
    let mut author = db.author_writer().await?;
    if db.canon().await?.is_none() {
        let mut canon = job.premise.canon.clone();
        canon.style.language = options.language.clone();
        author.import_canon(&canon).await?;
    }
    let log: SharedLog = Arc::new(Mutex::new(db.run_log_writer().await?));
    let calls = CancellationToken::new();
    let mut adapter = EngineAdapter::sharing(
        backend,
        log.clone(),
        route.model.clone(),
        writer_sampling(),
        calls.clone(),
    );
    if let Some(price) = route.list_price() {
        adapter = adapter.with_list_price(price);
    }
    let deadline = tokio::time::sleep(options.timeout);
    tokio::pin!(deadline);
    let mut cut = false;
    let config = SingleConfig {
        words: options.words,
        target: job.target,
        budget: options.budget,
        window: options.window,
        // M3's backstop under a target; the story tools' end without one.
        most_revisions: job
            .target
            .map_or(MOST_REVISIONS, |_| SingleConfig::default().most_revisions),
    };
    let work = async {
        let written = single::write(db, &mut author, &log, &adapter, &config).await?;
        let ending = match written.ended {
            Ended::Spent => Ending::Finished("spent".to_owned()),
            Ended::Window => Ending::Finished("window".to_owned()),
            Ended::Revisions => Ending::Finished("revisions".to_owned()),
            Ended::Budget => Ending::Finished("budget".to_owned()),
            Ended::Cancelled(step) => Ending::Stopped {
                step,
                why: "cancelled".to_owned(),
                transient: true,
            },
            Ended::Failed(step, why) => Ending::Stopped {
                step,
                why,
                transient: true,
            },
        };
        Ok::<_, StoryDbError>((ending, single_json(&config)))
    };
    tokio::pin!(work);
    let (ending, config) = loop {
        tokio::select! {
            done = &mut work => break done?,
            () = &mut deadline, if !cut => {
                cut = true;
                calls.cancel();
            }
        }
    };
    let ending = match ending {
        Ending::Stopped { step, .. } if cut => Ending::TimedOut(step),
        other => other,
    };
    Ok((ending, config))
}

fn single_json(config: &SingleConfig) -> Value {
    json!({
        "words": [config.words.0, config.words.1],
        "target": config.target.map(|t| json!({"usd": t.usd, "tokens": t.tokens})),
        "budget": {"tokens": config.budget.tokens, "usd": config.budget.usd},
        "window": config.window,
        "most_revisions": config.most_revisions,
    })
}

// ---------- the trace ----------

fn spend_json(spent: &Spend) -> Value {
    json!({
        "calls": spent.calls,
        "input_tokens": spent.input_tokens,
        "output_tokens": spent.output_tokens,
        "cost_usd": spent.cost_usd,
    })
}

/// What a story's stores hold once it ended: the trace the judging reads
/// beside the markdown (spec §11's metrics): its spend, its length, its
/// versions.
async fn trace(
    job: &Job,
    route: &Route,
    db: &StoryDb,
    ending: &Ending,
    config: Value,
    took: Duration,
) -> Result<Value, StoryDbError> {
    let mut calls = Vec::new();
    for run in db.runs().await? {
        calls.extend(db.calls(&run).await?);
    }
    let mut roles: BTreeMap<String, Spend> = BTreeMap::new();
    for c in &calls {
        let spent = roles.entry(c.call.role.clone()).or_default();
        spent.calls += 1;
        spent.input_tokens += c.call.usage.input_tokens.unwrap_or(0);
        spent.output_tokens += c.call.usage.output_tokens.unwrap_or(0);
        if let Some(cost) = c.call.usage.cost_usd {
            spent.cost_usd = Some(spent.cost_usd.unwrap_or(0.0) + cost);
        }
    }
    let canon = db
        .canon()
        .await?
        .unwrap_or_else(|| job.premise.canon.clone());
    let versions = db.chapter_versions(1).await?;
    let story = export::markdown(db).await?;
    Ok(json!({
        "premise": job.premise.id,
        "sha256": job.premise.sha256,
        "frozen": job.premise.frozen,
        "system": job.system.as_str(),
        "seed": job.seed,
        "language": canon.style.language,
        "route": route.to_json(),
        "config": config,
        "ending": ending.to_json(),
        "words": author_prompts::words_of(&story),
        "versions": versions.len(),
        "spent": spend_json(&db.spend("").await?),
        "roles": roles.iter().map(|(r, s)| (r.clone(), spend_json(s))).collect::<serde_json::Map<_, _>>(),
        "seconds": took.as_secs(),
        "runner": env!("CARGO_PKG_VERSION"),
    }))
}

// ---------- the batch ----------

/// A batch: B0's story of every premise × seed asked for, in one output
/// directory, `parallel` stories at a time.
#[derive(Debug, Clone, PartialEq)]
pub struct Batch {
    /// The premises.
    pub premises: Vec<Premise>,
    /// The seeds.
    pub seeds: Vec<u64>,
    /// The output directory.
    pub out: PathBuf,
    /// How many stories run at once.
    pub parallel: usize,
    /// The spend each story reaches, when the command names one.
    pub target: Option<Target>,
}

/// Where a story of a batch is written.
pub fn story_dir(out: &Path, premise: &str, system: System, seed: u64) -> PathBuf {
    out.join(premise)
        .join(system.as_str())
        .join(format!("s{seed}"))
}

/// A trace already written for `dir`, if any.
async fn written_trace(dir: &Path) -> Option<Value> {
    let text = tokio::fs::read_to_string(dir.join("trace.json"))
        .await
        .ok()?;
    serde_json::from_str(&text).ok()
}

/// Whether a written trace says its story ended for good.
fn ended(trace: &Value) -> bool {
    match trace["ending"]["kind"].as_str() {
        Some("finished") => true,
        Some("stopped") => !trace["ending"]["transient"].as_bool().unwrap_or(false),
        _ => false,
    }
}

/// The jobs of `batch`: B0's story of each premise and seed, but the ones
/// already ended.
async fn jobs(batch: &Batch) -> Vec<Job> {
    let mut jobs = Vec::new();
    for premise in &batch.premises {
        for seed in &batch.seeds {
            let dir = story_dir(&batch.out, &premise.id, System::B0, *seed);
            if written_trace(&dir).await.is_some_and(|t| ended(&t)) {
                continue;
            }
            jobs.push(Job {
                premise: premise.clone(),
                system: System::B0,
                seed: *seed,
                dir,
                target: batch.target,
            });
        }
    }
    jobs
}

/// How many times a story whose call failed is run again within a batch.
const TRIES: usize = 3;

/// Runs `batch` to its end on the model `route` names: its stories
/// `parallel` at a time, each one resumed where it stands, a story whose call
/// failed tried again. One line a story on stdout and in `batch.jsonl`.
pub async fn batch(batch: &Batch, route: &Route, options: &Options) -> Result<(), String> {
    tokio::fs::create_dir_all(&batch.out)
        .await
        .map_err(|e| format!("{}: {e}", batch.out.display()))?;
    let backend = route.backend()?;
    {
        let jobs = jobs(batch).await;
        println!("{} stories", jobs.len());
        futures_util::stream::iter(jobs)
            .for_each_concurrent(batch.parallel.max(1), |job| {
                let backend = backend.clone();
                async move {
                    for attempt in 1..=TRIES {
                        let result = run_story(&job, route, backend.clone(), options).await;
                        let line = log_line(&job, attempt, &result);
                        println!("{line}");
                        append(&batch.out.join("batch.jsonl"), &line).await;
                        match result {
                            Ok(trace) if !ended(&trace) && attempt < TRIES => continue,
                            _ => break,
                        }
                    }
                }
            })
            .await;
    }
    Ok(())
}

fn log_line(job: &Job, attempt: usize, result: &Result<Value, StoryDbError>) -> String {
    let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let body = match result {
        Ok(trace) => json!({
            "at": at,
            "story": format!("{}/{}/s{}", job.premise.id, job.system.as_str(), job.seed),
            "attempt": attempt,
            "ending": trace["ending"],
            "words": trace["words"],
            "spent": trace["spent"],
            "seconds": trace["seconds"],
        }),
        Err(e) => json!({
            "at": at,
            "story": format!("{}/{}/s{}", job.premise.id, job.system.as_str(), job.seed),
            "attempt": attempt,
            "error": e.to_string(),
        }),
    };
    body.to_string()
}

async fn append(path: &Path, line: &str) {
    use tokio::io::AsyncWriteExt;
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await;
    if let Ok(mut file) = file {
        let _ = file.write_all(format!("{line}\n").as_bytes()).await;
    }
}

// ---------- the command ----------

/// What `mindfork story-eval` was asked.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// List the frozen premises and check their hashes.
    Premises,
    /// Write one story.
    Run {
        /// The story.
        job: Box<Job>,
        /// The model.
        route: Route,
        /// The run's options.
        options: Options,
    },
    /// Write a batch.
    Batch {
        /// The batch.
        batch: Batch,
        /// The model.
        route: Route,
        /// The run's options.
        options: Options,
    },
    /// Read a run's pairs as one judge of the panel.
    Judge {
        /// What is read.
        judging: Judging,
        /// The judge.
        judge: Judge,
    },
    /// Count the panel's readings.
    Verdict {
        /// The judges' output directory.
        out: PathBuf,
    },
}

/// The command's usage, printed on a mistake: it has no `--help` line.
pub const USAGE: &str = "\
usage: mindfork story-eval premises
       mindfork story-eval run --premise ID|FILE --out DIR [--system B0] [--seed N] \
[--target-usd X] [--target-tokens N] ENGINE [OPTIONS]
       mindfork story-eval batch --out DIR [--premises ID,..] [--seeds 1,2,3] [--parallel N] \
[--target-usd X] [--target-tokens N] ENGINE [OPTIONS]
       mindfork story-eval judge --stories DIR --out DIR --name NAME [--pairs F-B0,F-A1,control] \
[--premises ID,..] [--seeds 1,2,3] [--parallel N] [--effort max] [--max-tokens N] ENGINE
       mindfork story-eval verdict --out DIR
ENGINE:  --engine openrouter|compatible|anthropic|openai|gemini --model NAME \
[--url URL] [--key-env VAR] [--provider NAME,..]
OPTIONS: [--language NAME] [--budget-tokens N] [--budget-usd X] [--timeout-hours H] \
[--window N]";

/// Parses the arguments after `story-eval`.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let Some((verb, rest)) = args.split_first() else {
        return Err(USAGE.to_owned());
    };
    let mut flags: BTreeMap<String, String> = BTreeMap::new();
    let mut it = rest.iter();
    while let Some(flag) = it.next() {
        let Some(name) = flag.strip_prefix("--") else {
            return Err(format!("unexpected argument {flag}\n{USAGE}"));
        };
        let value = it
            .next()
            .ok_or_else(|| format!("--{name} needs a value\n{USAGE}"))?;
        flags.insert(name.to_owned(), value.clone());
    }
    let mut take = |name: &str| flags.remove(name);
    let command = match verb.as_str() {
        "premises" => Command::Premises,
        "run" => {
            let premise = premise(&take("premise").ok_or("--premise is required")?)?;
            let system = take("system").unwrap_or_else(|| "B0".to_owned());
            let system =
                System::parse(&system).ok_or_else(|| format!("no system is called {system}"))?;
            if system != System::B0 {
                return Err(REMOVED.into());
            }
            let seed = number(take("seed").as_deref().unwrap_or("1"), "--seed")?;
            let dir = PathBuf::from(take("out").ok_or("--out is required")?);
            let target = target(&mut take)?;
            let route = route(&mut take)?;
            let options = options(&mut take)?;
            Command::Run {
                job: Box::new(Job {
                    premise,
                    system,
                    seed,
                    dir,
                    target,
                }),
                route,
                options,
            }
        }
        "batch" => {
            let out = PathBuf::from(take("out").ok_or("--out is required")?);
            let premises = match take("premises") {
                Some(list) => list
                    .split(',')
                    .map(|p| premise(p.trim()))
                    .collect::<Result<Vec<_>, _>>()?,
                None => frozen()?,
            };
            if premises.is_empty() {
                return Err(
                    "no premise to run: none is frozen yet, and --premises names none".into(),
                );
            }
            if take("systems").is_some() {
                return Err(REMOVED.into());
            }
            let seeds = seeds(take("seeds"))?;
            let parallel = number(take("parallel").as_deref().unwrap_or("8"), "--parallel")?;
            let target = target(&mut take)?;
            let route = route(&mut take)?;
            let options = options(&mut take)?;
            Command::Batch {
                batch: Batch {
                    premises,
                    seeds,
                    out,
                    parallel: parallel as usize,
                    target,
                },
                route,
                options,
            }
        }
        "judge" => {
            let stories = PathBuf::from(take("stories").ok_or("--stories is required")?);
            let out = PathBuf::from(take("out").ok_or("--out is required")?);
            let name = take("name").ok_or("--name is required: the judge's directory")?;
            let matchups = match take("pairs") {
                Some(list) => list
                    .split(',')
                    .map(|m| Matchup::parse(m).ok_or_else(|| format!("no pair is called {m}")))
                    .collect::<Result<Vec<_>, _>>()?,
                None => Matchup::DECISIONS.to_vec(),
            };
            let premises = match take("premises") {
                Some(list) => list.split(',').map(|p| p.trim().to_owned()).collect(),
                None => frozen()?.into_iter().map(|p| p.id).collect(),
            };
            let seeds = seeds(take("seeds"))?;
            let parallel = number(take("parallel").as_deref().unwrap_or("4"), "--parallel")?;
            let effort = take("effort").unwrap_or_else(|| "max".to_owned());
            let effort: ReasoningEffort = serde_json::from_value(json!(effort.trim()))
                .map_err(|_| format!("--effort takes low|medium|high|xhigh|max, not {effort}"))?;
            let max_tokens = number(
                take("max-tokens").as_deref().unwrap_or("64000"),
                "--max-tokens",
            )?;
            let route = route(&mut take)?;
            Command::Judge {
                judging: Judging {
                    stories,
                    out,
                    premises,
                    seeds,
                    matchups,
                    parallel: parallel as usize,
                },
                judge: Judge {
                    name,
                    route,
                    effort,
                    max_tokens: max_tokens as usize,
                },
            }
        }
        "verdict" => Command::Verdict {
            out: PathBuf::from(take("out").ok_or("--out is required")?),
        },
        other => return Err(format!("no story-eval command is called {other}\n{USAGE}")),
    };
    if let Some(name) = flags.keys().next() {
        return Err(format!(
            "--{name} is not an option of story-eval {verb}\n{USAGE}"
        ));
    }
    Ok(command)
}

/// Why a system other than B0 is not run.
const REMOVED: &str = "only B0 is written: F and its ablations A1-A4 were removed after M3 \
(docs/fabula.md §6, T28)";

/// The spend `--target-usd` and `--target-tokens` name; none without them.
fn target(take: &mut impl FnMut(&str) -> Option<String>) -> Result<Option<Target>, String> {
    Ok(match (take("target-usd"), take("target-tokens")) {
        (None, None) => None,
        (usd, tokens) => Some(Target {
            usd: usd.map(|u| real(&u, "--target-usd")).transpose()?,
            tokens: tokens.map_or(Ok(u64::MAX), |t| number(&t, "--target-tokens"))?,
        }),
    })
}

/// The seeds a list names; 1 to 3 by default.
fn seeds(list: Option<String>) -> Result<Vec<u64>, String> {
    match list {
        Some(list) => list
            .split(',')
            .map(|s| number(s.trim(), "--seeds"))
            .collect(),
        None => Ok(vec![1, 2, 3]),
    }
}

fn number(value: &str, flag: &str) -> Result<u64, String> {
    value
        .trim()
        .parse()
        .map_err(|_| format!("{flag} takes a whole number, not {value}"))
}

fn real(value: &str, flag: &str) -> Result<f64, String> {
    value
        .trim()
        .parse()
        .map_err(|_| format!("{flag} takes a number, not {value}"))
}

fn route(take: &mut impl FnMut(&str) -> Option<String>) -> Result<Route, String> {
    let engine = take("engine").ok_or("--engine is required")?;
    let wire = Wire::parse(&engine).ok_or_else(|| format!("no engine is called {engine}"))?;
    let model = take("model").ok_or("--model is required")?;
    let pin: Vec<String> = take("provider")
        .map(|p| p.split(',').map(|s| s.trim().to_owned()).collect())
        .unwrap_or_default();
    if !pin.is_empty() && wire != Wire::OpenRouter {
        return Err("--provider pins OpenRouter's routing; it needs --engine openrouter".into());
    }
    Ok(Route {
        wire,
        model,
        url: take("url"),
        key_env: take("key-env").unwrap_or_else(|| wire.key_env().to_owned()),
        pin,
    })
}

fn options(take: &mut impl FnMut(&str) -> Option<String>) -> Result<Options, String> {
    let mut options = Options::default();
    if let Some(language) = take("language") {
        options.language = language;
    }
    if let Some(tokens) = take("budget-tokens") {
        let tokens = number(&tokens, "--budget-tokens")?;
        options.budget.tokens = (tokens > 0).then_some(tokens);
    }
    if let Some(usd) = take("budget-usd") {
        options.budget.usd = Some(real(&usd, "--budget-usd")?).filter(|u| *u > 0.0);
    }
    if let Some(hours) = take("timeout-hours") {
        options.timeout = Duration::from_secs_f64(real(&hours, "--timeout-hours")? * 3600.0);
    }
    if let Some(window) = take("window") {
        options.window = number(&window, "--window")?;
    }
    Ok(options)
}

/// Runs `command`; what it prints goes to stdout, the logs to the output
/// directory's.
pub async fn execute(command: Command) -> Result<(), String> {
    match command {
        Command::Premises => {
            for p in frozen()? {
                println!("{}  {}  {}", p.sha256, p.id, p.canon.arc.premise);
            }
            Ok(())
        }
        Command::Run {
            job,
            route,
            options,
        } => {
            let backend = route.backend()?;
            let trace = run_story(&job, &route, backend, &options)
                .await
                .map_err(|e| e.to_string())?;
            println!(
                "{}: {} — {} words, {}",
                job.dir.display(),
                trace["ending"],
                trace["words"],
                trace["spent"]
            );
            Ok(())
        }
        Command::Batch {
            batch: plan,
            route,
            options,
        } => batch(&plan, &route, &options).await,
        Command::Judge { judging, judge } => {
            let backend = judge.route.backend()?;
            judge::judge(&judging, &judge, backend).await
        }
        Command::Verdict { out } => {
            let records = judge::records(&out).await;
            let verdict = judge::verdict(&records);
            let text = serde_json::to_string_pretty(&verdict).unwrap_or_default();
            tokio::fs::write(out.join("verdict.json"), &text)
                .await
                .map_err(|e| format!("{}: {e}", out.display()))?;
            println!("{text}");
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "eval_tests.rs"]
mod tests;

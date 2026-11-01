//! Gate G1 as a harness (docs/fabula.md §4, PR 4; docs/research/
//! fabula-engine.md §13): scenes simulated from the fixture canon through the
//! application's own clients, each in a story of its own, and three things
//! measured from what they left:
//!
//! - **leaks** — every character call read back from the call log and audited
//!   for a planted token its character did not know at that moment
//!   ([`super::leaks::audit`]); the gate wants none;
//! - **first attempts** — the share of calls whose first attempt met its
//!   schema; the gate wants at least 99%;
//! - **the World Master's refusals**, per scene: proposals it refused, events
//!   it wrote that the engine refused, and the slips of its rulings.
//!
//! Characters run with thinking off (the application's mute: `thinking: false`,
//! effort `none`, budget `0`), the World Master with thinking on at a low
//! effort, unless `MINDFORK_FABULA_WM_THINKING=off`.
//!
//! `#[ignore]`, skipped without its variables:
//!
//! - the OpenAI-compatible engine: `MINDFORK_ENGINE_URL` (+ `_KEY`, `_MODEL`),
//!   `MINDFORK_ENGINE_DIALECT=openrouter` for the gateway's client;
//! - Claude: `MINDFORK_ANTHROPIC_KEY` and `MINDFORK_FABULA_CLAUDE_MODEL`;
//! - `MINDFORK_FABULA_G1_SCENES` (20), `_K` (3), `_ROUNDS` (8), `_PARALLEL`
//!   (scenes at once, 4), `_OUT` (where each scene's trace and calls are
//!   written; `target/fabula/g1-<model>` by default), `_ONLY` (the scenes
//!   to run, `1,3,7-9`, in place of all of them), `_BOUND_SECS` (a scene's
//!   bound, 2700: a slow route's queue under many scenes at once can pass it);
//! - `MINDFORK_FABULA_WM_THINKING` (`on`), `MINDFORK_FABULA_WM_EFFORT` (`low`).
//!
//! `cargo test simulation_live -- --ignored --nocapture --test-threads=1`

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::stream::{self, StreamExt};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::simulation::{BranchResult, ScenePlan, SceneRun, SimConfig, simulate_scene};
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::entities::story::{BranchId, Proposal, SceneSetup, fixture_canon};
use crate::features::story::contracts::{InitiativeRuling, ReactionsRuling, ReflectionReply};
use crate::features::story::engine::EngineAdapter;
use crate::features::story::leaks::{AuditedCall, Leak, audit};
use crate::features::story::port::Role;
use crate::features::story::scene::tests::{FixtureCall, fresh_story};
use crate::features::story::schema::{CanonKeys, Schema, StorySchema, extract_object};
use crate::shared::api::AnthropicClient;
use crate::shared::api::contract::EngineBackend;
use crate::shared::api::live_client;
use crate::shared::api::retry::RetryBackend;

/// A scene that hangs fails instead of holding the run (docs/lessons.md §2).
fn bound() -> Duration {
    Duration::from_secs(env_or("MINDFORK_FABULA_G1_BOUND_SECS", 45 * 60))
}

/// The scenes to run: `MINDFORK_FABULA_G1_ONLY` (`1,3,7-9`) or all of them.
fn chosen(scenes: usize) -> Vec<usize> {
    let Ok(only) = std::env::var("MINDFORK_FABULA_G1_ONLY") else {
        return (0..scenes).collect();
    };
    only.split(',')
        .filter_map(|part| match part.trim().split_once('-') {
            Some((a, b)) => Some((a.parse().ok()?..=b.parse().ok()?).collect::<Vec<usize>>()),
            None => part.trim().parse().ok().map(|n| vec![n]),
        })
        .flatten()
        .collect()
}

fn env_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// The route the variables name: the backend, the model's name, and whether
/// it is Claude.
fn route() -> Option<(Arc<dyn EngineBackend>, String)> {
    if let (Ok(key), Ok(model)) = (
        std::env::var("MINDFORK_ANTHROPIC_KEY"),
        std::env::var("MINDFORK_FABULA_CLAUDE_MODEL"),
    ) {
        let client = AnthropicClient::new("https://api.anthropic.com", key, model.clone());
        return Some((RetryBackend::wrap(Arc::new(client)), model));
    }
    let client = live_client("MINDFORK_ENGINE_URL", "MINDFORK_ENGINE_KEY")?;
    let gateway = std::env::var("MINDFORK_ENGINE_DIALECT").is_ok_and(|d| d.trim() == "openrouter");
    let client = if gateway {
        client.for_openrouter(false)
    } else {
        client
    };
    let model = std::env::var("MINDFORK_ENGINE_MODEL").unwrap_or_else(|_| "unnamed".to_owned());
    Some((RetryBackend::wrap(Arc::new(client)), model))
}

/// The characters' sampling: thinking off, as the application mutes a turn.
fn character_sampling() -> SamplingConfig {
    SamplingConfig {
        temperature: Some(0.8),
        thinking: Some(false),
        reasoning_effort: Some(ReasoningEffort::None),
        reasoning_budget: Some(0),
        max_tokens: Some(4096),
        ..SamplingConfig::default()
    }
}

/// The World Master's: thinking on at a low effort, unless asked otherwise.
fn master_sampling() -> SamplingConfig {
    let on = std::env::var("MINDFORK_FABULA_WM_THINKING").map_or(true, |v| v.trim() != "off");
    if !on {
        return SamplingConfig {
            temperature: Some(0.6),
            max_tokens: Some(8192),
            ..character_sampling()
        };
    }
    let effort = match std::env::var("MINDFORK_FABULA_WM_EFFORT")
        .as_deref()
        .map(str::trim)
    {
        Ok("medium") => ReasoningEffort::Medium,
        Ok("high") => ReasoningEffort::High,
        _ => ReasoningEffort::Low,
    };
    SamplingConfig {
        temperature: Some(0.6),
        thinking: Some(true),
        reasoning_effort: Some(effort),
        max_tokens: Some(16384),
        ..SamplingConfig::default()
    }
}

/// The five scenes the harness deals in turn, each with a pressure that
/// pushes on someone's secret.
fn setups() -> Vec<(SceneSetup, &'static str)> {
    let setup = |location: &str, present: &[&str], just: &str, pressure: &str| SceneSetup {
        location: location.into(),
        present: present.iter().map(|&c| c.into()).collect(),
        just_happened: just.to_owned(),
        pressure: pressure.to_owned(),
    };
    vec![
        (
            setup(
                "harbour",
                &["mara", "silas", "ines"],
                "The last ferry has been cancelled and the storm has shut the harbour for the \
                 night. Silas came down from the inn to ask after his crates; Ines came down \
                 from the lighthouse to say the lamp's oil will not last till dawn.",
                "The harbour gate closes at sunset, a quarter of an hour from now, and the \
                 customs ledger lies open on the harbour master's table.",
            ),
            "Ines begins to suspect that the ledger is false.",
        ),
        (
            setup(
                "warehouse",
                &["mara", "silas"],
                "Silas asked Mara to let him into the bonded warehouse to check his crates \
                 before the storm breaks; the door's lock is stiff with salt.",
                "A customs officer arrives on the morning ferry, and tonight's crates are not on \
                 the ledger yet.",
            ),
            "Mara wavers about the bribe.",
        ),
        (
            setup(
                "lighthouse",
                &["ines", "mara"],
                "Mara climbed the ninety steps to the lamp room to tell Ines the harbour is \
                 closed for the night.",
                "The lamp's oil will not last till dawn, and midnight is an hour away.",
            ),
            "Each suspects the other of knowing about midnight.",
        ),
        (
            setup(
                "inn",
                &["silas", "ines"],
                "Ines came down to the Gull Inn for oil for the lamp and found Silas alone by \
                 the fire.",
                "The innkeeper has gone to bed, and the lantern on the shelf holds the only \
                 spare oil in town.",
            ),
            "Silas tries to win Ines over.",
        ),
        (
            setup(
                "chapel",
                &["mara", "ines"],
                "Ines found Mara in the Seamen's Chapel, standing before the names of the \
                 drowned.",
                "The chapel bell is about to ring midnight, and Ines has a question she has put \
                 off for days.",
            ),
            "Mara comes close to a confession.",
        ),
    ]
}

/// The schema a call's act answers to.
fn schema_of(address: &str, keys: &CanonKeys) -> Option<Schema> {
    let act = address.rsplit('/').next()?;
    Some(match act {
        "propose" | "react" => Proposal::schema(keys),
        "reflect" => ReflectionReply::schema(keys),
        "initiative" => InitiativeRuling::schema(keys),
        "reactions" => ReactionsRuling::schema(keys),
        _ => return None,
    })
}

/// One scene's measurements.
#[derive(Default)]
struct SceneTally {
    calls: usize,
    first_valid: usize,
    first_clean: usize,
    retries: usize,
    invalid: Vec<String>,
    by_role: BTreeMap<String, (usize, usize)>,
    mechanisms: BTreeMap<String, usize>,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
    thought_chars: u64,
    leaks: Vec<Leak>,
    branches_failed: Vec<String>,
    endings: Vec<String>,
    deeds: u32,
    world_events: u32,
    waits: u32,
    proposals: u32,
    refused_by_ruling: u32,
    refused_by_engine: Vec<String>,
    slips: Vec<String>,
    seconds: u64,
    error: Option<String>,
}

impl SceneTally {
    fn read_calls(&mut self, calls: &[FixtureCall], keys: &CanonKeys) {
        for call in calls {
            *self.mechanisms.entry(call.mechanism.clone()).or_default() += 1;
            self.input_tokens += call.input_tokens.unwrap_or_default();
            self.output_tokens += call.output_tokens.unwrap_or_default();
            self.cost_usd += call.cost_usd.unwrap_or_default();
            let response = call.response.clone().unwrap_or(Value::Null);
            self.thought_chars += response["thought_chars"].as_u64().unwrap_or_default();
            if call.attempt > 1 {
                self.retries += 1;
                continue;
            }
            self.calls += 1;
            let entry = self.by_role.entry(call.role.clone()).or_default();
            entry.0 += 1;
            let text = response["text"].as_str().unwrap_or_default();
            let valid = call.status == "ok"
                && schema_of(&call.address, keys)
                    .zip(extract_object(text))
                    .is_some_and(|(schema, value)| schema.validate(&value).is_ok());
            if valid {
                self.first_valid += 1;
                entry.1 += 1;
                if serde_json::from_str::<Value>(text.trim()).is_ok() {
                    self.first_clean += 1;
                }
            } else {
                self.invalid.push(format!(
                    "{} [{}]: {}",
                    call.address,
                    call.status,
                    text.chars().take(200).collect::<String>()
                ));
            }
        }
    }

    fn read_run(&mut self, run: &SceneRun) {
        for branch in &run.branches {
            match branch {
                BranchResult::Played(b) => {
                    self.endings
                        .push(format!("{}:{}", b.rounds, b.ending.as_str()));
                    let t = &b.tally;
                    self.deeds += t.committed;
                    self.world_events += t.world_events;
                    self.waits += t.waits;
                    self.proposals += t.intents + t.reactions;
                    self.refused_by_ruling += t.refused_by_ruling;
                    self.refused_by_engine.extend(
                        t.refused_by_engine
                            .iter()
                            .map(|r| format!("{}: {r}", b.branch)),
                    );
                    self.slips
                        .extend(t.slips.iter().map(|s| format!("{}: {s}", b.branch)));
                }
                BranchResult::Failed { branch, error } => {
                    self.branches_failed.push(format!("{branch}: {error}"));
                }
            }
        }
    }
}

/// Simulates scene `n` in a story of its own and measures it.
async fn one_scene(
    n: usize,
    backend: Arc<dyn EngineBackend>,
    model: &str,
    config: SimConfig,
    out: &std::path::Path,
) -> SceneTally {
    let started = Instant::now();
    let (mut story, log) = fresh_story().await;
    let port = EngineAdapter::new(
        backend,
        log,
        model,
        character_sampling(),
        CancellationToken::new(),
    )
    .with_role_sampling(Role::WorldMaster, master_sampling());
    let all = setups();
    let (setup, intent) = all[n % all.len()].clone();
    let plan = ScenePlan {
        chapter: 1,
        scene: 1,
        rerun: 0,
        setup,
        intent: intent.to_owned(),
    };
    let played = tokio::time::timeout(
        bound(),
        simulate_scene(
            &story.db,
            &mut story.author,
            &port,
            &story.run,
            &plan,
            &config,
        ),
    )
    .await;
    let mut tally = SceneTally::default();
    let run = match played {
        Ok(Ok(run)) => Some(run),
        Ok(Err(e)) => {
            tally.error = Some(e.to_string());
            None
        }
        Err(_) => {
            tally.error = Some(format!("past {:?}", bound()));
            None
        }
    };
    let canon = fixture_canon();
    let keys = CanonKeys::of(&canon);
    let calls: Vec<FixtureCall> = story
        .db
        .calls(&story.run)
        .await
        .unwrap()
        .iter()
        .map(FixtureCall::of)
        .collect();
    tally.read_calls(&calls, &keys);
    // The audit reads every branch the calls name, played or not.
    let mut paths = BTreeMap::new();
    for k in 0..config.branches {
        let branch = BranchId::new(format!("ch1/sc1/r0/k{k}"));
        if let Ok(path) = story.db.path(&branch).await {
            paths.insert(branch, path);
        }
    }
    let audited: Vec<AuditedCall> = calls
        .iter()
        .map(|c| AuditedCall {
            address: &c.address,
            role: &c.role,
            request: &c.request,
            response: c.response.as_ref(),
        })
        .collect();
    tally.leaks = audit(&canon, &paths, &audited);
    if let Some(run) = &run {
        tally.read_run(run);
    }
    tally.seconds = started.elapsed().as_secs();
    let trace = json!({
        "scene": n,
        "setup": plan.setup,
        "intent": plan.intent,
        "error": tally.error,
        "trace": run.as_ref().map(SceneRun::to_json),
        "calls": calls,
    });
    let file = out.join(format!("scene{n:02}.json"));
    std::fs::write(&file, serde_json::to_string_pretty(&trace).unwrap()).unwrap();
    eprintln!(
        "scene {n:02} ({}): {} calls, {}/{} first valid, {} leaks, {} deeds, refused {} by ruling and {} by the engine, {} slips, {} s{}",
        plan.setup.location,
        tally.calls,
        tally.first_valid,
        tally.calls,
        tally.leaks.len(),
        tally.deeds,
        tally.refused_by_ruling,
        tally.refused_by_engine.len(),
        tally.slips.len(),
        tally.seconds,
        tally
            .error
            .as_ref()
            .map_or(String::new(), |e| format!(" — failed: {e}")),
    );
    tally
}

/// Runs the gate's scenes on `backend` and prints the report; fails on a
/// NO-GO.
async fn gate(backend: Arc<dyn EngineBackend>, model: &str) {
    let scenes: usize = env_or("MINDFORK_FABULA_G1_SCENES", 20);
    let parallel: usize = env_or("MINDFORK_FABULA_G1_PARALLEL", 4);
    let config = SimConfig {
        branches: env_or("MINDFORK_FABULA_G1_K", 3),
        max_rounds: env_or("MINDFORK_FABULA_G1_ROUNDS", 8),
        ..SimConfig::default()
    };
    let out = std::env::var("MINDFORK_FABULA_G1_OUT").map_or_else(
        |_| {
            let safe: String = model
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            PathBuf::from("target/fabula").join(format!("g1-{safe}"))
        },
        PathBuf::from,
    );
    std::fs::create_dir_all(&out).unwrap();
    eprintln!(
        "{model}: {scenes} scenes, K = {}, up to {} rounds, {parallel} at a time; the backend constrains: {}; traces in {}",
        config.branches,
        config.max_rounds,
        backend.enforces_output_schema().await,
        out.display()
    );
    let picked = chosen(scenes);
    let tallies: Vec<SceneTally> = stream::iter(picked.clone())
        .map(|n| one_scene(n, backend.clone(), model, config, &out))
        .buffered(parallel.max(1))
        .collect()
        .await;

    let mut lines = vec![
        "| Scene | Place | Calls | First valid | Leaks | Branch endings | Deeds | World events | Refused by ruling | Refused by engine | Slips | Seconds |".to_owned(),
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |".to_owned(),
    ];
    let places = setups();
    for (&n, t) in picked.iter().zip(&tallies) {
        lines.push(format!(
            "| {n} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            places[n % places.len()].0.location,
            t.calls,
            t.first_valid,
            t.leaks.len(),
            if t.error.is_some() {
                "failed".to_owned()
            } else {
                t.endings.join(" ")
            },
            t.deeds,
            t.world_events,
            t.refused_by_ruling,
            t.refused_by_engine.len(),
            t.slips.len(),
            t.seconds,
        ));
    }
    let sum = |f: fn(&SceneTally) -> usize| tallies.iter().map(f).sum::<usize>();
    let calls = sum(|t| t.calls);
    let valid = sum(|t| t.first_valid);
    let clean = sum(|t| t.first_clean);
    let leaks = sum(|t| t.leaks.len());
    let retries = sum(|t| t.retries);
    let failed_scenes = sum(|t| usize::from(t.error.is_some()));
    let failed_branches = sum(|t| t.branches_failed.len());
    let share = if calls == 0 {
        0.0
    } else {
        valid as f64 / calls as f64
    };
    let mut roles: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut mechanisms: BTreeMap<String, usize> = BTreeMap::new();
    for t in &tallies {
        for (role, (n, v)) in &t.by_role {
            let e = roles.entry(role.clone()).or_default();
            e.0 += n;
            e.1 += v;
        }
        for (m, n) in &t.mechanisms {
            *mechanisms.entry(m.clone()).or_default() += n;
        }
    }
    let input: u64 = tallies.iter().map(|t| t.input_tokens).sum();
    let output: u64 = tallies.iter().map(|t| t.output_tokens).sum();
    let thought: u64 = tallies.iter().map(|t| t.thought_chars).sum();
    let cost: f64 = tallies.iter().map(|t| t.cost_usd).sum();
    let proposals: u32 = tallies.iter().map(|t| t.proposals).sum();
    let waits: u32 = tallies.iter().map(|t| t.waits).sum();
    let by_ruling: u32 = tallies.iter().map(|t| t.refused_by_ruling).sum();
    let by_engine = sum(|t| t.refused_by_engine.len());
    let slips = sum(|t| t.slips.len());
    eprintln!("\n{model}\n{}\n", lines.join("\n"));
    eprintln!(
        "first attempts schema-valid: {valid}/{calls} ({:.2}%), clean JSON {clean}; by role {roles:?}; \
         mechanisms {mechanisms:?}; extra attempts {retries}",
        share * 100.0
    );
    eprintln!(
        "leaks {leaks}; scenes failed {failed_scenes}; branches failed {failed_branches}; proposals {proposals} \
         ({waits} waits); refused by the World Master {by_ruling}, by the engine {by_engine}; slips {slips}"
    );
    eprintln!("tokens in {input}, out {output}; thought chars {thought}; cost ${cost:.4}");
    for t in &tallies {
        for line in t
            .invalid
            .iter()
            .chain(&t.refused_by_engine)
            .chain(&t.slips)
            .chain(&t.branches_failed)
        {
            eprintln!("  {line}");
        }
        for leak in &t.leaks {
            eprintln!("  LEAK {leak}");
        }
    }
    let go = leaks == 0 && share >= 0.99 && failed_scenes == 0;
    eprintln!("Gate G1 on {model}: {}", if go { "GO" } else { "NO-GO" });
    assert_eq!(leaks, 0, "the gate wants no leak");
    assert!(
        share >= 0.99,
        "the gate wants 99% valid first attempts: {share}"
    );
    assert_eq!(failed_scenes, 0, "every scene ran to its end");
}

#[tokio::test]
#[ignore = "needs a live model: MINDFORK_ANTHROPIC_KEY + MINDFORK_FABULA_CLAUDE_MODEL, or MINDFORK_ENGINE_URL (+ _KEY, _MODEL, _DIALECT)"]
async fn simulation_live_gate_g1() {
    let Some((backend, model)) = route() else {
        eprintln!("skip: no live route; see the module's head");
        return;
    };
    gate(backend, &model).await;
}

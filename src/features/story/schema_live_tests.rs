//! The live probe of docs/research/fabula-engine.md §8 as a smoke
//! (docs/fabula.md §4, PR 3): the harbour scene's first round — three
//! characters proposing at once — played through the application's own
//! clients and the engine adapter, under schema-constrained decoding and under
//! the fallback, with thinking off and on, and every attempt read back from
//! the call log.
//!
//! What it reports per arm: the calls, how their first attempts ended, how many
//! of those met the schema, how many were clean JSON (no fence, no prose), the
//! extra attempts, and the median output, thought and latency. What it asserts:
//! that each arm ran on the mechanism it names, and that under the constraint
//! every first attempt that finished met the schema — which is the decoder's
//! promise, not the model's.
//!
//! `#[ignore]`, skipped without its variables:
//!
//! - the OpenAI-compatible engine: `MINDFORK_ENGINE_URL` (+ `_KEY`, `_MODEL`);
//!   `MINDFORK_ENGINE_DIALECT=openrouter` builds the gateway's client, whose
//!   catalogue decides the mechanism and which asks for `require_parameters`;
//! - a cloud's own wire ([`Cloud`]): Claude (`MINDFORK_ANTHROPIC_KEY`,
//!   `MINDFORK_FABULA_CLAUDE_MODEL`), OpenAI's Responses (`MINDFORK_OPENAI_KEY`,
//!   `MINDFORK_FABULA_OPENAI_MODEL`), Gemini (`MINDFORK_GEMINI_KEY`,
//!   `MINDFORK_FABULA_GEMINI_MODEL`);
//! - `MINDFORK_FABULA_PROBE_SCENES`, the scenes per arm (3 by default, three
//!   calls each).
//!
//! `cargo test schema_live -- --ignored --nocapture --test-threads=1`

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::entities::story::{Proposal, fixture_canon};
use crate::features::story::engine::EngineAdapter;
use crate::features::story::scene::tests::{FixtureCall, fresh_story};
use crate::features::story::scene::{SceneScript, harbour_script, play_scripted_scene};
use crate::features::story::schema::{CanonKeys, Property, Schema, StorySchema, extract_object};
use crate::shared::api::contract::{
    ApiMessage, ChatChunk, ChatRequest, EngineBackend, OutputSchema,
};
use crate::shared::api::retry::RetryBackend;
use crate::shared::api::{AnthropicClient, GeminiClient, ResponsesClient, live_client};

/// A hung route fails its arm instead of holding the run (docs/lessons.md §2).
const BOUND: Duration = Duration::from_secs(600);

/// A cloud reached on its own wire, as the application's cloud modes reach it.
#[derive(Debug, Clone, Copy)]
pub(super) enum Cloud {
    Claude,
    OpenAi,
    Gemini,
}

impl Cloud {
    pub(super) const ALL: [Cloud; 3] = [Cloud::Claude, Cloud::OpenAi, Cloud::Gemini];

    /// The variables that name the key and the model.
    pub(super) fn vars(self) -> (&'static str, &'static str) {
        match self {
            Cloud::Claude => ("MINDFORK_ANTHROPIC_KEY", "MINDFORK_FABULA_CLAUDE_MODEL"),
            Cloud::OpenAi => ("MINDFORK_OPENAI_KEY", "MINDFORK_FABULA_OPENAI_MODEL"),
            Cloud::Gemini => ("MINDFORK_GEMINI_KEY", "MINDFORK_FABULA_GEMINI_MODEL"),
        }
    }

    /// The backend and the model's name, or `None` when a variable is unset.
    pub(super) fn backend(self) -> Option<(Arc<dyn EngineBackend>, String)> {
        let (key, model) = self.vars();
        let (Ok(key), Ok(model)) = (std::env::var(key), std::env::var(model)) else {
            return None;
        };
        let client: Arc<dyn EngineBackend> = match self {
            Cloud::Claude => Arc::new(AnthropicClient::new(
                "https://api.anthropic.com",
                key,
                model.clone(),
            )),
            Cloud::OpenAi => Arc::new(ResponsesClient::new(
                "https://api.openai.com/v1",
                key,
                model.clone(),
            )),
            Cloud::Gemini => Arc::new(GeminiClient::new(
                "https://generativelanguage.googleapis.com/v1beta",
                key,
                model.clone(),
            )),
        };
        Some((RetryBackend::wrap(client), model))
    }
}

/// The OpenAI-compatible engine the variables name, the gateway's client when
/// `MINDFORK_ENGINE_DIALECT=openrouter`.
pub(super) fn engine_route() -> Option<(Arc<dyn EngineBackend>, String)> {
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

/// One arm: a mechanism and a thinking setting.
struct Arm {
    name: &'static str,
    fallback_only: bool,
    sampling: SamplingConfig,
}

/// Thinking off is the three fields the application's muted turns send
/// (`app::orchestrator::title`): on a template with thinking baked in (Qwen,
/// Gemma) only `reasoning_budget: 0` stops it, and `thinking: false` alone let
/// Qwen 3.8 think 6,000 characters a call on the local llama.cpp.
fn arms(effort_when_thinking: Option<ReasoningEffort>) -> Vec<Arm> {
    let sampling = |thinking: bool| SamplingConfig {
        temperature: Some(0.8),
        thinking: Some(thinking),
        reasoning_effort: if thinking {
            effort_when_thinking
        } else {
            Some(ReasoningEffort::None)
        },
        reasoning_budget: (!thinking).then_some(0),
        max_tokens: Some(6144),
        ..SamplingConfig::default()
    };
    vec![
        Arm {
            name: "schema, thinking off",
            fallback_only: false,
            sampling: sampling(false),
        },
        Arm {
            name: "fallback, thinking off",
            fallback_only: true,
            sampling: sampling(false),
        },
        Arm {
            name: "schema, thinking on",
            fallback_only: false,
            sampling: sampling(true),
        },
        Arm {
            name: "fallback, thinking on",
            fallback_only: true,
            sampling: sampling(true),
        },
    ]
}

fn scenes() -> usize {
    std::env::var("MINDFORK_FABULA_PROBE_SCENES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3)
}

/// What one arm left in its call logs.
#[derive(Default)]
struct Tally {
    calls: usize,
    statuses: Vec<String>,
    valid: usize,
    clean: usize,
    extra: usize,
    output: Vec<u64>,
    thoughts: Vec<u64>,
    latency: Vec<u64>,
    mechanisms: Vec<String>,
    failed_scenes: Vec<String>,
    invalid: Vec<String>,
    served_by: std::collections::BTreeSet<String>,
}

fn median(values: &[u64]) -> u64 {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted.get(sorted.len() / 2).copied().unwrap_or_default()
}

impl Tally {
    fn add(&mut self, calls: &[FixtureCall]) {
        let schema = Proposal::schema(&CanonKeys::of(&fixture_canon()));
        for call in calls {
            self.mechanisms.push(call.mechanism.clone());
            if call.attempt > 1 {
                self.extra += 1;
                continue;
            }
            self.calls += 1;
            self.statuses.push(call.status.clone());
            let response = call.response.clone().unwrap_or(Value::Null);
            let text = response["text"].as_str().unwrap_or_default();
            if let Some(route) = response["served_by"].as_str() {
                self.served_by.insert(route.to_owned());
            }
            self.thoughts
                .push(response["thought_chars"].as_u64().unwrap_or_default());
            self.output.extend(call.output_tokens);
            self.latency.extend(call.latency_ms);
            if call.status != "ok" {
                continue;
            }
            if serde_json::from_str::<Value>(text.trim()).is_ok() {
                self.clean += 1;
            }
            match extract_object(text).map(|v| schema.validate(&v)) {
                Some(Ok(_)) => self.valid += 1,
                _ => self.invalid.push(format!(
                    "{}: {}",
                    call.address,
                    text.chars().take(160).collect::<String>()
                )),
            }
        }
    }

    fn ok(&self) -> usize {
        self.statuses.iter().filter(|s| *s == "ok").count()
    }

    fn line(&self, arm: &str) -> String {
        format!(
            "| {arm} | {} | {} ok / {} cut / {} other | {}/{} | {}/{} | {} | {} | {} | {} | {} |",
            self.calls,
            self.ok(),
            self.statuses.iter().filter(|s| *s == "cut").count(),
            self.calls - self.ok() - self.statuses.iter().filter(|s| *s == "cut").count(),
            self.valid,
            self.calls,
            self.clean,
            self.calls,
            self.extra,
            median(&self.output),
            median(&self.thoughts),
            median(&self.latency),
            if self.served_by.is_empty() {
                "-".to_owned()
            } else {
                self.served_by
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        )
    }
}

/// Plays the arms on `backend` and prints the table; asserts the mechanism of
/// each arm and the constraint's promise.
async fn probe(backend: Arc<dyn EngineBackend>, model: &str, arms: Vec<Arm>) {
    let enforces = backend.enforces_output_schema().await;
    eprintln!("{model}: the backend says it constrains: {enforces}");
    let script = SceneScript {
        rounds: 1,
        ..harbour_script()
    };
    let mut lines = vec![
        "| Arm | Calls | First attempts | Schema-valid | Clean JSON | Extra attempts | Median output tokens | Median thought chars | Median ms | Served by |".to_owned(),
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |".to_owned(),
    ];
    let mut failures = Vec::new();
    for arm in arms {
        let mut tally = Tally::default();
        for n in 0..scenes() {
            let (mut story, log) = fresh_story().await;
            let mut port = EngineAdapter::new(
                backend.clone(),
                log,
                model,
                arm.sampling.clone(),
                CancellationToken::new(),
            );
            if arm.fallback_only {
                port = port.fallback_only();
            }
            let played = tokio::time::timeout(
                BOUND,
                play_scripted_scene(&story.db, &mut story.author, &port, &story.run, &script),
            )
            .await;
            match played {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => tally.failed_scenes.push(format!("scene {n}: {e}")),
                Err(_) => tally
                    .failed_scenes
                    .push(format!("scene {n}: past {BOUND:?}")),
            }
            let calls = story.db.calls(&story.run).await.unwrap();
            tally.add(&calls.iter().map(FixtureCall::of).collect::<Vec<_>>());
        }
        lines.push(tally.line(arm.name));
        for f in tally.failed_scenes.iter().chain(&tally.invalid) {
            eprintln!("  {}: {f}", arm.name);
        }
        let wanted = if arm.fallback_only || !enforces {
            "fallback"
        } else {
            "schema"
        };
        if let Some(other) = tally.mechanisms.iter().find(|m| *m != wanted) {
            failures.push(format!("{}: ran on {other}, not {wanted}", arm.name));
        }
        if wanted == "schema" && tally.valid < tally.ok() {
            failures.push(format!(
                "{}: {} of {} finished first attempts met the schema under the constraint",
                arm.name,
                tally.valid,
                tally.ok()
            ));
        }
    }
    eprintln!("\n{model}\n{}\n", lines.join("\n"));
    assert_eq!(failures, Vec::<String>::new());
}

#[tokio::test]
#[ignore = "needs a live model: MINDFORK_ENGINE_URL (+ _KEY, _MODEL, MINDFORK_ENGINE_DIALECT=openrouter)"]
async fn schema_live_probe_on_the_engine() {
    let Some((backend, model)) = engine_route() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    probe(backend, &model, arms(None)).await;
}

/// The probe on a cloud's own wire: the "off" arms send the application's
/// mute, which each client spells in its model's terms, and the "on" arms
/// thinking at the lowest effort; the thought column says what each did.
async fn probe_on(cloud: Cloud) {
    let Some((backend, model)) = cloud.backend() else {
        let (key, model) = cloud.vars();
        eprintln!("skip: {key} and {model} are needed");
        return;
    };
    probe(backend, &model, arms(Some(ReasoningEffort::Low))).await;
}

#[tokio::test]
#[ignore = "needs MINDFORK_ANTHROPIC_KEY + MINDFORK_FABULA_CLAUDE_MODEL"]
async fn schema_live_probe_on_claude() {
    probe_on(Cloud::Claude).await;
}

#[tokio::test]
#[ignore = "needs MINDFORK_OPENAI_KEY + MINDFORK_FABULA_OPENAI_MODEL"]
async fn schema_live_probe_on_openai() {
    probe_on(Cloud::OpenAi).await;
}

#[tokio::test]
#[ignore = "needs MINDFORK_GEMINI_KEY + MINDFORK_FABULA_GEMINI_MODEL"]
async fn schema_live_probe_on_gemini() {
    probe_on(Cloud::Gemini).await;
}

/// One reply's text, the stream drained.
async fn reply_text(backend: &dyn EngineBackend, req: ChatRequest) -> String {
    let mut stream = backend
        .chat_stream(req, CancellationToken::new())
        .await
        .expect("the request is taken");
    let mut text = String::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            ChatChunk::Text(t) => text.push_str(&t),
            ChatChunk::Error { message, .. } => panic!("the provider failed: {message}"),
            _ => {}
        }
    }
    text
}

/// The constraint is the provider's, not the prompt's: asked in words for a
/// bare greeting and never shown the schema, the reply is the schema's object
/// all the same; without the schema it is the greeting.
async fn holds_against_the_prompt(backend: Arc<dyn EngineBackend>) {
    let schema = Schema::Object(vec![Property::new(
        "mood",
        Schema::Enum(vec!["calm".to_owned(), "grim".to_owned()]),
    )]);
    let ask = |output_schema: Option<OutputSchema>| ChatRequest {
        system: Some("Reply with the single word hello, and nothing else.".to_owned()),
        messages: vec![ApiMessage::user("Say it.")],
        sampling: SamplingConfig {
            max_tokens: Some(256),
            thinking: Some(false),
            reasoning_effort: Some(ReasoningEffort::None),
            reasoning_budget: Some(0),
            ..SamplingConfig::default()
        },
        output_schema,
        ..ChatRequest::default()
    };
    let free = reply_text(backend.as_ref(), ask(None)).await;
    let bound = OutputSchema::new("mood", schema.to_json()).unwrap();
    let held = reply_text(backend.as_ref(), ask(Some(bound))).await;
    eprintln!("without the schema: {free:?}\nwith it: {held:?}");
    let value: Value = serde_json::from_str(held.trim()).expect("the constrained reply is JSON");
    assert!(schema.validate(&value).is_ok(), "{held}");
    assert!(
        serde_json::from_str::<Value>(free.trim()).is_err(),
        "the control: unconstrained, the model says the word ({free:?})"
    );
}

#[tokio::test]
#[ignore = "needs a live model: MINDFORK_ENGINE_URL (+ _KEY, _MODEL, MINDFORK_ENGINE_DIALECT=openrouter)"]
async fn schema_live_holds_on_the_engine_against_the_prompt() {
    let Some((backend, _)) = engine_route() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    holds_against_the_prompt(backend).await;
}

async fn holds_on(cloud: Cloud) {
    let Some((backend, _)) = cloud.backend() else {
        let (key, model) = cloud.vars();
        eprintln!("skip: {key} and {model} are needed");
        return;
    };
    holds_against_the_prompt(backend).await;
}

#[tokio::test]
#[ignore = "needs MINDFORK_ANTHROPIC_KEY + MINDFORK_FABULA_CLAUDE_MODEL"]
async fn schema_live_holds_on_claude_against_the_prompt() {
    holds_on(Cloud::Claude).await;
}

#[tokio::test]
#[ignore = "needs MINDFORK_OPENAI_KEY + MINDFORK_FABULA_OPENAI_MODEL"]
async fn schema_live_holds_on_openai_against_the_prompt() {
    holds_on(Cloud::OpenAi).await;
}

#[tokio::test]
#[ignore = "needs MINDFORK_GEMINI_KEY + MINDFORK_FABULA_GEMINI_MODEL"]
async fn schema_live_holds_on_gemini_against_the_prompt() {
    holds_on(Cloud::Gemini).await;
}

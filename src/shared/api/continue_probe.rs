//! Stage-0 live probe for `/continue` — docs/research/continue-generation.md §7.
//!
//! Measures, against real servers, whether a trailing assistant message is
//! *continued* (assistant prefill / continue-final-message) — the mechanism the
//! `/continue` command would ride — and how that interacts with thinking models,
//! explicit continuation knobs, and the tool grammar. Every test is `#[ignore]`
//! and silently skips without its env var, like every other live smoke.
//!
//! The first live run reshaped the instrument (docs/lessons.md §3 — suspect the
//! fixture before the feature): llama.cpp **echoes the prefill** back in the
//! response, so continuation and verbatim restart are indistinguishable without
//! a marker the model would never regenerate on its own — hence the invented
//! "Zorbville atlas" in the partial (the compaction smokes' invented-code
//! discipline). And a cut *inside* a single token ("…is Par" of a one-token
//! "Paris") is a state no real stream interruption can produce — streams break
//! at token boundaries — so the asserted arm cuts at a word boundary and the
//! mid-word arm is recorded, not asserted.
//!
//! Go/no-go criteria (research doc §7): [`llama_continues_a_word_boundary_tail`]
//! is the gate for the whole feature;
//! [`thinking_model_rejects_prefill_and_the_kwarg_lifts_it`] for thinking-model
//! support; [`prefill_coexists_with_the_tool_grammar`] decides fork F5.
//!
//! Run (llama.cpp stack):
//! `MINDFORK_ENGINE_URL=http://…:8000/v1 cargo test continue_probe -- --ignored --nocapture --test-threads=1`
//! Cloud arms: `MINDFORK_ANTHROPIC_KEY` / `MINDFORK_GEMINI_KEY` / `MINDFORK_GROK_KEY`.

use futures_util::StreamExt;

use crate::entities::sampling::SamplingConfig;
use crate::shared::api::contract::{
    ApiMessage, ChatChunk, ChatRequest, ChatStream, EngineBackend, FinishReason,
    ToolCallAccumulator, ToolSchema,
};
use crate::shared::api::{AnthropicClient, GeminiClient, OpenAiClient};

const QUESTION: &str = "What is the capital of France? Answer in one short sentence.";
/// The primary partial: cut at a **word boundary**, carrying a marker
/// ("Zorbville") no model would produce on a fresh restart — which is what
/// makes echo, continuation and restart three distinguishable outcomes.
/// No trailing whitespace (Anthropic rejects a prefill that ends in it).
const PARTIAL: &str = "Per the Zorbville atlas, the capital of France is";
/// The mid-word partial ("Par" of "Paris"): a state a real token-boundary
/// interruption cannot produce for a single-token word. Recorded to document
/// the retokenization merge artifact, never asserted.
const PARTIAL_MIDWORD: &str = "Per the Zorbville atlas, the capital of France is Par";
/// The primary partial with a trailing space — what a stream cut right after
/// a whitespace token leaves behind. Anthropic rejects such a prefill as-is;
/// the wire right-trims its copy (stage 2), and this fixture is what proves
/// the trim live.
const PARTIAL_TRAILING: &str = "Per the Zorbville atlas, the capital of France is ";

fn fixture(partial: &str) -> Vec<ApiMessage> {
    vec![ApiMessage::user(QUESTION), ApiMessage::assistant(partial)]
}

/// Deterministic-ish sampling so reruns are comparable across arms.
fn deterministic() -> SamplingConfig {
    SamplingConfig {
        max_tokens: Some(256),
        temperature: Some(0.0),
        seed: Some(42),
        ..Default::default()
    }
}

/// What the reply's bytes say happened, echo-aware. `seam` is whatever sits
/// between the cut and the first real character — whitespace or invisible
/// joiners (the first run measured U+00AD from claude-haiku-4-5 and `"\n"`
/// from Gemini) — kept verbatim because seam exactness is the claim under test.
#[derive(Debug)]
enum Outcome {
    /// The response repeats the partial, then continues it.
    Echoed {
        seam: String,
        cont: String,
    },
    /// The response is the continuation alone.
    Continued {
        seam: String,
        cont: String,
    },
    /// The response re-answers from the top (no marker, restates the question's
    /// words) — prefill did not happen.
    Restarted,
    Unclear,
}

fn split_seam(rest: &str) -> (String, String) {
    let seam: String = rest
        .chars()
        .take_while(|c| c.is_whitespace() || matches!(c, '\u{00AD}' | '\u{200B}' | '\u{FEFF}'))
        .collect();
    let cont = rest[seam.len()..].to_string();
    (seam, cont)
}

/// Both in-distribution completions of the fixture. "Paris" is world
/// knowledge; "Zorbville" is the fictional premise taken at its word —
/// measured live from Qwen 3.6, which answered in-universe where Gemma,
/// haiku-4-5 and Gemini answered from geography. The marker that makes echo
/// detectable also offers the model a second right answer, so the assertion
/// must accept either; a restart or garbage still fails it.
fn answers_the_question(cont: &str) -> bool {
    cont.starts_with("Paris") || cont.starts_with("Zorbville")
}

fn analyze(content: &str, partial: &str) -> Outcome {
    if let Some(rest) = content.strip_prefix(partial) {
        let (seam, cont) = split_seam(rest);
        return Outcome::Echoed { seam, cont };
    }
    let (seam, cont) = split_seam(content);
    if answers_the_question(&cont) {
        return Outcome::Continued { seam, cont };
    }
    if cont.to_lowercase().contains("capital") {
        return Outcome::Restarted;
    }
    Outcome::Unclear
}

/// The seam bytes and continuation text, whether the server echoed the prefill
/// or not; `None` for a restart (nothing continues the partial).
fn continuation(outcome: &Outcome) -> Option<(&str, &str)> {
    match outcome {
        Outcome::Echoed { seam, cont } | Outcome::Continued { seam, cont } => Some((seam, cont)),
        Outcome::Restarted | Outcome::Unclear => None,
    }
}

async fn collect(mut stream: ChatStream) -> (String, String, Option<FinishReason>) {
    let mut text = String::new();
    let mut thoughts = String::new();
    let mut finish = None;
    while let Some(chunk) = stream.next().await {
        match chunk {
            ChatChunk::Text(t) => text.push_str(&t),
            ChatChunk::Thoughts(t) => thoughts.push_str(&t),
            ChatChunk::Error { message, .. } => eprintln!("engine error: {message}"),
            ChatChunk::Finished(r) => {
                finish = Some(r);
                break;
            }
            _ => {}
        }
    }
    (text, thoughts, finish)
}

/// llama-server may prepend a template-injected empty think block to a
/// prefill continuation on Qwen (ggml-org/llama.cpp#21511). The app's
/// fallback thoughts parser strips exactly this shape; the probe mirrors it
/// so the assertion measures the model's text, and reports the residue.
fn strip_think_residue(content: &str) -> (&str, bool) {
    let trimmed = content.trim_start();
    let Some(rest) = trimmed.strip_prefix("<think>") else {
        return (content, false);
    };
    match rest.split_once("</think>") {
        Some((_, after)) => (after, true),
        None => (rest, true),
    }
}

fn bearer(rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match std::env::var("MINDFORK_ENGINE_KEY") {
        Ok(k) if !k.is_empty() => rb.bearer_auth(k),
        _ => rb,
    }
}

/// The feature's gate: a llama.cpp server continues a word-boundary assistant
/// tail through the app's own client, with no extra request fields — the
/// documented default (`--prefill-assistant`, server README). The mid-word arm
/// is recorded for the research doc, not asserted (see the module doc).
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn llama_continues_a_word_boundary_tail() {
    let Some(client) =
        crate::shared::api::live_client("MINDFORK_ENGINE_URL", "MINDFORK_ENGINE_KEY")
    else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };

    // Asserted arm: the word-boundary cut.
    let req = ChatRequest {
        continue_final: false,
        system: None,
        messages: fixture(PARTIAL),
        sampling: deterministic(),
        tools: vec![],
    };
    let (text, thoughts, finish) = collect(
        client
            .chat_stream(req, Default::default())
            .await
            .expect("a plain trailing-assistant request must open a stream"),
    )
    .await;
    let outcome = analyze(&text, PARTIAL);
    println!(
        "llama word-boundary: finish={finish:?} outcome={outcome:?}\nraw={text:?}\nthoughts={thoughts:?}"
    );
    let (seam, cont) = continuation(&outcome)
        .unwrap_or_else(|| panic!("the reply did not continue the tail: {text:?}"));
    println!("seam bytes at the cut: {seam:?}");
    assert!(
        answers_the_question(cont),
        "expected the continuation to resume with the answer, got: {cont:?} (raw {text:?})"
    );
    assert_eq!(finish, Some(FinishReason::Stop));

    // Recorded arm: the mid-word cut (retokenization artifact documentation).
    let req = ChatRequest {
        continue_final: false,
        system: None,
        messages: fixture(PARTIAL_MIDWORD),
        sampling: deterministic(),
        tools: vec![],
    };
    let (text, _thoughts, finish) = collect(
        client
            .chat_stream(req, Default::default())
            .await
            .expect("the mid-word arm must open a stream"),
    )
    .await;
    println!(
        "llama mid-word (recorded, not asserted): finish={finish:?} outcome={:?}\nraw={text:?}",
        analyze(&text, PARTIAL_MIDWORD)
    );
}

/// Thinking models (Qwen): the behaviour is **build-dependent**. The
/// #21889-era rule is a pre-stream 400 ("Assistant response prefill is
/// incompatible with enable_thinking", `--reasoning-budget` default -1) with
/// `chat_template_kwargs {"enable_thinking": false}` as the documented
/// per-request escape. Measured on b10659 (2026-08-27, Qwen 3.6): the plain
/// prefill is **not** rejected — the server accepts it and skips thinking
/// (empty thoughts, no `<think>` residue) — and the kwarg arm continues too.
/// Arm A records whichever behaviour the build has (asserting the reason only
/// when it *does* reject); arm B proves the escape by raw body, since the
/// client has no such knob yet — adding one is the feature.
///
/// Skips (loudly) when the server's model is not a Qwen — rerun after
/// switching the stack.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server with a Qwen thinking model (MINDFORK_ENGINE_URL)"]
async fn thinking_model_rejects_prefill_and_the_kwarg_lifts_it() {
    let Ok(url) = std::env::var("MINDFORK_ENGINE_URL") else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let base = url.trim_end_matches('/').to_string();
    let http = reqwest::Client::new();
    let models: serde_json::Value = bearer(http.get(format!("{base}/models")))
        .send()
        .await
        .expect("GET /v1/models")
        .json()
        .await
        .expect("/v1/models body must be JSON");
    let model_id = models["data"][0]["id"]
        .as_str()
        .unwrap_or("")
        .to_lowercase();
    if !model_id.contains("qwen") {
        eprintln!(
            "skip: server model is {model_id:?}, not a Qwen thinking model — rerun after switching the stack"
        );
        return;
    }

    // Arm A — the default, through the app's own client (no extra fields).
    let client = OpenAiClient::new(base.clone());
    let req = ChatRequest {
        continue_final: false,
        system: None,
        messages: fixture(PARTIAL),
        sampling: deterministic(),
        tools: vec![],
    };
    match client.chat_stream(req, Default::default()).await {
        Err(err) => {
            let msg = format!("{err:#}").to_lowercase();
            println!("arm A (thinking at server default): rejected pre-stream: {err:#}");
            assert!(
                msg.contains("prefill") || msg.contains("thinking"),
                "rejected for an unexpected reason: {err:#}"
            );
        }
        Ok(stream) => {
            let (text, thoughts, finish) = collect(stream).await;
            println!(
                "arm A (thinking at server default): NOT rejected — finish={finish:?} outcome={:?}\nraw={text:?}\nthoughts={thoughts:?}",
                analyze(&text, PARTIAL)
            );
        }
    }

    // Arm B — the escape hatch, raw body. This is the go/no-go.
    let body = serde_json::json!({
        "messages": [
            {"role": "user", "content": QUESTION},
            {"role": "assistant", "content": PARTIAL},
        ],
        "temperature": 0.0,
        "max_tokens": 256,
        "stream": false,
        "chat_template_kwargs": {"enable_thinking": false},
    });
    let resp = bearer(http.post(format!("{base}/chat/completions")).json(&body))
        .send()
        .await
        .expect("POST /v1/chat/completions (arm B)");
    let status = resp.status();
    let v: serde_json::Value = resp.json().await.expect("arm B body must be JSON");
    assert!(
        status.is_success(),
        "the enable_thinking=false escape was rejected ({status}): {v}"
    );
    let content = v["choices"][0]["message"]["content"].as_str().unwrap_or("");
    let (stripped, had_residue) = strip_think_residue(content);
    let outcome = analyze(stripped.trim_start(), PARTIAL);
    println!(
        "arm B (enable_thinking=false): status={status} think_residue={had_residue} outcome={outcome:?}\nraw={content:?}"
    );
    let (seam, cont) = continuation(&outcome)
        .unwrap_or_else(|| panic!("no continuation after the kwarg escape: {content:?}"));
    println!("seam bytes at the cut: {seam:?}");
    assert!(
        answers_the_question(cont),
        "expected a continuation after the kwarg escape, got: {cont:?} (raw {content:?})"
    );
}

/// Fork F5: a continuation request carries the turn's tools; the continued
/// round must still be able to end in a *parsed* tool call (not prose).
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn prefill_coexists_with_the_tool_grammar() {
    let Some(client) =
        crate::shared::api::live_client("MINDFORK_ENGINE_URL", "MINDFORK_ENGINE_KEY")
    else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let tool = ToolSchema {
        name: "get_weather".into(),
        description: "Get the current weather for a city.".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {"city": {"type": "string"}},
            "required": ["city"],
        }),
    };
    let req = ChatRequest {
        continue_final: false,
        system: None,
        messages: vec![
            ApiMessage::user(
                "What is the weather in Paris right now? You must call the get_weather tool — do not answer from memory.",
            ),
            ApiMessage::assistant("Sure — let me check the current conditions"),
        ],
        sampling: deterministic(),
        tools: vec![tool],
    };
    let mut stream = client
        .chat_stream(req, Default::default())
        .await
        .expect("a trailing-assistant request with tools must open a stream");
    let mut text = String::new();
    let mut acc = ToolCallAccumulator::default();
    let mut finish = None;
    while let Some(chunk) = stream.next().await {
        match chunk {
            ChatChunk::Text(t) => text.push_str(&t),
            ChatChunk::ToolCall(d) => acc.push(d),
            ChatChunk::Error { message, .. } => eprintln!("engine error: {message}"),
            ChatChunk::Finished(r) => {
                finish = Some(r);
                break;
            }
            _ => {}
        }
    }
    let calls = acc.finish();
    println!("prefill+tools: finish={finish:?} calls={calls:?}\ncontinued text={text:?}");
    assert!(
        calls.iter().any(|c| c.name == "get_weather"),
        "the continued round produced no parsed tool call (F5 no-go evidence): finish={finish:?} text={text:?}"
    );
}

/// The explicit knobs (`continue_final_message` + `add_generation_prompt`),
/// which vLLM requires and newer llama.cpp also understands. Three arms:
/// knobs ON must continue (go/no-go — on llama.cpp default prefill continues
/// even if the fields are ignored, which is exactly the compatibility the
/// design relies on); knobs explicitly OFF discriminates "honoured" from
/// "ignored"; a wrong-typed value discriminates "validated" from "coerced"
/// (the xAI lesson: a 200 is not proof a parameter works).
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn explicit_continuation_knobs_probe() {
    let Ok(url) = std::env::var("MINDFORK_ENGINE_URL") else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let base = url.trim_end_matches('/').to_string();
    let http = reqwest::Client::new();
    let messages = serde_json::json!([
        {"role": "user", "content": QUESTION},
        {"role": "assistant", "content": PARTIAL},
    ]);

    // Arm A: knobs ON.
    let body_on = serde_json::json!({
        "messages": messages, "temperature": 0.0, "max_tokens": 256, "stream": false,
        "continue_final_message": true, "add_generation_prompt": false,
    });
    let resp = bearer(http.post(format!("{base}/chat/completions")).json(&body_on))
        .send()
        .await
        .expect("POST (knobs on)");
    let status_on = resp.status();
    let v: serde_json::Value = resp.json().await.expect("knobs-on body must be JSON");
    let content_on = v["choices"][0]["message"]["content"].as_str().unwrap_or("");
    let outcome_on = analyze(content_on, PARTIAL);
    println!("knobs ON: status={status_on} outcome={outcome_on:?} raw={content_on:?}");
    assert!(
        status_on.is_success(),
        "explicit knobs rejected ({status_on}): {v}"
    );
    let (seam, cont) = continuation(&outcome_on).unwrap_or_else(|| {
        panic!("with the explicit knobs the reply did not continue: {content_on:?}")
    });
    println!("seam bytes at the cut: {seam:?}");
    assert!(
        answers_the_question(cont),
        "with the explicit knobs the reply did not continue: {cont:?} (raw {content_on:?})"
    );

    // Arm B: knobs explicitly OFF — a server that honours the fields restarts;
    // one that ignores them continues via default prefill. Recorded, not asserted.
    let body_off = serde_json::json!({
        "messages": messages, "temperature": 0.0, "max_tokens": 256, "stream": false,
        "continue_final_message": false, "add_generation_prompt": true,
    });
    let resp = bearer(
        http.post(format!("{base}/chat/completions"))
            .json(&body_off),
    )
    .send()
    .await
    .expect("POST (knobs off)");
    let status_off = resp.status();
    let v: serde_json::Value = resp.json().await.expect("knobs-off body must be JSON");
    let content_off = v["choices"][0]["message"]["content"].as_str().unwrap_or("");
    let outcome_off = analyze(content_off, PARTIAL);
    let honoured = match &outcome_off {
        Outcome::Restarted => "server honours the explicit fields",
        Outcome::Echoed { .. } | Outcome::Continued { .. } => {
            "fields ignored - default prefill only"
        }
        Outcome::Unclear => "unclear",
    };
    println!(
        "knobs OFF: status={status_off} outcome={outcome_off:?} -> {honoured}\nraw={content_off:?}"
    );

    // Arm C: wrong type — 4xx means the field is validated (known), 2xx means
    // it was silently dropped (unknown to this server).
    let body_bad = serde_json::json!({
        "messages": messages, "temperature": 0.0, "max_tokens": 16, "stream": false,
        "continue_final_message": "banana",
    });
    let resp = bearer(
        http.post(format!("{base}/chat/completions"))
            .json(&body_bad),
    )
    .send()
    .await
    .expect("POST (wrong type)");
    let status_bad = resp.status();
    println!(
        "wrong-typed continue_final_message: status={status_bad} -> {}",
        if status_bad.is_client_error() {
            "field validated (server knows it)"
        } else {
            "field silently dropped (server does not know it)"
        }
    );
}

fn anthropic_client(model: &str) -> Option<AnthropicClient> {
    let key = std::env::var("MINDFORK_ANTHROPIC_KEY").ok()?;
    Some(AnthropicClient::new(
        "https://api.anthropic.com",
        key,
        model.to_string(),
    ))
}

/// One Anthropic continuation request through the app's own wire
/// (`continue_final: true` — stage 2), optionally asking for extended
/// thinking, which the wire must drop.
async fn anthropic_continue(
    client: &AnthropicClient,
    partial: &'static str,
    thinking: bool,
) -> (String, Option<FinishReason>) {
    let mut sampling = deterministic();
    sampling.thinking = thinking.then_some(true);
    let req = ChatRequest {
        continue_final: true,
        system: None,
        messages: fixture(partial),
        sampling,
        tools: vec![],
    };
    let (text, _thoughts, finish) = collect(
        client
            .chat_stream(req, Default::default())
            .await
            .expect("haiku-4-5 must accept a continuation request"),
    )
    .await;
    (text, finish)
}

/// Anthropic, ≤4.5 generation, through the app's wire (stage 2): the base
/// continuation, one with extended thinking requested (the wire drops it —
/// sent as-is, the API answers `400 prefill is incompatible`), and one whose
/// partial ends in whitespace (the wire right-trims its copy — sent as-is,
/// the API rejects trailing whitespace). The first run (mid-word cut)
/// measured a U+00AD soft hyphen at the seam; the word-boundary cut is what
/// the feature actually sends.
#[tokio::test]
#[ignore = "requires MINDFORK_ANTHROPIC_KEY (live Anthropic API)"]
async fn anthropic_haiku_continues_a_trailing_assistant() {
    let Some(client) = anthropic_client("claude-haiku-4-5") else {
        eprintln!("skip: MINDFORK_ANTHROPIC_KEY not set");
        return;
    };
    for (label, partial, thinking) in [
        ("base", PARTIAL, false),
        ("thinking suppressed by the wire", PARTIAL, true),
        (
            "trailing whitespace trimmed by the wire",
            PARTIAL_TRAILING,
            false,
        ),
    ] {
        let (text, finish) = anthropic_continue(&client, partial, thinking).await;
        let outcome = analyze(&text, partial);
        println!(
            "anthropic haiku-4-5 [{label}]: finish={finish:?} outcome={outcome:?}\nraw={text:?}"
        );
        let (seam, cont) = continuation(&outcome)
            .unwrap_or_else(|| panic!("[{label}] did not continue the tail: {text:?}"));
        println!("seam bytes at the cut: {seam:?}");
        assert!(
            answers_the_question(cont),
            "[{label}] did not resume with the answer: {cont:?} (raw {text:?})"
        );
    }
}

/// Anthropic, current generation (4.6+/5-family): prefill is documented as
/// removed — the request must fail, which is what gates the capability table.
/// First run measured the exact wording: "This model does not support
/// assistant message prefill. The conversation must end with a user message."
#[tokio::test]
#[ignore = "requires MINDFORK_ANTHROPIC_KEY (live Anthropic API)"]
async fn anthropic_current_model_rejects_prefill() {
    let model =
        std::env::var("MINDFORK_ANTHROPIC_MODEL").unwrap_or_else(|_| "claude-opus-4-8".into());
    let Some(client) = anthropic_client(&model) else {
        eprintln!("skip: MINDFORK_ANTHROPIC_KEY not set");
        return;
    };
    let req = ChatRequest {
        continue_final: false,
        system: None,
        messages: fixture(PARTIAL),
        sampling: deterministic(),
        tools: vec![],
    };
    match client.chat_stream(req, Default::default()).await {
        Err(err) => {
            let msg = format!("{err:#}");
            println!("anthropic {model}: rejected as documented: {msg}");
            assert!(msg.contains("400"), "expected a 400 rejection, got: {msg}");
        }
        Ok(stream) => {
            let (text, _thoughts, finish) = collect(stream).await;
            panic!(
                "anthropic {model} accepted a prefill the docs say is removed — finish={finish:?} outcome={:?} text={text:?}",
                analyze(&text, PARTIAL)
            );
        }
    }
}

/// Gemini, through the app's wire (stage 2 — the flag adds nothing on this
/// wire; the trailing `model` turn is the whole mechanism, and the measured
/// configuration is deliberately the unmodified one). First run (mid-word
/// cut) measured a `"\n"` injected at the seam.
#[tokio::test]
#[ignore = "requires MINDFORK_GEMINI_KEY (live Gemini API)"]
async fn gemini_continues_a_trailing_model_turn() {
    let Some(key) = std::env::var("MINDFORK_GEMINI_KEY").ok() else {
        eprintln!("skip: MINDFORK_GEMINI_KEY not set");
        return;
    };
    let model =
        std::env::var("MINDFORK_GEMINI_MODEL").unwrap_or_else(|_| "gemini-2.5-flash".into());
    let client = GeminiClient::new(
        "https://generativelanguage.googleapis.com/v1beta",
        key,
        model.clone(),
    );
    let req = ChatRequest {
        continue_final: true,
        system: None,
        messages: fixture(PARTIAL),
        sampling: deterministic(),
        tools: vec![],
    };
    let (text, _thoughts, finish) = collect(
        client
            .chat_stream(req, Default::default())
            .await
            .expect("gemini must accept a trailing model turn"),
    )
    .await;
    let outcome = analyze(&text, PARTIAL);
    println!("gemini {model}: finish={finish:?} outcome={outcome:?}\nraw={text:?}");
    let (seam, cont) = continuation(&outcome)
        .unwrap_or_else(|| panic!("gemini did not continue the tail: {text:?}"));
    println!("seam bytes at the cut: {seam:?}");
    assert!(
        answers_the_question(cont),
        "gemini did not resume with the answer: {cont:?} (raw {text:?})"
    );
}

/// Grok: xAI documents role order freedom and nothing about continuation —
/// this arm only records which behaviour the API actually has. The first run
/// (256 tokens, default reasoning) came back with empty text and `Stop`;
/// this run gives reasoning room and prints the thoughts channel too.
#[tokio::test]
#[ignore = "requires MINDFORK_GROK_KEY (live xAI API)"]
async fn grok_trailing_assistant_behaviour_is_recorded() {
    let Some(key) = std::env::var("MINDFORK_GROK_KEY").ok() else {
        eprintln!("skip: MINDFORK_GROK_KEY not set");
        return;
    };
    let model = std::env::var("MINDFORK_GROK_MODEL").unwrap_or_else(|_| "grok-4.5".into());
    let client = OpenAiClient::new(crate::shared::config::CloudProvider::Grok.chat_base_url())
        .with_api_key(Some(key))
        .with_model(Some(model.clone()))
        .with_effort_none_omitted(true);
    let req = ChatRequest {
        continue_final: false,
        system: None,
        messages: fixture(PARTIAL),
        sampling: SamplingConfig {
            max_tokens: Some(2048),
            temperature: Some(0.0),
            ..Default::default()
        },
        tools: vec![],
    };
    let (text, thoughts, finish) = collect(
        client
            .chat_stream(req, Default::default())
            .await
            .expect("grok must accept the request shape (roles in any order)"),
    )
    .await;
    println!(
        "grok {model} (recorded, not asserted): finish={finish:?} outcome={:?}\nraw={text:?}\nthoughts={thoughts:?}",
        analyze(&text, PARTIAL)
    );
}

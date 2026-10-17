//! Stage-0 live probe for the prompt-cache fields of every wire —
//! docs/research/cached-tokens.md §7.
//!
//! The clients drop every cache figure a provider reports, so the probe reads
//! the raw stream: each wire's arm (a `cache_probe_tests.rs` beside its client)
//! builds the body with the client's own request builder, and this module sends
//! it, walks the SSE events and prints every usage-bearing object as it came.
//! What the research needs from it, per row: which field names arrive, whether
//! a miss is a `0` or an absent key, and whether the prompt count includes the
//! cached part. It asserts transport only — a hit is the provider's to give.
//!
//! One request, sent [`RUNS`] times in a row: the first is cold, the rest should
//! read the cache. A fresh nonce leads the system message on every probe run, so
//! a cache left over from an earlier run cannot answer the first request
//! (docs/research/prompt-caching.md §3.2).
//!
//! Run every arm whose variables are set:
//! `cargo test cache_probe -- --ignored --nocapture --test-threads=1`.

use std::collections::BTreeMap;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value;

use crate::entities::sampling::SamplingConfig;
use crate::shared::api::contract::{ApiMessage, ChatRequest};

/// Requests per probe: one cold, the rest warm.
pub(crate) const RUNS: usize = 3;

/// A hung provider must fail the arm loudly, not sit forever (lessons §2).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);

/// Paragraphs of the stable prefix. About 8000 tokens by the bytes/4 rule —
/// above every documented minimum (4096 on Haiku 4.5 and Gemini 3.x).
const PARAGRAPHS: usize = 160;

/// The probe's request: a nonce, then a long stable system message, then one
/// short question. Built once per probe and sent unchanged [`RUNS`] times.
pub(crate) fn probe_request() -> ChatRequest {
    let mut system = format!(
        "Probe run {}. You are a terse archivist. The archive follows.\n\n",
        uuid::Uuid::new_v4()
    );
    for i in 0..PARAGRAPHS {
        system.push_str(&format!(
            "Entry {i}. In the year {year} the keeper of lamp {lamp} logged the tide at \
             {hour:02}:{minute:02}, repainted railing {rail} in {colour}, and noted that the \
             {ship} left the harbour carrying {cargo} crates bound for the {port} quay.\n",
            year = 1700 + i,
            lamp = i % 7,
            hour = (i * 5) % 24,
            minute = (i * 13) % 60,
            rail = i * 3 + 1,
            colour = ["grey", "white", "ochre", "green", "blue"][i % 5],
            ship = ["Merrow", "Skylark", "Tern", "Gannet"][i % 4],
            cargo = 10 + (i * 17) % 90,
            port = ["north", "east", "salt", "old"][i % 4],
        ));
    }
    ChatRequest {
        system: Some(system),
        messages: vec![ApiMessage::user("Answer with the single word: ready.")],
        sampling: SamplingConfig {
            // Room for a reasoning model's default thinking: a small cap starves
            // it (lessons §3).
            max_tokens: Some(1024),
            ..SamplingConfig::default()
        },
        ..ChatRequest::default()
    }
}

/// The models an arm probes: the comma-separated list in `var`, or `default`.
pub(crate) fn models(var: &str, default: &str) -> Vec<String> {
    std::env::var(var)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
        .split(',')
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .collect()
}

/// Probes every model in turn — `send(model, &req)` builds one request — and
/// fails at the end if any of them did, so one model a provider no longer
/// serves does not hide the rest of the table.
pub(crate) async fn probe_models(
    wire: &str,
    models: &[String],
    send: impl Fn(&str, &ChatRequest) -> reqwest::RequestBuilder,
) {
    let mut failed = Vec::new();
    for model in models {
        let label = format!("{wire} {model}");
        if let Err(e) = probe(&label, |req| send(model, req)).await {
            println!("{e}");
            failed.push(e);
        }
    }
    assert!(
        failed.is_empty(),
        "{} probe(s) failed: {failed:#?}",
        failed.len()
    );
}

/// Sends `send(&req)` [`RUNS`] times and prints, per run, the last value of
/// every usage-bearing object the stream carried. A transport failure or a
/// stream with no usage at all is an error — the two outcomes that would make
/// the run say nothing.
async fn probe(
    label: &str,
    send: impl Fn(&ChatRequest) -> reqwest::RequestBuilder,
) -> Result<(), String> {
    let req = probe_request();
    println!(
        "== {label}: system {} bytes, {RUNS} runs",
        req.system.as_deref().map_or(0, str::len)
    );
    for run in 1..=RUNS {
        let seen = tokio::time::timeout(REQUEST_TIMEOUT, read_usage(send(&req)))
            .await
            .map_err(|_| format!("{label} run {run}: no answer in {REQUEST_TIMEOUT:?}"))?
            .map_err(|e| format!("{label} run {run}: {e}"))?;
        if seen.is_empty() {
            return Err(format!("{label} run {run}: the stream carried no usage"));
        }
        for (path, value) in &seen {
            println!("{label} run {run} {path}: {value}");
        }
    }
    Ok(())
}

/// One request's stream: the last value under each usage path, keyed by the
/// event type (where the wire names one) and the path.
async fn read_usage(rb: reqwest::RequestBuilder) -> Result<BTreeMap<String, Value>, String> {
    use eventsource_stream::Eventsource;
    let response = rb.send().await.map_err(|e| format!("send: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "HTTP {status}: {}",
            body.chars().take(600).collect::<String>()
        ));
    }
    let mut seen = BTreeMap::new();
    let mut events = response.bytes_stream().eventsource();
    while let Some(event) = events.next().await {
        let event = event.map_err(|e| format!("stream: {e}"))?;
        if event.data.trim() == "[DONE]" {
            break;
        }
        let Ok(json) = serde_json::from_str::<Value>(&event.data) else {
            continue;
        };
        for (path, value) in usage_paths(&json) {
            let kind = json.get("type").and_then(Value::as_str).unwrap_or("chunk");
            seen.insert(format!("{kind} {path}"), value.clone());
        }
    }
    Ok(seen)
}

/// Every place a wire puts its token counts: the OpenAI-compatible chunk and
/// Anthropic's `message_delta` (`usage`), llama.cpp's `timings`, Anthropic's
/// `message_start` (`message.usage`), Responses' terminal events
/// (`response.usage`) and Gemini (`usageMetadata`).
fn usage_paths(json: &Value) -> Vec<(&'static str, &Value)> {
    const PATHS: [(&str, &[&str]); 5] = [
        ("usage", &["usage"]),
        ("timings", &["timings"]),
        ("message.usage", &["message", "usage"]),
        ("response.usage", &["response", "usage"]),
        ("usageMetadata", &["usageMetadata"]),
    ];
    PATHS
        .iter()
        .filter_map(|(name, keys)| {
            let value = keys.iter().try_fold(json, |v, k| v.get(*k))?;
            (!value.is_null()).then_some((*name, value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{probe_request, usage_paths};

    #[test]
    fn the_prefix_is_stable_and_only_the_nonce_differs() {
        let (a, b) = (probe_request(), probe_request());
        let (a, b) = (a.system.unwrap(), b.system.unwrap());
        assert_ne!(a, b, "each probe run carries its own nonce");
        let tail = |s: &str| s.split_once('\n').unwrap().1.to_string();
        assert_eq!(tail(&a), tail(&b));
        assert!(
            a.len() > 4 * 4096,
            "above every documented minimum: {}",
            a.len()
        );
    }

    #[test]
    fn every_wires_usage_is_found() {
        let found = |raw: &str| -> Vec<&'static str> {
            let json: serde_json::Value = serde_json::from_str(raw).unwrap();
            usage_paths(&json).into_iter().map(|(p, _)| p).collect()
        };
        assert_eq!(
            found(r#"{"usage":{"prompt_tokens":1},"timings":{"cache_n":0}}"#),
            ["usage", "timings"]
        );
        assert_eq!(
            found(r#"{"type":"message_start","message":{"usage":{}}}"#),
            ["message.usage"]
        );
        assert_eq!(
            found(r#"{"type":"response.completed","response":{"usage":{}}}"#),
            ["response.usage"]
        );
        assert_eq!(
            found(r#"{"usageMetadata":{"promptTokenCount":1}}"#),
            ["usageMetadata"]
        );
        assert!(found(r#"{"usage":null,"choices":[]}"#).is_empty());
    }
}

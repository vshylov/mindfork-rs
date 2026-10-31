//! Gate G0's live smoke (docs/fabula.md §4, PR 2): the harbour scene played
//! against a live model through the engine adapter, written out as the
//! fixture the offline replay test reads, and replayed at once.
//!
//! `#[ignore]`, and skipped without `MINDFORK_ENGINE_URL`. On a gateway it
//! takes `MINDFORK_ENGINE_KEY` and `MINDFORK_ENGINE_MODEL` like every smoke
//! (`shared::api::live_client`). The fixture is written to
//! `MINDFORK_FABULA_FIXTURE_OUT`, or to `target/fabula/harbour_scene.json`;
//! copying it over `src/features/story/fixtures/harbour_scene.json` is
//! re-recording the fixture. `MINDFORK_FABULA_NOTE` goes into the fixture as
//! it is: how the route was pinned, say.
//!
//! `cargo test scene_live -- --ignored --nocapture`

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use crate::entities::sampling::SamplingConfig;
use crate::features::story::scene::harbour_script;
use crate::features::story::scene::tests::{leaks, record, replay};
use crate::shared::api::live_client;
use crate::shared::api::retry::RetryBackend;

/// A hung server fails the smoke instead of holding it (docs/lessons.md §2).
const BOUND: Duration = Duration::from_secs(900);

#[tokio::test]
#[ignore = "needs a live model: MINDFORK_ENGINE_URL (+ MINDFORK_ENGINE_KEY, MINDFORK_ENGINE_MODEL)"]
async fn scene_live_records_the_harbour_fixture() {
    let Some(client) = live_client("MINDFORK_ENGINE_URL", "MINDFORK_ENGINE_KEY") else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let model = std::env::var("MINDFORK_ENGINE_MODEL").unwrap_or_else(|_| "unnamed".to_owned());
    let url = std::env::var("MINDFORK_ENGINE_URL").unwrap_or_default();
    // Characters run without thinking (spec §8, §10); the cap leaves room
    // for a model that thinks anyway.
    let sampling = SamplingConfig {
        temperature: Some(0.8),
        thinking: Some(false),
        max_tokens: Some(4096),
        ..SamplingConfig::default()
    };
    let recorded = json!({
        "model": model,
        "endpoint": url,
        "date": chrono::Utc::now().format("%Y-%m-%d").to_string(),
        "mechanism": "fallback",
        "note": std::env::var("MINDFORK_FABULA_NOTE").ok(),
    });
    let fixture = tokio::time::timeout(
        BOUND,
        record(
            RetryBackend::wrap(Arc::new(client)),
            &model,
            sampling,
            &harbour_script(),
            recorded,
        ),
    )
    .await
    .expect("the scene finished within its bound")
    .expect("the scene was played");

    let (mut tokens_in, mut tokens_out, mut cost) = (0, 0, 0.0);
    for call in &fixture.calls {
        let response = call.response.as_ref();
        eprintln!(
            "{} #{}: {} in={:?} out={:?} {:?} ms, served by {}",
            call.address,
            call.attempt,
            call.status,
            call.input_tokens,
            call.output_tokens,
            call.latency_ms,
            response
                .and_then(|r| r["served_by"].as_str())
                .unwrap_or("-"),
        );
        tokens_in += call.input_tokens.unwrap_or_default();
        tokens_out += call.output_tokens.unwrap_or_default();
        cost += call.cost_usd.unwrap_or_default();
    }
    let first_time = fixture.calls.iter().filter(|c| c.attempt == 1).count();
    let repaired = fixture.calls.len() - first_time;
    eprintln!(
        "{} calls, {} extra attempts; {tokens_in} tokens in, {tokens_out} out, ${cost:.4}",
        first_time, repaired
    );
    for p in fixture.outcome["proposals"].as_array().unwrap() {
        eprintln!(
            "round {} {}: {} {} — {}",
            p["round"], p["character"], p["verdict"], p["proposal"]["action"], p["reason"]
        );
    }

    let out = std::env::var("MINDFORK_FABULA_FIXTURE_OUT").map_or_else(
        |_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/fabula/harbour_scene.json"),
        PathBuf::from,
    );
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let mut text = serde_json::to_string_pretty(&fixture).unwrap();
    text.push('\n');
    std::fs::write(&out, text).unwrap();
    eprintln!("fixture: {}", out.display());

    assert_eq!(leaks(&fixture), Vec::<String>::new());
    let (played, drift) = replay(&fixture).await;
    assert_eq!(played.unwrap().to_json(), fixture.outcome);
    assert_eq!(drift, Vec::new());
}

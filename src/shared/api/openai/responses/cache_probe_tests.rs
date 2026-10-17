//! The Responses arm of the cache probe (docs/research/cached-tokens.md §7):
//! the client's own body, sent raw so the `input_tokens_details` the client
//! drops can be read.
//!
//! `MINDFORK_OPENAI_KEY=… [MINDFORK_CACHE_PROBE_OPENAI_MODELS=a,b] cargo test
//! responses::cache_probe -- --ignored --nocapture --test-threads=1`

use super::wire::build_request;
use crate::shared::api::cache_probe_tests::{models, probe_models};

/// The newest model, the 5.6 generation (a write is billed there), and one
/// from before it, measured 2026-10-17.
const MODELS: &str = "gpt-6.1-sol,gpt-5.6-sol,gpt-5.4-mini";

#[tokio::test]
#[ignore = "requires MINDFORK_OPENAI_KEY"]
async fn cache_probe_responses_live() {
    let Ok(key) = std::env::var("MINDFORK_OPENAI_KEY") else {
        eprintln!("skip: MINDFORK_OPENAI_KEY not set");
        return;
    };
    let http = reqwest::Client::new();
    let models = models("MINDFORK_CACHE_PROBE_OPENAI_MODELS", MODELS);
    probe_models("responses", &models, |model, req| {
        http.post("https://api.openai.com/v1/responses")
            .bearer_auth(&key)
            .json(&build_request(req, model, true))
    })
    .await;
}

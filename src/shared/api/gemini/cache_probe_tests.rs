//! The Gemini arm of the cache probe (docs/research/cached-tokens.md §7): the
//! client's own body, sent raw so the `cachedContentTokenCount` the client
//! drops can be read — and whether a miss is a zero or an absent key.
//!
//! `MINDFORK_GEMINI_KEY=… [MINDFORK_CACHE_PROBE_GEMINI_MODELS=a,b] cargo test
//! gemini::cache_probe -- --ignored --nocapture --test-threads=1`

use super::wire::build_request;
use crate::shared::api::cache_probe_tests::{models, probe_models};

/// The newest Flash (a 4096-token minimum) and the 2.5 generation (2048).
const MODELS: &str = "gemini-3.8-flash,gemini-2.5-flash";

#[tokio::test]
#[ignore = "requires MINDFORK_GEMINI_KEY"]
async fn cache_probe_gemini_live() {
    let Ok(key) = std::env::var("MINDFORK_GEMINI_KEY") else {
        eprintln!("skip: MINDFORK_GEMINI_KEY not set");
        return;
    };
    let http = reqwest::Client::new();
    let models = models("MINDFORK_CACHE_PROBE_GEMINI_MODELS", MODELS);
    probe_models("gemini", &models, |model, req| {
        http.post(format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{model}:streamGenerateContent?alt=sse"
        ))
        .header("x-goog-api-key", &key)
        .json(&build_request(req, model))
    })
    .await;
}

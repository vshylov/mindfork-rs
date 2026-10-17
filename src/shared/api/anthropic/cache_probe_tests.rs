//! The Anthropic arm of the cache probe (docs/research/cached-tokens.md §7):
//! the client's own body, sent raw so the two cache fields the client drops
//! can be read. Nothing in the body asks Anthropic to cache, so the run says
//! whether the fields come as zeros or not at all.
//!
//! `MINDFORK_ANTHROPIC_KEY=… [MINDFORK_CACHE_PROBE_ANTHROPIC_MODELS=a,b] cargo
//! test anthropic::cache_probe -- --ignored --nocapture --test-threads=1`

use super::client::ANTHROPIC_VERSION;
use super::wire::build_request;
use crate::shared::api::cache_probe_tests::{models, probe_models};
use crate::shared::config::CloudProvider;

#[tokio::test]
#[ignore = "requires MINDFORK_ANTHROPIC_KEY"]
async fn cache_probe_anthropic_live() {
    let Ok(key) = std::env::var("MINDFORK_ANTHROPIC_KEY") else {
        eprintln!("skip: MINDFORK_ANTHROPIC_KEY not set");
        return;
    };
    let http = reqwest::Client::new();
    let url = format!("{}/v1/messages", CloudProvider::Claude.chat_base_url());
    let models = models("MINDFORK_CACHE_PROBE_ANTHROPIC_MODELS", "claude-haiku-4-5");
    probe_models("anthropic", &models, |model, req| {
        http.post(&url)
            .header("x-api-key", &key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&build_request(req, model, true))
    })
    .await;
}

//! The OpenAI-compatible arms of the cache probe
//! (docs/research/cached-tokens.md §7): the gateway mode's body, xAI's, and any
//! server at `MINDFORK_ENGINE_URL` (llama.cpp, Ollama, LM Studio, vLLM) — each
//! sent raw so the `prompt_tokens_details` and `timings` the client drops can
//! be read.
//!
//! `MINDFORK_OPENROUTER_KEY=… MINDFORK_XAI_KEY=… MINDFORK_ENGINE_URL=…/v1
//! cargo test openai::cache_probe -- --ignored --nocapture --test-threads=1`

use super::wire::build_chat_request;
use crate::shared::api::cache_probe_tests::{models, probe_models};
use crate::shared::config::CloudProvider;

/// One route per vendor behind the gateway, measured 2026-10-17: OpenAI's
/// automatic cache, Anthropic's explicit one (nothing asks it to cache) and
/// Gemini's implicit one.
const OPENROUTER_MODELS: &str =
    "openai/gpt-5.4-mini,anthropic/claude-haiku-4.5,google/gemini-3.8-flash";

fn key(var: &str) -> Option<String> {
    let key = std::env::var(var).ok().filter(|k| !k.trim().is_empty());
    if key.is_none() {
        eprintln!("skip: {var} not set");
    }
    key
}

#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY"]
async fn cache_probe_openrouter_live() {
    let Some(key) = key("MINDFORK_OPENROUTER_KEY") else {
        return;
    };
    let http = reqwest::Client::new();
    let url = format!(
        "{}/chat/completions",
        CloudProvider::OpenRouter.chat_base_url()
    );
    let models = models("MINDFORK_CACHE_PROBE_OPENROUTER_MODELS", OPENROUTER_MODELS);
    probe_models("openrouter", &models, |model, req| {
        http.post(&url)
            .bearer_auth(&key)
            .json(&build_chat_request(req, true, Some(model), false).for_gateway())
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MINDFORK_XAI_KEY"]
async fn cache_probe_xai_live() {
    let Some(key) = key("MINDFORK_XAI_KEY") else {
        return;
    };
    let http = reqwest::Client::new();
    let url = format!("{}/chat/completions", CloudProvider::Grok.chat_base_url());
    let models = models("MINDFORK_CACHE_PROBE_XAI_MODELS", "grok-4.7");
    probe_models("xai", &models, |model, req| {
        http.post(&url)
            .bearer_auth(&key)
            .json(&build_chat_request(req, true, Some(model), true))
    })
    .await;
}

/// Any OpenAI-compatible server; `MINDFORK_ENGINE_MODEL` names the model where
/// the server needs one (Ollama, LM Studio), `MINDFORK_ENGINE_KEY` a key.
#[tokio::test]
#[ignore = "requires a server at MINDFORK_ENGINE_URL"]
async fn cache_probe_external_live() {
    let Some(base) = key("MINDFORK_ENGINE_URL") else {
        return;
    };
    let http = reqwest::Client::new();
    let url = format!("{}/chat/completions", base.trim_end_matches('/'));
    let model = std::env::var("MINDFORK_ENGINE_MODEL").ok();
    let api_key = std::env::var("MINDFORK_ENGINE_KEY").ok();
    let label = model.clone().unwrap_or_else(|| "(the loaded model)".into());
    probe_models("external", &[label], |_, req| {
        let rb = http
            .post(&url)
            .json(&build_chat_request(req, true, model.as_deref(), false));
        match &api_key {
            Some(k) => rb.bearer_auth(k),
            None => rb,
        }
    })
    .await;
}

//! The route a story's live smokes run on (docs/history/fabula.md §4): a cloud on its
//! own wire, as the application's cloud modes reach it, or an OpenAI-compatible
//! engine. Skipped — every smoke returns at once — without its variables:
//!
//! - a cloud ([`Cloud`]): Claude (`MINDFORK_ANTHROPIC_KEY`,
//!   `MINDFORK_FABULA_CLAUDE_MODEL`), OpenAI's Responses (`MINDFORK_OPENAI_KEY`,
//!   `MINDFORK_FABULA_OPENAI_MODEL`), Gemini (`MINDFORK_GEMINI_KEY`,
//!   `MINDFORK_FABULA_GEMINI_MODEL`);
//! - the OpenAI-compatible engine: `MINDFORK_ENGINE_URL` (+ `_KEY`, `_MODEL`);
//!   `MINDFORK_ENGINE_DIALECT=openrouter` builds the gateway's client;
//! - `MINDFORK_FABULA_MECHANISM=fallback`: the schema's fallback mechanism
//!   alone, never constrained decoding.

use std::sync::Arc;

use crate::shared::api::contract::EngineBackend;
use crate::shared::api::retry::RetryBackend;
use crate::shared::api::{AnthropicClient, GeminiClient, ResponsesClient, live_client};

/// A cloud reached on its own wire, as the application's cloud modes reach it.
#[derive(Debug, Clone, Copy)]
enum Cloud {
    Claude,
    OpenAi,
    Gemini,
}

impl Cloud {
    const ALL: [Cloud; 3] = [Cloud::Claude, Cloud::OpenAi, Cloud::Gemini];

    /// The variables that name the key and the model.
    fn vars(self) -> (&'static str, &'static str) {
        match self {
            Cloud::Claude => ("MINDFORK_ANTHROPIC_KEY", "MINDFORK_FABULA_CLAUDE_MODEL"),
            Cloud::OpenAi => ("MINDFORK_OPENAI_KEY", "MINDFORK_FABULA_OPENAI_MODEL"),
            Cloud::Gemini => ("MINDFORK_GEMINI_KEY", "MINDFORK_FABULA_GEMINI_MODEL"),
        }
    }

    /// The backend and the model's name, or `None` when a variable is unset.
    fn backend(self) -> Option<(Arc<dyn EngineBackend>, String)> {
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
fn engine_route() -> Option<(Arc<dyn EngineBackend>, String)> {
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

/// The route the variables name: a cloud first, then the engine.
pub(crate) fn route() -> Option<(Arc<dyn EngineBackend>, String)> {
    Cloud::ALL
        .into_iter()
        .find_map(Cloud::backend)
        .or_else(engine_route)
}

/// Whether the variables ask for the fallback mechanism alone.
pub(crate) fn fallback_asked() -> bool {
    std::env::var("MINDFORK_FABULA_MECHANISM").is_ok_and(|v| v.trim() == "fallback")
}

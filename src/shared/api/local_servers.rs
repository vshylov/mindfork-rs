//! Ollama and LM Studio — the two local servers a user most often already runs
//! (docs/research/local-servers.md): what tells each apart, and what each says
//! about the models it holds. Parsing only; the requests are the client's.
//!
//! Every shape here has a field that is **required**, and that is the point.
//! LM Studio answers every path it does not serve with `200` and
//! `{"error":"Unexpected endpoint or method. (GET /props)"}` (measured on 1.1.7,
//! §2 of the research), so a status, or a body with every field optional,
//! would name LM Studio whatever was asked.

use serde::Deserialize;

/// Ollama's `GET /api/version` — `{"version":"0.35.1"}`.
#[derive(Debug, Deserialize)]
pub(crate) struct OllamaVersion {
    pub(crate) version: String,
}

/// LM Studio's `GET /api/v1/models`: every downloaded model, each with the
/// instances of it that are loaded.
#[derive(Debug, Deserialize)]
pub(crate) struct LmStudioModels {
    models: Vec<LmStudioModel>,
}

#[derive(Debug, Deserialize)]
struct LmStudioModel {
    /// What a request names the model by, before it is loaded.
    key: String,
    #[serde(default)]
    loaded_instances: Vec<LmStudioInstance>,
}

#[derive(Debug, Deserialize)]
struct LmStudioInstance {
    /// The instance's identifier — the model's key for the first instance,
    /// `<key>:2` and on for more, or the `--identifier` it was loaded with. A
    /// request naming it is routed to it.
    id: String,
    #[serde(default)]
    config: LmStudioInstanceConfig,
}

#[derive(Debug, Default, Deserialize)]
struct LmStudioInstanceConfig {
    context_length: Option<u32>,
}

impl LmStudioModels {
    /// The window of the instance a request naming `wanted` runs in: the
    /// instance that has that identifier, or else the only instance of the
    /// model with that key. Several instances and none of them named is
    /// silence, like a model that is not loaded (LM Studio loads it on the
    /// first request, so the orchestrator asks again after the first turn) and
    /// a window of zero.
    pub(crate) fn window_of(&self, wanted: &str) -> Option<u32> {
        let instances = || self.models.iter().flat_map(|m| &m.loaded_instances);
        let named = instances().find(|i| i.id == wanted).or_else(|| {
            let model = self.models.iter().find(|m| m.key == wanted)?;
            match model.loaded_instances.as_slice() {
                [only] => Some(only),
                _ => None,
            }
        })?;
        named.config.context_length.filter(|&n| n > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LM Studio's `/api/v1/models` as measured on 1.1.7 with one model loaded
    /// at 4096, trimmed to what is read and a little of what is not.
    const LOADED: &str = r#"{"models":[
        {"type":"llm","publisher":"bartowski","key":"google_gemma-4-e4b-it",
         "display_name":"Google Gemma 4 E4B Instruct","max_context_length":131072,
         "loaded_instances":[{"id":"google_gemma-4-e4b-it",
            "config":{"context_length":4096,"parallel":4,"flash_attention":true}}]},
        {"type":"embedding","key":"text-embedding-nomic-embed-text-v1.5",
         "loaded_instances":[]}]}"#;

    fn parse(body: &str) -> LmStudioModels {
        serde_json::from_str(body).expect("the listing parses")
    }

    #[test]
    fn the_configured_models_window() {
        assert_eq!(parse(LOADED).window_of("google_gemma-4-e4b-it"), Some(4096));
    }

    /// A second instance of the same model has an identifier of its own, and a
    /// request naming it runs there, not in the first.
    #[test]
    fn an_instance_named_wins() {
        const TWO: &str = r#"{"models":[{"key":"qwen","loaded_instances":[
            {"id":"qwen","config":{"context_length":8192}},
            {"id":"qwen:2","config":{"context_length":32768}}]}]}"#;
        assert_eq!(parse(TWO).window_of("qwen:2"), Some(32768));
        assert_eq!(parse(TWO).window_of("qwen"), Some(8192));
    }

    /// An instance loaded under an identifier of the user's own is still the
    /// model's only one, and a request naming the key runs in it.
    #[test]
    fn the_only_instance_of_the_key() {
        const ALIASED: &str = r#"{"models":[{"key":"qwen","loaded_instances":[
            {"id":"my-qwen","config":{"context_length":16384}}]}]}"#;
        assert_eq!(parse(ALIASED).window_of("qwen"), Some(16384));
        assert_eq!(parse(ALIASED).window_of("my-qwen"), Some(16384));
    }

    /// Silence, each way: a model that is not loaded, a model the server does
    /// not hold, a window of zero, two instances and neither named.
    #[test]
    fn silence_is_never_a_guess() {
        const COLD: &str = r#"{"models":[{"key":"gemma","loaded_instances":[]}]}"#;
        const ZERO: &str = r#"{"models":[{"key":"gemma","loaded_instances":[{"id":"gemma","config":{"context_length":0}}]}]}"#;
        const TWO: &str = r#"{"models":[{"key":"gemma","loaded_instances":[
            {"id":"a","config":{"context_length":8192}},
            {"id":"b","config":{"context_length":4096}}]}]}"#;
        assert_eq!(parse(COLD).window_of("gemma"), None);
        assert_eq!(parse(LOADED).window_of("absent"), None);
        assert_eq!(parse(ZERO).window_of("gemma"), None);
        assert_eq!(parse(TWO).window_of("gemma"), None);
    }

    /// LM Studio's answer to a path it does not serve is a `200` — and must not
    /// parse as any of the shapes here, or it would be taken for Ollama, or for
    /// itself when another server sent it.
    #[test]
    fn an_error_body_is_neither_server() {
        const ERROR: &str = r#"{"error":"Unexpected endpoint or method. (GET /api/version)"}"#;
        assert!(serde_json::from_str::<OllamaVersion>(ERROR).is_err());
        assert!(serde_json::from_str::<LmStudioModels>(ERROR).is_err());
        // Ollama's own answer to the version question.
        let v: OllamaVersion = serde_json::from_str(r#"{"version":"0.35.1"}"#).unwrap();
        assert_eq!(v.version, "0.35.1");
    }
}

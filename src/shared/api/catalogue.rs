//! The provider's model catalogue — the list the settings screen offers instead
//! of a hint that names models.
//!
//! Stage 4b of the public-release track
//! ([docs/research/model-picker.md](../../../docs/research/model-picker.md)):
//! D7 was "the hint names models that are one or two generations old", and the
//! answer chosen was not to rewrite the names but to fetch them. Four endpoint
//! shapes were measured (§2 of that document) and they differ in every part —
//! the path, the auth header, the key the list sits under, and above all in what
//! they say a model is *for*:
//!
//! | Provider | Route | Says what a model does |
//! |---|---|---|
//! | OpenAI, any OpenAI-compatible server | `GET {base}/models` | nothing at all (132 entries mixing chat, image, audio, embeddings) |
//! | Gemini | `GET {base}/models` (native) | `supportedGenerationMethods` |
//! | Anthropic | `GET {base}/v1/models` | the route itself: the Messages API has only chat models |
//! | xAI | `GET {base}/language-models` | the route itself: the language half of a catalogue that also holds image and video models |
//!
//! Hence [`ModelRole::Unstated`], which is **not** "it does nothing": where the
//! endpoint publishes no claim, none is invented, and every model it lists is
//! offered (fork F2, the user's decision of 2026-09-18).

use std::time::Duration;

use serde::Deserialize;

use super::anthropic::ANTHROPIC_VERSION;

/// How long a catalogue fetch may take in total.
///
/// A chat stream gets only a *connect* timeout ([`super::http::CONNECT_TIMEOUT`])
/// because it is legitimately silent for minutes; this is one small `GET` behind
/// a keypress, measured at 190–1300 ms (§2.6 of the research, the slowest being
/// OpenAI's 132-entry list). Twenty seconds is an order of magnitude of headroom
/// and still ends the "fetching…" line rather than leaving it forever.
const TOTAL_TIMEOUT: Duration = Duration::from_secs(20);

/// How many entries to ask for.
///
/// Not cosmetic: Gemini's default page is **50** and it lists **58**, so the
/// unpaged request silently drops eight models, and Anthropic's default `limit`
/// is 20. Both cap at 1000, and neither of the other two shapes pages at all.
const PAGE_SIZE: &str = "1000";

/// The catalogue shape an endpoint answers with — the table in the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogueShape {
    /// `{"data": [{"id": …}]}` — the OpenAI cloud, a `llama-server`, LM Studio,
    /// a gateway. The one shape that can arrive without a key.
    OpenAi,
    /// `{"models": [{"name": "models/…", "supportedGenerationMethods": […]}]}`.
    Gemini,
    /// `{"data": [{"id": …, "display_name": …}]}` behind `x-api-key`.
    Anthropic,
    /// `{"models": [{"id": …}]}` — xAI's language-only half of its catalogue.
    Xai,
}

/// What the endpoint said a model is for.
///
/// [`Self::Unstated`] is silence, not a denial: OpenAI publishes no capability
/// field at all, and neither does a `llama-server`. A model whose role is
/// unstated is offered for every slot, because refusing it would be our claim
/// about a catalogue that made none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelRole {
    /// The endpoint says this model answers chat requests.
    Chat,
    /// The endpoint says this model embeds text.
    Embedding,
    /// The endpoint says this model does something else (Gemini's video, music
    /// and live-only models).
    Other,
    /// The endpoint said nothing about it.
    Unstated,
}

/// Which settings row is being filled — it decides what the list is narrowed to.
///
/// The assistant and impersonation both send chat requests, so they share a
/// filter; managed mode is absent on purpose (its row is a GGUF path on this
/// machine, not a name any catalogue can offer — N1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSlot {
    /// The model the assistant answers with.
    Assistant,
    /// The model that writes in the user's voice (spec §11.6).
    Impersonation,
    /// The model that embeds notes and attachments.
    Embedder,
}

/// One entry of a provider's catalogue, reduced to what a picker needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogModel {
    /// Exactly what goes into the settings field. Verbatim as the endpoint
    /// publishes it — on a multi-model endpoint this string *is* the selector
    /// (N3) — except for Gemini's `models/` prefix, which the client appends
    /// itself (N2).
    pub id: String,
    /// The name the endpoint published for people to read, when it published
    /// one (`displayName`/`display_name`).
    pub display: Option<String>,
    /// What the endpoint said this model is for.
    pub role: ModelRole,
    /// The day the endpoint stops serving this model, when it publishes one
    /// (OpenAI's `shutdown_date`: 56 of its 132 entries carry one). The provider
    /// stating its own model is on the way out — the very staleness D7 is about,
    /// from the only party that knows.
    pub retiring: Option<String>,
}

/// Why there is no list. Every arm leaves the row exactly as it is today — a
/// text field — and says this much (N5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogueError {
    /// This provider needs a key and none is configured.
    NoKey,
    /// Nobody answered: no route, a refused connection, a timeout.
    Unreachable,
    /// The endpoint answered, and the answer was a refusal (a wrong key, a
    /// route it does not serve).
    Refused(u16),
    /// The endpoint answered with something that is not a catalogue.
    Unreadable,
    /// There is nothing to ask: this mode takes a file on this machine (managed),
    /// borrows another slot's engine (impersonation's `shared`), or has no
    /// address typed yet.
    NotConfigured,
}

impl CatalogueError {
    /// The localized line the settings row shows in place of the list.
    pub fn message_key(self) -> &'static str {
        match self {
            CatalogueError::NoKey => "ui.settings.models.err.no_key",
            CatalogueError::Unreachable => "ui.settings.models.err.unreachable",
            CatalogueError::Refused(_) => "ui.settings.models.err.refused",
            CatalogueError::Unreadable => "ui.settings.models.err.unreadable",
            CatalogueError::NotConfigured => "ui.settings.models.err.not_configured",
        }
    }
}

/// What one catalogue request answered: the entries, or why there are none.
///
/// `Arc` because the answer crosses the channel to the screen and is kept there
/// for the visit (N6); an empty `Ok` is an answer, not a failure.
pub type CatalogueAnswer = Result<std::sync::Arc<[CatalogModel]>, CatalogueError>;

/// Where to ask, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogueRequest {
    /// The shape the answer will have.
    pub shape: CatalogueShape,
    /// The base URL, no trailing slash — the provider's own, or the override the
    /// slot is configured with.
    pub base: String,
    /// The API key. `None` sends no auth header at all, which is a local
    /// `llama-server`'s normal case; a cloud slot without a key never gets here
    /// (the caller answers [`CatalogueError::NoKey`] without a request).
    pub key: Option<String>,
}

impl CatalogueRequest {
    /// The full URL, per the table in the module docs.
    fn url(&self) -> String {
        let base = self.base.trim_end_matches('/');
        match self.shape {
            // The page size is spelled into the URL rather than passed as a
            // query pair because `reqwest` is built here without the feature
            // that adds `query()` (Cargo.toml) — and both values are constants.
            CatalogueShape::OpenAi => format!("{base}/models"),
            CatalogueShape::Gemini => format!("{base}/models?pageSize={PAGE_SIZE}"),
            // The Anthropic base carries no version segment (the client appends
            // `/v1/messages` itself), so the catalogue appends its own.
            CatalogueShape::Anthropic => format!("{base}/v1/models?limit={PAGE_SIZE}"),
            CatalogueShape::Xai => format!("{base}/language-models"),
        }
    }
}

/// Asks the endpoint for its catalogue.
///
/// Fetch and parse are separate on purpose: [`parse`] is pure and is what the
/// unit tests pin against the bodies measured in §2 of the research, so the
/// shapes are covered without a network.
pub async fn fetch(req: &CatalogueRequest) -> Result<Vec<CatalogModel>, CatalogueError> {
    let http = reqwest::Client::builder()
        .connect_timeout(super::http::CONNECT_TIMEOUT)
        .timeout(TOTAL_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let url = req.url();
    let mut rb = http.get(&url);
    rb = match (req.shape, req.key.as_deref()) {
        (CatalogueShape::Gemini, Some(key)) => rb.header("x-goog-api-key", key),
        (CatalogueShape::Anthropic, Some(key)) => rb
            .header("x-api-key", key)
            .header("anthropic-version", ANTHROPIC_VERSION),
        (_, Some(key)) => rb.bearer_auth(key),
        // A local server with no key at all — the shape that needs none.
        (_, None) => rb,
    };
    let resp = match rb.send().await {
        Ok(r) => r,
        Err(err) => {
            tracing::debug!(%url, error = %err, "no answer from the model catalogue");
            return Err(CatalogueError::Unreachable);
        }
    };
    let status = resp.status();
    if !status.is_success() {
        tracing::debug!(%url, %status, "the model catalogue was refused");
        return Err(CatalogueError::Refused(status.as_u16()));
    }
    let body = resp.text().await.map_err(|err| {
        tracing::debug!(%url, error = %err, "the catalogue body could not be read");
        CatalogueError::Unreachable
    })?;
    parse(req.shape, &body)
}

/// Reads a catalogue body into entries, newest first where the endpoint
/// publishes a date.
///
/// Gemini is left in the endpoint's own order — it publishes no creation date,
/// and inventing one from a name would be exactly the guess this feature exists
/// to avoid.
pub fn parse(shape: CatalogueShape, body: &str) -> Result<Vec<CatalogModel>, CatalogueError> {
    let unreadable = |err: serde_json::Error| {
        tracing::debug!(error = %err, "the model catalogue did not parse");
        CatalogueError::Unreadable
    };
    let mut models: Vec<CatalogModel> = match shape {
        CatalogueShape::OpenAi => {
            let list: OpenAiList = serde_json::from_str(body).map_err(unreadable)?;
            let mut v: Vec<_> = list
                .data
                .into_iter()
                .filter(|e| !e.id.is_empty())
                .map(|e| {
                    (
                        e.created.unwrap_or(0),
                        CatalogModel {
                            id: e.id,
                            display: None,
                            role: ModelRole::Unstated,
                            retiring: e.shutdown_date.filter(|d| !d.is_empty()),
                        },
                    )
                })
                .collect();
            v.sort_by_key(|(created, _)| std::cmp::Reverse(*created));
            v.into_iter().map(|(_, m)| m).collect()
        }
        CatalogueShape::Gemini => {
            let list: GeminiList = serde_json::from_str(body).map_err(unreadable)?;
            // Google serves two lists: the native one asked for here, and the
            // OpenAI-compatible one under `data` — same 58 models, same
            // `models/` prefix, but no methods, so every role stays unstated
            // (§2.2). Reading both means a slot pointed at the compat path
            // still offers a list instead of a parse failure.
            let entries = if list.models.is_empty() {
                list.data
            } else {
                list.models
            };
            entries
                .into_iter()
                .filter_map(|e| {
                    // `models/gemini-2.5-pro` → `gemini-2.5-pro`: the client
                    // builds `{base}/models/{model}:streamGenerateContent`, so
                    // the prefix would be sent twice (N2).
                    let id = e
                        .name
                        .strip_prefix("models/")
                        .unwrap_or(&e.name)
                        .to_string();
                    (!id.is_empty()).then(|| CatalogModel {
                        id,
                        display: e.display_name.filter(|d| !d.is_empty()),
                        role: gemini_role(&e.supported_generation_methods),
                        retiring: None,
                    })
                })
                .collect()
        }
        CatalogueShape::Anthropic => {
            let list: AnthropicList = serde_json::from_str(body).map_err(unreadable)?;
            let mut v: Vec<_> = list
                .data
                .into_iter()
                .filter(|e| !e.id.is_empty())
                .map(|e| {
                    (
                        e.created_at.clone().unwrap_or_default(),
                        CatalogModel {
                            id: e.id,
                            display: e.display_name.filter(|d| !d.is_empty()),
                            // The Messages API serves chat and nothing else —
                            // Anthropic publishes no embedding or speech model,
                            // so the route itself is the claim (§2.3).
                            role: ModelRole::Chat,
                            retiring: None,
                        },
                    )
                })
                .collect();
            // RFC 3339, so lexicographic order is chronological order.
            v.sort_by_key(|(created_at, _)| std::cmp::Reverse(created_at.clone()));
            v.into_iter().map(|(_, m)| m).collect()
        }
        CatalogueShape::Xai => {
            let list: XaiList = serde_json::from_str(body).map_err(unreadable)?;
            let mut v: Vec<_> = list
                .models
                .into_iter()
                .filter(|e| !e.id.is_empty())
                .map(|e| {
                    (
                        e.created.unwrap_or(0),
                        CatalogModel {
                            id: e.id,
                            display: None,
                            // `/language-models` is the half of xAI's catalogue
                            // that answers chat; the image and video models live
                            // under `/models` only (§2.4).
                            role: ModelRole::Chat,
                            retiring: None,
                        },
                    )
                })
                .collect();
            v.sort_by_key(|(created, _)| std::cmp::Reverse(*created));
            v.into_iter().map(|(_, m)| m).collect()
        }
    };
    // A catalogue that lists the same id twice (an alias published as its own
    // entry) would offer the same line twice.
    models.dedup_by(|a, b| a.id == b.id);
    Ok(models)
}

/// What Gemini's `supportedGenerationMethods` says the model is for.
///
/// `generateContent` first: a chat model also lists `countTokens`, while an
/// embedding model lists `embedContent` and never `generateContent` (§2.2). The
/// filter is not perfect — the image and text-to-speech models generate content
/// too — but it is the endpoint's claim, not ours.
fn gemini_role(methods: &[String]) -> ModelRole {
    if methods.iter().any(|m| m == "generateContent") {
        ModelRole::Chat
    } else if methods.iter().any(|m| m == "embedContent") {
        ModelRole::Embedding
    } else if methods.is_empty() {
        ModelRole::Unstated
    } else {
        ModelRole::Other
    }
}

/// Narrows a catalogue to what the slot can use (fork F2/F3).
///
/// [`ModelRole::Unstated`] passes every filter — see [`ModelRole`].
pub fn for_slot(models: Vec<CatalogModel>, slot: ModelSlot) -> Vec<CatalogModel> {
    models
        .into_iter()
        .filter(|m| match (slot, m.role) {
            (_, ModelRole::Unstated) => true,
            (ModelSlot::Assistant | ModelSlot::Impersonation, role) => role == ModelRole::Chat,
            (ModelSlot::Embedder, role) => role == ModelRole::Embedding,
        })
        .collect()
}

#[derive(Deserialize)]
struct OpenAiList {
    #[serde(default)]
    data: Vec<OpenAiEntry>,
}

#[derive(Deserialize)]
struct OpenAiEntry {
    id: String,
    #[serde(default)]
    created: Option<i64>,
    #[serde(default)]
    shutdown_date: Option<String>,
}

#[derive(Deserialize)]
struct GeminiList {
    #[serde(default)]
    models: Vec<GeminiEntry>,
    /// The compat catalogue's key — see [`parse`].
    #[serde(default)]
    data: Vec<GeminiEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiEntry {
    /// `models/gemini-2.5-pro`. The compat list spells the same value `id`.
    #[serde(default, alias = "id")]
    name: String,
    #[serde(default, alias = "display_name")]
    display_name: Option<String>,
    #[serde(default)]
    supported_generation_methods: Vec<String>,
}

#[derive(Deserialize)]
struct AnthropicList {
    #[serde(default)]
    data: Vec<AnthropicEntry>,
}

#[derive(Deserialize)]
struct AnthropicEntry {
    id: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
}

#[derive(Deserialize)]
struct XaiList {
    #[serde(default)]
    models: Vec<XaiEntry>,
}

#[derive(Deserialize)]
struct XaiEntry {
    id: String,
    #[serde(default)]
    created: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from the body measured on 2026-09-18 (§2.1): no capability field
    /// anywhere, a `shutdown_date` on the ones going away, and the modalities
    /// mixed together.
    const OPENAI_BODY: &str = r#"{"object":"list","data":[
        {"id":"whisper-1","object":"model","created":1677532384,"owned_by":"openai-internal","shutdown_date":null},
        {"id":"gpt-4","object":"model","created":1687882411,"owned_by":"openai","shutdown_date":"2026-10-23"},
        {"id":"gpt-6-astra","object":"model","created":1779000000,"owned_by":"system","shutdown_date":null},
        {"id":"text-embedding-3-small","object":"model","created":1705948997,"owned_by":"system","shutdown_date":null}
    ]}"#;

    /// §2.2, with one model of each kind the catalogue actually holds.
    const GEMINI_BODY: &str = r#"{"models":[
        {"name":"models/gemini-2.5-flash","displayName":"Gemini 2.5 Flash","inputTokenLimit":1048576,
         "supportedGenerationMethods":["generateContent","countTokens","batchGenerateContent"]},
        {"name":"models/gemini-embedding-001","displayName":"Gemini Embedding 001",
         "supportedGenerationMethods":["embedContent","countTokens","asyncBatchEmbedContent"]},
        {"name":"models/veo-3.1-generate-preview","displayName":"Veo 3.1",
         "supportedGenerationMethods":["predictLongRunning"]}
    ]}"#;

    /// §2.3 — `has_more` and the capability tree left out, `created_at` kept
    /// because it decides the order.
    const ANTHROPIC_BODY: &str = r#"{"data":[
        {"type":"model","id":"claude-opus-4-8","display_name":"Claude Opus 4.8","created_at":"2026-02-05T00:00:00Z"},
        {"type":"model","id":"claude-opus-5","display_name":"Claude Opus 5","created_at":"2026-07-24T00:00:00Z"}
    ],"has_more":false}"#;

    /// §2.4 — note the `models` key and the aliases the picker does not list.
    const XAI_BODY: &str = r#"{"models":[
        {"id":"grok-4.5","aliases":["grok-4.5-latest"],"created":1770000000,
         "input_modalities":["text","image"],"output_modalities":["text"]},
        {"id":"grok-4.6","aliases":[],"created":1780000000,
         "input_modalities":["text","image"],"output_modalities":["text"]}
    ]}"#;

    #[test]
    fn openai_lists_everything_newest_first_and_claims_nothing() {
        let models = parse(CatalogueShape::OpenAi, OPENAI_BODY).expect("a catalogue");
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            [
                "gpt-6-astra",
                "text-embedding-3-small",
                "gpt-4",
                "whisper-1"
            ],
            "newest first, by the `created` the endpoint publishes"
        );
        assert!(
            models.iter().all(|m| m.role == ModelRole::Unstated),
            "OpenAI publishes no capability field, so no role may be claimed"
        );
        let gpt4 = models.iter().find(|m| m.id == "gpt-4").expect("gpt-4");
        assert_eq!(gpt4.retiring.as_deref(), Some("2026-10-23"));
        assert!(
            models.iter().filter(|m| m.retiring.is_some()).count() == 1,
            "a null shutdown_date is not a date"
        );
    }

    #[test]
    fn an_unstated_role_is_offered_for_every_slot() {
        let models = parse(CatalogueShape::OpenAi, OPENAI_BODY).expect("a catalogue");
        for slot in [
            ModelSlot::Assistant,
            ModelSlot::Impersonation,
            ModelSlot::Embedder,
        ] {
            assert_eq!(
                for_slot(models.clone(), slot).len(),
                4,
                "silence narrows nothing — {slot:?} sees the whole list"
            );
        }
    }

    #[test]
    fn gemini_strips_the_prefix_and_reads_the_published_methods() {
        let models = parse(CatalogueShape::Gemini, GEMINI_BODY).expect("a catalogue");
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            [
                "gemini-2.5-flash",
                "gemini-embedding-001",
                "veo-3.1-generate-preview"
            ],
            "the `models/` prefix is the client's to add, and the order is the endpoint's"
        );
        assert_eq!(models[0].role, ModelRole::Chat);
        assert_eq!(models[1].role, ModelRole::Embedding);
        assert_eq!(models[2].role, ModelRole::Other);
        assert_eq!(models[0].display.as_deref(), Some("Gemini 2.5 Flash"));
    }

    #[test]
    fn gemini_narrows_per_slot() {
        let models = parse(CatalogueShape::Gemini, GEMINI_BODY).expect("a catalogue");
        assert_eq!(
            for_slot(models.clone(), ModelSlot::Assistant)
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            ["gemini-2.5-flash"]
        );
        assert_eq!(
            for_slot(models, ModelSlot::Embedder)
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            ["gemini-embedding-001"],
            "the embedder gets what the endpoint says embeds, not what the name suggests"
        );
    }

    #[test]
    fn anthropic_is_chat_newest_first() {
        let models = parse(CatalogueShape::Anthropic, ANTHROPIC_BODY).expect("a catalogue");
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["claude-opus-5", "claude-opus-4-8"]
        );
        assert!(models.iter().all(|m| m.role == ModelRole::Chat));
        assert_eq!(models[0].display.as_deref(), Some("Claude Opus 5"));
        assert!(
            for_slot(models, ModelSlot::Embedder).is_empty(),
            "Anthropic publishes no embedding model, and an empty list says so"
        );
    }

    #[test]
    fn xai_reads_the_models_key_not_data() {
        let models = parse(CatalogueShape::Xai, XAI_BODY).expect("a catalogue");
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["grok-4.6", "grok-4.5"],
            "the language half of the catalogue, newest first"
        );
        assert!(models.iter().all(|m| m.role == ModelRole::Chat));
    }

    /// A `llama-server`'s own answer, measured on b11009 (§2.5): one entry whose
    /// id is the `-m` path. It is written into the field verbatim — on a
    /// multi-model endpoint that string is the selector.
    #[test]
    fn a_llama_server_lists_its_path_and_it_is_kept_as_is() {
        let body = r#"{"object":"list","data":[
            {"id":"D:\\LLM\\GGUF\\gemma-4-31B_q4_0-it.gguf","object":"model","created":1789691057,"owned_by":"llamacpp"}
        ],"models":[{"name":"D:\\LLM\\GGUF\\gemma-4-31B_q4_0-it.gguf","capabilities":["completion","multimodal"]}]}"#;
        let models = parse(CatalogueShape::OpenAi, body).expect("a catalogue");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, r"D:\LLM\GGUF\gemma-4-31B_q4_0-it.gguf");
        assert_eq!(models[0].role, ModelRole::Unstated);
    }

    #[test]
    fn a_body_that_is_not_a_catalogue_is_unreadable() {
        assert_eq!(
            parse(CatalogueShape::OpenAi, "<html>404</html>"),
            Err(CatalogueError::Unreadable)
        );
        // An answer of the right kind with no list is an empty catalogue, not a
        // failure: a router with nothing loaded answers exactly this.
        assert_eq!(parse(CatalogueShape::OpenAi, "{}"), Ok(vec![]));
        assert_eq!(parse(CatalogueShape::Gemini, "{}"), Ok(vec![]));
    }

    #[test]
    fn every_shape_has_its_own_route() {
        let req = |shape| CatalogueRequest {
            shape,
            base: "https://example.test/v1/".to_string(),
            key: None,
        };
        assert_eq!(
            req(CatalogueShape::OpenAi).url(),
            "https://example.test/v1/models"
        );
        assert_eq!(
            req(CatalogueShape::Gemini).url(),
            "https://example.test/v1/models?pageSize=1000",
            "unpaged, Gemini answers 50 of its 58 models"
        );
        assert_eq!(
            req(CatalogueShape::Xai).url(),
            "https://example.test/v1/language-models"
        );
        assert_eq!(
            CatalogueRequest {
                shape: CatalogueShape::Anthropic,
                base: "https://api.anthropic.com".to_string(),
                key: None,
            }
            .url(),
            "https://api.anthropic.com/v1/models?limit=1000",
            "the Anthropic base carries no version segment"
        );
    }
}

/// Live smokes against the real catalogues — the measurements of §2 of
/// docs/research/model-picker.md, turned into assertions that still hold.
///
/// Each is skipped (with a line saying so) when its key is not set, like every
/// other cloud smoke in this crate. Run:
/// `cargo test catalogue_e2e_live -- --ignored --nocapture --test-threads=1`.
#[cfg(test)]
mod live_smoke {
    use super::*;

    /// Prints what a catalogue answered — the record the journal entry quotes.
    fn report(name: &str, models: &[CatalogModel]) {
        let chat = models.iter().filter(|m| m.role == ModelRole::Chat).count();
        let embed = models
            .iter()
            .filter(|m| m.role == ModelRole::Embedding)
            .count();
        let unstated = models
            .iter()
            .filter(|m| m.role == ModelRole::Unstated)
            .count();
        let retiring = models.iter().filter(|m| m.retiring.is_some()).count();
        eprintln!(
            "{name}: {} entries — chat {chat}, embedding {embed}, unstated {unstated}, retiring {retiring}",
            models.len()
        );
        for m in models.iter().take(5) {
            eprintln!("    {} {:?} {:?}", m.id, m.display, m.role);
        }
    }

    async fn fetched(shape: CatalogueShape, base: &str, key: Option<String>) -> Vec<CatalogModel> {
        super::fetch(&CatalogueRequest {
            shape,
            base: base.to_string(),
            key,
        })
        .await
        .expect("the catalogue answered")
    }

    #[tokio::test]
    #[ignore = "requires MINDFORK_OPENAI_KEY (live OpenAI API)"]
    async fn the_openai_catalogue_e2e_live() {
        let Ok(key) = std::env::var("MINDFORK_OPENAI_KEY") else {
            eprintln!("skip: MINDFORK_OPENAI_KEY not set");
            return;
        };
        let models = fetched(
            CatalogueShape::OpenAi,
            "https://api.openai.com/v1",
            Some(key),
        )
        .await;
        report("openai", &models);
        assert!(models.len() > 10, "a catalogue of {}", models.len());
        assert!(models.iter().all(|m| !m.id.is_empty()));
        assert!(
            models.iter().all(|m| m.role == ModelRole::Unstated),
            "OpenAI publishes no capability field — no role may be claimed"
        );
        assert!(
            models.iter().any(|m| m.retiring.is_some()),
            "the shutdown dates are what the endpoint does publish"
        );
        assert_eq!(
            for_slot(models.clone(), ModelSlot::Assistant).len(),
            models.len(),
            "silence narrows nothing"
        );
    }

    #[tokio::test]
    #[ignore = "requires MINDFORK_GEMINI_KEY (live Gemini API)"]
    async fn the_gemini_catalogue_e2e_live() {
        let Ok(key) = std::env::var("MINDFORK_GEMINI_KEY") else {
            eprintln!("skip: MINDFORK_GEMINI_KEY not set");
            return;
        };
        let models = fetched(
            CatalogueShape::Gemini,
            "https://generativelanguage.googleapis.com/v1beta",
            Some(key),
        )
        .await;
        report("gemini", &models);
        assert!(models.len() > 10);
        assert!(
            models.iter().all(|m| !m.id.starts_with("models/")),
            "the prefix belongs to the URL the client builds, not to the field"
        );
        let chat = for_slot(models.clone(), ModelSlot::Assistant);
        let embed = for_slot(models.clone(), ModelSlot::Embedder);
        assert!(!chat.is_empty() && !embed.is_empty());
        assert!(
            chat.len() < models.len(),
            "supportedGenerationMethods is what narrows the list, and it does"
        );
        assert!(
            embed.iter().all(|m| !chat.iter().any(|c| c.id == m.id)),
            "a model the endpoint calls an embedder is not offered for chat"
        );
    }

    #[tokio::test]
    #[ignore = "requires MINDFORK_ANTHROPIC_KEY (live Anthropic API)"]
    async fn the_anthropic_catalogue_e2e_live() {
        let Ok(key) = std::env::var("MINDFORK_ANTHROPIC_KEY") else {
            eprintln!("skip: MINDFORK_ANTHROPIC_KEY not set");
            return;
        };
        let models = fetched(
            CatalogueShape::Anthropic,
            "https://api.anthropic.com",
            Some(key),
        )
        .await;
        report("anthropic", &models);
        assert!(!models.is_empty());
        assert!(models.iter().all(|m| m.role == ModelRole::Chat));
        assert!(
            models.iter().all(|m| m.display.is_some()),
            "Anthropic publishes a display name for every model"
        );
        assert!(
            for_slot(models, ModelSlot::Embedder).is_empty(),
            "Anthropic has no embedding model, and the empty list says so"
        );
    }

    #[tokio::test]
    #[ignore = "requires MINDFORK_GROK_KEY (live xAI API)"]
    async fn the_xai_catalogue_e2e_live() {
        let Ok(key) = std::env::var("MINDFORK_GROK_KEY") else {
            eprintln!("skip: MINDFORK_GROK_KEY not set");
            return;
        };
        let models = fetched(CatalogueShape::Xai, "https://api.x.ai/v1", Some(key)).await;
        report("xai", &models);
        assert!(!models.is_empty(), "the `models` key, not `data`");
        assert!(models.iter().all(|m| m.role == ModelRole::Chat));
        assert!(
            models.iter().all(|m| !m.id.contains("imagine")),
            "the image and video models live under /models, which is why this route is asked"
        );
    }

    /// The local arm: whatever `MINDFORK_ENGINE_URL` points at (a `llama-server`,
    /// LM Studio, a gateway). An id that is a `-m` path is the normal answer and
    /// is kept verbatim — it is what a multi-model endpoint routes on.
    #[tokio::test]
    #[ignore = "requires MINDFORK_ENGINE_URL (a live OpenAI-compatible server)"]
    async fn the_local_server_catalogue_e2e_live() {
        let Ok(base) = std::env::var("MINDFORK_ENGINE_URL") else {
            eprintln!("skip: MINDFORK_ENGINE_URL not set");
            return;
        };
        let key = std::env::var("MINDFORK_ENGINE_KEY")
            .ok()
            .filter(|k| !k.is_empty());
        let models = fetched(CatalogueShape::OpenAi, &base, key).await;
        report("external", &models);
        assert!(!models.is_empty(), "a server with a model loaded lists it");
        assert!(models.iter().all(|m| !m.id.is_empty()));
        assert!(
            models.iter().all(|m| m.role == ModelRole::Unstated),
            "a llama.cpp publishes no role in the OpenAI list"
        );
    }
}

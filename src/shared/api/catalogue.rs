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
//! | OpenRouter | `GET {base}/models/user`, `/models`, `/embeddings/models`, `…?output_modalities=speech`, `…?input_modalities=video` | `architecture.output_modalities` and `input_modalities` — and the window, the price, the parameters and the voices besides |
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
    /// The OpenRouter gateway's list of models that answer with **text**:
    /// `/models/user` with a key — narrowed by the account's own privacy and
    /// provider settings — and the public `/models` without one. The one cloud
    /// catalogue that needs no key, and the one that says per model what the
    /// window is, what a token costs and which parameters are taken
    /// (docs/research/openrouter-mode.md §3.2).
    OpenRouter,
    /// The same gateway's `/embeddings/models`.
    OpenRouterEmbeddings,
    /// The same gateway's models that answer with **speech**: the list of
    /// [`Self::OpenRouter`] narrowed by the gateway's own filter,
    /// `?output_modalities=speech` — which the account's list takes as the
    /// public one does (measured: 21 entries either way,
    /// docs/research/openrouter-mode.md §13).
    OpenRouterSpeech,
    /// The same gateway's models that **take video**. Without a key it is the
    /// public list under the gateway's own filter, `?input_modalities=video` —
    /// 85 entries. With one it is the account's whole list: measured, that list
    /// does **not** take this filter — 461 entries came back, not 85 — so the
    /// entries are narrowed here, by the `input_modalities` each of them
    /// publishes ([`ModelFacts::video`], docs/research/openrouter-mode.md §14).
    OpenRouterVideo,
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
    /// The endpoint says this model answers with speech.
    Speech,
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
    /// The model that reads messages aloud. One speech mode has a catalogue
    /// behind it — the gateway's — and its voice rows are filled from the same
    /// answer: a voice belongs to a model ([`CatalogModel::voices`]).
    Speech,
    /// The model that watches a video for `youtube_watch`. One provider of that
    /// slot has a catalogue — the gateway.
    Video,
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
    /// What else the endpoint published about the model. Empty for every
    /// catalogue that publishes names alone.
    pub facts: ModelFacts,
    /// The voices the endpoint says this model speaks in, in its order
    /// (`supported_voices`). Empty for a model that is not a speech model, and
    /// for one that lists none — four of the gateway's 21 speak without a
    /// voice being named.
    pub voices: Vec<String>,
}

impl CatalogModel {
    /// A voice as an entry of a list: what the voice rows' picker offers.
    /// The name is all there is to a voice — the catalogue says nothing else
    /// about one.
    pub fn voice(name: &str) -> Self {
        Self {
            id: name.to_string(),
            display: None,
            role: ModelRole::Speech,
            retiring: None,
            facts: ModelFacts::default(),
            voices: Vec::new(),
        }
    }
}

/// What a catalogue says about a model besides its name — each field the
/// **endpoint's** claim, and `None` where it made none. Integers throughout, so
/// that an entry compares exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModelFacts {
    /// The context window, in tokens.
    pub context_length: Option<u32>,
    /// What a million **prompt** tokens cost, in millionths of a dollar.
    pub prompt_price: Option<u64>,
    /// What a million **completion** tokens cost, in millionths of a dollar.
    pub completion_price: Option<u64>,
    /// Whether the model takes tool schemas: `Some(false)` — the entry lists
    /// its parameters and `tools` is not among them. This application is driven
    /// by tools, so a model without them chats and does nothing else.
    pub tools: Option<bool>,
    /// Whether the model takes video, from the inputs the entry lists:
    /// `Some(false)` — it lists its inputs and video is not among them; `None` —
    /// it lists none. What a row is narrowed by, not what a row shows.
    pub video: Option<bool>,
}

impl ModelFacts {
    /// Whether the endpoint said anything a row has to show. What an entry
    /// takes as input is not shown — it is what the list was narrowed by.
    pub fn is_empty(&self) -> bool {
        let shown = ModelFacts {
            video: None,
            ..*self
        };
        shown == ModelFacts::default()
    }
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
    /// Whether the request names this application to the OpenRouter gateway,
    /// in the two headers every other request to it carries
    /// ([`super::openai::attributed`]). The gateway's switch, and nobody
    /// else's: `false` for every other provider.
    pub attribution: bool,
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
            // The account's own list where there is an account to ask about.
            CatalogueShape::OpenRouter if self.key.is_some() => format!("{base}/models/user"),
            CatalogueShape::OpenRouter => format!("{base}/models"),
            CatalogueShape::OpenRouterEmbeddings => format!("{base}/embeddings/models"),
            CatalogueShape::OpenRouterSpeech if self.key.is_some() => {
                format!("{base}/models/user?output_modalities=speech")
            }
            CatalogueShape::OpenRouterSpeech => format!("{base}/models?output_modalities=speech"),
            CatalogueShape::OpenRouterVideo if self.key.is_some() => format!("{base}/models/user"),
            CatalogueShape::OpenRouterVideo => format!("{base}/models?input_modalities=video"),
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
    if req.attribution {
        rb = super::openai::attributed(rb);
    }
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
                            facts: ModelFacts::default(),
                            voices: Vec::new(),
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
                        facts: ModelFacts::default(),
                        voices: Vec::new(),
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
                            facts: ModelFacts::default(),
                            voices: Vec::new(),
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
                            facts: ModelFacts::default(),
                            voices: Vec::new(),
                        },
                    )
                })
                .collect();
            v.sort_by_key(|(created, _)| std::cmp::Reverse(*created));
            v.into_iter().map(|(_, m)| m).collect()
        }
        CatalogueShape::OpenRouter
        | CatalogueShape::OpenRouterEmbeddings
        | CatalogueShape::OpenRouterSpeech
        | CatalogueShape::OpenRouterVideo => {
            let list: GatewayList = serde_json::from_str(body).map_err(unreadable)?;
            let mut v: Vec<_> = list
                .data
                .into_iter()
                // A `:batch` slug is the gateway's Batch API under a model's
                // name: asked to chat it answers `404 … cannot be used with the
                // chat/completions endpoint` (measured,
                // docs/research/openrouter-mode.md §2.4, D2). The refusal is the
                // endpoint's own, so leaving these out narrows on its claim.
                .filter(|e| !e.id.is_empty() && !e.id.ends_with(":batch"))
                .map(|e| (e.created.unwrap_or(0), e.into_model()))
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

/// Words in a model's **name** that suggest it does something other than chat.
///
/// Read the guarantee before adding one: this list **only orders** a list the
/// endpoint said nothing about, and never removes anything from it. That is the
/// whole difference from the filter rejected in fork F2(c) — a name we guess
/// wrong costs a scroll, not a model that cannot be chosen. Measured against
/// OpenAI's 132 entries (2026-09-18): 49 sink, 83 stay, and none of the 83 is
/// anything but a chat or completion model.
const ANOTHER_JOB_IN_A_NAME: [&str; 17] = [
    "tts",
    "whisper",
    "image",
    "sora",
    "embedding",
    "realtime",
    "transcribe",
    "moderation",
    "audio",
    "dall-e",
    "live",
    "speech",
    "video",
    "imagine",
    "veo",
    "lyria",
    "music",
];

/// The family measured to take a YouTube link as a video — what the video
/// row's list opens with. An alias (`~google/gemini-flash-latest`) is of the
/// family too.
const VIDEO_LINK_IN_A_NAME: [&str; 1] = ["google/gemini"];

/// The mirror of [`ANOTHER_JOB_IN_A_NAME`] for the embedder's row.
const EMBEDDING_IN_A_NAME: [&str; 1] = ["embed"];

/// Whether any of `needles` appears in the id, case-insensitively.
fn name_hints(id: &str, needles: &[&str]) -> bool {
    let id = id.to_ascii_lowercase();
    needles.iter().any(|n| id.contains(n))
}

/// Where an entry sits in the list: `0` — what this slot is for, `1` — the rest.
///
/// A **guess about a name**, and only ever consulted for a model whose role the
/// endpoint left [`ModelRole::Unstated`]: where it said what a model does, its
/// word is what sorts.
fn rank(m: &CatalogModel, slot: ModelSlot) -> u8 {
    match slot {
        ModelSlot::Assistant | ModelSlot::Impersonation => {
            u8::from(m.role == ModelRole::Unstated && name_hints(&m.id, &ANOTHER_JOB_IN_A_NAME))
        }
        ModelSlot::Embedder => u8::from(
            !(m.role == ModelRole::Embedding
                || (m.role == ModelRole::Unstated && name_hints(&m.id, &EMBEDDING_IN_A_NAME))),
        ),
        // The one catalogue this slot reads says what every entry is for, so
        // there is no name to guess from: the endpoint's order stands.
        ModelSlot::Speech => 0,
        // Gemini first. Every entry listed claims video, and the list is
        // narrowed by that claim alone; that the Gemini family is the one that
        // takes a YouTube **link** — the others go to download it, and refuse —
        // is this project's measurement, which may order a list and never
        // narrow it (docs/research/openrouter-mode.md §4.5, fork F7).
        ModelSlot::Video => u8::from(!name_hints(&m.id, &VIDEO_LINK_IN_A_NAME)),
    }
}

/// Narrows a catalogue to what the slot can use, and puts first what the slot is
/// for (forks F2/F3).
///
/// [`ModelRole::Unstated`] passes every filter — see [`ModelRole`] — which is
/// what left OpenAI's list opening on this month's image models, since they are
/// its newest entries (reported from a live run, 2026-09-18). So the list is
/// **ordered** by [`rank`] and nothing is removed: the sort is stable, so the
/// endpoint's own order — newest first where it publishes a date — survives
/// inside each group.
pub fn for_slot(models: Vec<CatalogModel>, slot: ModelSlot) -> Vec<CatalogModel> {
    let mut kept: Vec<CatalogModel> = models
        .into_iter()
        .filter(|m| match (slot, m.role) {
            (_, ModelRole::Unstated) => true,
            (ModelSlot::Assistant | ModelSlot::Impersonation, role) => role == ModelRole::Chat,
            (ModelSlot::Embedder, role) => role == ModelRole::Embedding,
            (ModelSlot::Speech, role) => role == ModelRole::Speech,
            // What answers with text and takes video — or has not said what it
            // takes, which is silence.
            (ModelSlot::Video, role) => role == ModelRole::Chat && m.facts.video != Some(false),
        })
        .collect();
    kept.sort_by_key(|m| rank(m, slot));
    kept
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
struct GatewayList {
    #[serde(default)]
    data: Vec<GatewayEntry>,
}

/// One entry of the gateway's catalogue. Everything but the id is optional and
/// read leniently: a field that changes its shape costs that one fact, never the
/// list.
#[derive(Deserialize)]
struct GatewayEntry {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    created: Option<i64>,
    #[serde(default)]
    context_length: Option<serde_json::Value>,
    #[serde(default)]
    pricing: Option<serde_json::Value>,
    #[serde(default)]
    architecture: Option<serde_json::Value>,
    #[serde(default)]
    supported_parameters: Option<Vec<String>>,
    #[serde(default)]
    supported_voices: Option<serde_json::Value>,
    #[serde(default)]
    expiration_date: Option<String>,
}

impl GatewayEntry {
    fn into_model(self) -> CatalogModel {
        let outputs: Vec<&str> = self
            .architecture
            .as_ref()
            .and_then(|a| a.get("output_modalities"))
            .and_then(|o| o.as_array())
            .map(|list| list.iter().filter_map(|m| m.as_str()).collect())
            .unwrap_or_default();
        let role = if outputs.is_empty() {
            ModelRole::Unstated
        } else if outputs.contains(&"text") {
            ModelRole::Chat
        } else if outputs.contains(&"embeddings") {
            ModelRole::Embedding
        } else if outputs.contains(&"speech") {
            ModelRole::Speech
        } else {
            ModelRole::Other
        };
        // `null` for a model that lists none. Read leniently, like the rest:
        // an entry that is not a name costs that entry, never the list.
        let voices = self
            .supported_voices
            .as_ref()
            .and_then(|v| v.as_array())
            .map(|list| {
                list.iter()
                    .filter_map(|v| v.as_str())
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let price = |key: &str| {
            self.pricing
                .as_ref()
                .and_then(|p| p.get(key))
                .and_then(price_per_million)
        };
        let facts = ModelFacts {
            context_length: self
                .context_length
                .as_ref()
                .and_then(serde_json::Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .filter(|&n| n > 0),
            prompt_price: price("prompt"),
            completion_price: price("completion"),
            tools: self
                .supported_parameters
                .as_ref()
                .filter(|p| !p.is_empty())
                .map(|p| p.iter().any(|x| x == "tools")),
            video: self
                .architecture
                .as_ref()
                .and_then(|a| a.get("input_modalities"))
                .and_then(|i| i.as_array())
                .filter(|list| !list.is_empty())
                .map(|list| list.iter().any(|m| m.as_str() == Some("video"))),
        };
        CatalogModel {
            id: self.id,
            display: self.name.filter(|n| !n.is_empty()),
            role,
            retiring: self.expiration_date.filter(|d| !d.is_empty()),
            facts,
            voices,
        }
    }
}

/// The gateway's price of **one token**, in dollars — published as a string
/// (`"0.000001"`) — as the price of a million, in millionths of a dollar.
/// `None` for anything that is not a number, and for a negative one: the
/// gateway's router models publish `-1` to say "it depends".
fn price_per_million(per_token: &serde_json::Value) -> Option<u64> {
    let dollars = match per_token {
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok()?,
        serde_json::Value::Number(n) => n.as_f64()?,
        _ => return None,
    };
    (dollars.is_finite() && dollars >= 0.0).then(|| (dollars * 1e12).round() as u64)
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

    /// The ordering the live run asked for: a name that looks like another job
    /// goes **down**, never out — the whole difference from the filter F2(c)
    /// rejected.
    #[test]
    fn a_silent_catalogue_is_ordered_not_filtered() {
        let models = parse(CatalogueShape::OpenAi, OPENAI_BODY).expect("a catalogue");
        let chat = for_slot(models.clone(), ModelSlot::Assistant);
        assert_eq!(chat.len(), models.len(), "nothing is hidden, ever");
        assert_eq!(
            chat.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            [
                "gpt-6-astra",
                "gpt-4",
                "text-embedding-3-small",
                "whisper-1"
            ],
            "the chat models first, each group still newest-first"
        );
        let embed = for_slot(models.clone(), ModelSlot::Embedder);
        assert_eq!(embed.len(), models.len());
        assert_eq!(
            embed[0].id, "text-embedding-3-small",
            "the embedder's row wants the mirror of the same guess"
        );
        assert!(
            chat.iter().any(|m| m.id == "whisper-1"),
            "a model we guessed about is still there to be chosen"
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
            attribution: false,
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
                attribution: false,
            }
            .url(),
            "https://api.anthropic.com/v1/models?limit=1000",
            "the Anthropic base carries no version segment"
        );
    }

    // ---------- the OpenRouter gateway (docs/research/openrouter-mode.md §3.2) ----------

    /// Entries in the gateway's own shape, measured 2026-09-29 and cut to the
    /// keys this client reads, in an order that is **not** by date: a `:batch`
    /// twin, a router that prices itself `-1`, a free model without tools, a
    /// model on its way out, one that answers with images, and two whose fields
    /// changed shape.
    const GATEWAY_BODY: &str = r#"{"data":[
        {"id":"anthropic/claude-haiku-4.5","name":"Anthropic: Claude Haiku 4.5","created":1760547638,
         "context_length":200000,
         "architecture":{"input_modalities":["text","image","file"],"output_modalities":["text"]},
         "pricing":{"prompt":"0.000001","completion":"0.000005"},
         "supported_parameters":["max_tokens","temperature","tools"],"expiration_date":null},
        {"id":"anthropic/claude-haiku-4.5:batch","name":"Anthropic: Claude Haiku 4.5 (batch)","created":1760547638,
         "context_length":200000,"architecture":{"output_modalities":["text"]},
         "pricing":{"prompt":"0.0000005","completion":"0.0000025"},"supported_parameters":["tools"]},
        {"id":"openrouter/auto","name":"Auto Router","created":1699401600,"context_length":2000000,
         "architecture":{"output_modalities":["text"]},
         "pricing":{"prompt":"-1","completion":"-1"},"supported_parameters":["tools"]},
        {"id":"google/gemini-3.5-flash","name":"Google: Gemini 3.5 Flash","created":1779000000,
         "context_length":1048576,
         "architecture":{"input_modalities":["text","image","video"],"output_modalities":["text"]},
         "pricing":{"prompt":"0.0000003","completion":"0.0000025"},
         "supported_parameters":["reasoning","max_tokens","tools"],"expiration_date":""},
        {"id":"meta-llama/llama-3.3-70b-instruct:free","name":"Meta: Llama 3.3 70B (free)","created":1733506137,
         "context_length":65536,"architecture":{"output_modalities":["text"]},
         "pricing":{"prompt":"0","completion":"0"},"supported_parameters":["max_tokens","temperature"]},
        {"id":"qwen/qwen3.6-27b","name":"Qwen: Qwen3.6 27B","created":1775000000,"context_length":131072,
         "architecture":{"output_modalities":["text"]},
         "pricing":{"prompt":"0.00000012","completion":"0.00000048"},"supported_parameters":["tools"]},
        {"id":"openai/gpt-4-turbo","name":"OpenAI: GPT-4 Turbo","created":1712620800,
         "context_length":128000,"architecture":{"output_modalities":["text"]},
         "pricing":{"prompt":0.00001,"completion":0.00003},"expiration_date":"2026-11-01"},
        {"id":"openai/gpt-image-2","name":"","created":1770000000,"context_length":32000,
         "architecture":{"output_modalities":["image"]},"pricing":{"prompt":"0.000005"}},
        {"id":"odd/shapes","created":1700000000,"context_length":"large",
         "architecture":"text->text","pricing":{"prompt":"n/a","completion":null},
         "supported_parameters":[]},
        {"id":"odd/window","context_length":0,"pricing":"free"}
    ]}"#;

    /// `GET /embeddings/models`: the same shape, answering with embeddings.
    const GATEWAY_EMBEDDINGS_BODY: &str = r#"{"data":[
        {"id":"openai/text-embedding-3-small","name":"OpenAI: Text Embedding 3 Small","created":1706000000,
         "context_length":8191,"architecture":{"input_modalities":["text"],"output_modalities":["embeddings"]},
         "pricing":{"prompt":"0.00000002","completion":"0"}},
        {"id":"baai/bge-m3","name":"BAAI: bge-m3","created":1754000000,"context_length":8192,
         "architecture":{"input_modalities":["text"],"output_modalities":["embeddings"]},
         "pricing":{"prompt":"0.00000001","completion":"0"}}
    ]}"#;

    /// `GET /models?output_modalities=speech`, measured 2026-09-29 and cut to
    /// the keys this client reads: a model with voices, one that lists `null`,
    /// one whose list holds what is not a name, and — were the filter ever to
    /// let one through — a chat model.
    const GATEWAY_SPEECH_BODY: &str = r#"{"data":[
        {"id":"x-ai/grok-voice-tts-1.0","name":"xAI: Grok Voice TTS 1.0","created":1782000000,
         "context_length":15000,
         "architecture":{"modality":"text->speech","input_modalities":["text"],"output_modalities":["speech"]},
         "pricing":{"prompt":"0.000015","completion":"0"},"supported_parameters":[],
         "supported_voices":["eve","ara","rex","sal","leo"]},
        {"id":"fish-audio/s1","name":"Fish Audio: S1","created":1788000000,"context_length":0,
         "architecture":{"input_modalities":["text"],"output_modalities":["speech"]},
         "pricing":{"prompt":"0.000015","completion":"0"},"supported_parameters":[],
         "supported_voices":null},
        {"id":"odd/voices","created":1700000000,
         "architecture":{"output_modalities":["speech"]},
         "supported_voices":["Kore",""," Puck ",7,null,{"name":"Zephyr"}]},
        {"id":"odd/voice-list","created":1600000000,
         "architecture":{"output_modalities":["speech"]},"supported_voices":"alloy"},
        {"id":"google/gemini-3.5-flash","created":1779000000,
         "architecture":{"output_modalities":["text"]},"supported_voices":null}
    ]}"#;

    fn gateway() -> Vec<CatalogModel> {
        parse(CatalogueShape::OpenRouter, GATEWAY_BODY).expect("a catalogue")
    }

    /// The speech list: what answers with speech is a speech model by the
    /// gateway's word, and is offered for the speech row and no other; its
    /// voices are the ones it lists, in its order. A list that holds what is
    /// not a name costs those entries, and one that is not a list — all of
    /// them; neither costs the model its row.
    #[test]
    fn the_gateway_says_which_models_speak_and_in_which_voices() {
        let listed = parse(CatalogueShape::OpenRouterSpeech, GATEWAY_SPEECH_BODY).expect("a list");
        let voices = |id: &str| named(&listed, id).voices.join(" ");
        assert_eq!(voices("x-ai/grok-voice-tts-1.0"), "eve ara rex sal leo");
        assert_eq!(voices("fish-audio/s1"), "");
        assert_eq!(voices("odd/voices"), "Kore Puck");
        assert_eq!(voices("odd/voice-list"), "");
        assert_eq!(
            named(&listed, "x-ai/grok-voice-tts-1.0").role,
            ModelRole::Speech
        );

        let offered = for_slot(listed.clone(), ModelSlot::Speech);
        assert_eq!(
            ids(&offered),
            [
                "fish-audio/s1",
                "x-ai/grok-voice-tts-1.0",
                "odd/voices",
                "odd/voice-list"
            ],
            "newest first, and the chat model is not a speech model"
        );
        for slot in [
            ModelSlot::Assistant,
            ModelSlot::Impersonation,
            ModelSlot::Embedder,
        ] {
            let others = for_slot(listed.clone(), slot);
            assert!(
                others.iter().all(|m| m.role != ModelRole::Speech),
                "{slot:?}: {:?}",
                ids(&others)
            );
        }
        // Silence narrows nothing, here as everywhere.
        let unstated = for_slot(gateway(), ModelSlot::Speech);
        assert_eq!(ids(&unstated), ["odd/shapes", "odd/window"]);
        // A chat model lists no voices, whatever its entry holds.
        assert!(gateway().iter().all(|m| m.voices.is_empty()));
    }

    /// The video row's list: what answers with text and takes video, by the
    /// inputs each entry publishes — so the account's list, which the gateway
    /// does not narrow, is narrowed here, to what the public one is narrowed to
    /// by the gateway. An entry that lists no inputs is silence, and stays.
    /// Gemini opens the list, in the gateway's order; the rest follow in it.
    #[test]
    fn the_video_list_is_what_claims_video_with_gemini_first() {
        let takes = |id: &str| named(&gateway(), id).facts.video;
        assert_eq!(takes("google/gemini-3.5-flash"), Some(true));
        assert_eq!(takes("anthropic/claude-haiku-4.5"), Some(false));
        assert_eq!(takes("qwen/qwen3.6-27b"), None, "it lists no inputs");
        assert_eq!(takes("odd/shapes"), None, "not a list of inputs");

        let listed = parse(
            CatalogueShape::OpenRouterVideo,
            r#"{"data":[
            {"id":"qwen/qwen3.6-flash","created":5,"architecture":
                {"input_modalities":["text","image","video"],"output_modalities":["text"]}},
            {"id":"google/gemini-3.5-flash","created":4,"architecture":
                {"input_modalities":["text","video"],"output_modalities":["text"]}},
            {"id":"anthropic/claude-haiku-4.5","created":3,"architecture":
                {"input_modalities":["text","image"],"output_modalities":["text"]}},
            {"id":"~google/gemini-flash-latest","created":2,"architecture":
                {"input_modalities":["video","text"],"output_modalities":["text"]}},
            {"id":"vendor/says-nothing-of-inputs","created":1,"architecture":
                {"input_modalities":[],"output_modalities":["text"]}},
            {"id":"vendor/films","created":6,"architecture":
                {"input_modalities":["text","video"],"output_modalities":["video"]}},
            {"id":"google/gemini-3.5-flash:batch","created":7,"architecture":
                {"input_modalities":["text","video"],"output_modalities":["text"]}}
            ]}"#,
        )
        .expect("a list");
        assert_eq!(
            ids(&for_slot(listed.clone(), ModelSlot::Video)),
            [
                "google/gemini-3.5-flash",
                "~google/gemini-flash-latest",
                "qwen/qwen3.6-flash",
                "vendor/says-nothing-of-inputs"
            ]
        );
        // The chat row is not narrowed by what a model takes.
        assert_eq!(for_slot(listed, ModelSlot::Assistant).len(), 5);
    }

    /// What an entry takes as input is what a list is narrowed by, not what a
    /// row shows: an entry that said this and nothing else has nothing to show,
    /// and keeps the name a row without facts is drawn with.
    #[test]
    fn what_a_model_takes_is_not_a_fact_a_row_shows() {
        let only_inputs = ModelFacts {
            video: Some(true),
            ..ModelFacts::default()
        };
        assert!(only_inputs.is_empty());
        let windowed = ModelFacts {
            context_length: Some(8192),
            ..only_inputs
        };
        assert!(!windowed.is_empty());
        assert!(ModelFacts::default().is_empty());
    }

    /// A voice is an entry of a list like a model is — the voice rows' picker
    /// offers them — and its name is all the row holds.
    #[test]
    fn a_voice_is_an_entry_with_a_name_and_nothing_else() {
        let voice = CatalogModel::voice("Kore");
        assert_eq!(voice.id, "Kore");
        assert_eq!(voice.role, ModelRole::Speech);
        assert!(voice.facts.is_empty() && voice.voices.is_empty());
        assert_eq!((voice.display, voice.retiring), (None, None));
    }

    fn ids(models: &[CatalogModel]) -> Vec<&str> {
        models.iter().map(|m| m.id.as_str()).collect()
    }

    fn named<'a>(models: &'a [CatalogModel], id: &str) -> &'a CatalogModel {
        let found = models.iter().find(|m| m.id == id);
        found.unwrap_or_else(|| panic!("{id} is not listed in {:?}", ids(models)))
    }

    /// Defect D2: every `:batch` slug is in the list and answers a chat with a
    /// `404`, so the picker leaves them out — and only them: a `:free` variant
    /// chats, and an entry whose fields changed shape costs its facts, never
    /// its row. Newest first, by the `created` the gateway publishes; an entry
    /// without one sorts last.
    #[test]
    fn the_gateway_lists_everything_but_batch_newest_first() {
        let newest_first = "google/gemini-3.5-flash qwen/qwen3.6-27b openai/gpt-image-2 \
            anthropic/claude-haiku-4.5 meta-llama/llama-3.3-70b-instruct:free \
            openai/gpt-4-turbo odd/shapes openrouter/auto odd/window";
        assert_eq!(
            ids(&gateway()),
            newest_first.split_whitespace().collect::<Vec<_>>()
        );
    }

    /// Unlike every catalogue before it, this one says what a model is **for**
    /// — by what it answers with — so the role is the gateway's claim and the
    /// list narrows on it (fork F7). Where it said nothing readable the role is
    /// unstated, which is silence and narrows nothing.
    #[test]
    fn the_gateway_says_what_a_model_is_for_by_what_it_answers_with() {
        let models = gateway();
        let role = |id: &str| named(&models, id).role;
        assert_eq!(role("anthropic/claude-haiku-4.5"), ModelRole::Chat);
        assert_eq!(role("openai/gpt-image-2"), ModelRole::Other);
        assert_eq!(role("odd/shapes"), ModelRole::Unstated);
        assert_eq!(role("odd/window"), ModelRole::Unstated);

        let embedders = parse(
            CatalogueShape::OpenRouterEmbeddings,
            GATEWAY_EMBEDDINGS_BODY,
        )
        .expect("a list");
        assert_eq!(
            ids(&embedders),
            ["baai/bge-m3", "openai/text-embedding-3-small"]
        );
        assert!(embedders.iter().all(|m| m.role == ModelRole::Embedding));

        let chat = for_slot(models.clone(), ModelSlot::Assistant);
        assert!(
            !ids(&chat).contains(&"openai/gpt-image-2") && chat.len() == models.len() - 1,
            "what answers with images alone is not offered for chat: {:?}",
            ids(&chat)
        );
        assert_eq!(chat, for_slot(models.clone(), ModelSlot::Impersonation));
        assert_eq!(
            ids(&for_slot(models, ModelSlot::Embedder)),
            ["odd/shapes", "odd/window"],
            "a chat model is not offered as an embedder; silence still is"
        );
        assert_eq!(
            for_slot(embedders.clone(), ModelSlot::Embedder),
            embedders,
            "the embedder's row gets the embedding list whole, in its order"
        );
        assert!(for_slot(embedders, ModelSlot::Assistant).is_empty());
    }

    /// Defect D3 was a row with an id and nothing else, from a response that
    /// carries the window, the price and the parameters. The price arrives as
    /// dollars **per token** in a string and is kept as millionths of a dollar
    /// per million tokens — an integer, so a row compares exactly, and a
    /// rounded one: twelve cents, read as a float and scaled, falls a hair
    /// short of 120000. Columns: the id, the window, the two prices, tools,
    /// why (`-` — the gateway made no claim).
    const GATEWAY_FACTS: &str = "
        anthropic/claude-haiku-4.5             200000  1000000  5000000  yes | $1 in, $5 out, as published
        google/gemini-3.5-flash                1048576 300000   2500000  yes | thirty cents in, two and a half dollars out
        qwen/qwen3.6-27b                       131072  120000   480000   yes | rounded, not cut: neither is 119999 or 479999
        meta-llama/llama-3.3-70b-instruct:free 65536   0        0        no  | free is a price, and no tools is a claim
        openai/gpt-4-turbo                     128000  10000000 30000000 -   | a price sent as a number; no parameter list
        openrouter/auto                        2000000 -        -        yes | a router prices itself -1: it depends
        openai/gpt-image-2                     32000   5000000  -        -   | half a price is still that half
        odd/shapes                             -       -        -        -   | unreadable fields, an empty parameter list
        odd/window                             -       -        -        -   | a window of zero is no window
    ";

    #[test]
    fn the_gateways_window_price_and_tools_are_read_as_published() {
        let models = gateway();
        let rows: Vec<&str> = GATEWAY_FACTS
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect();
        assert_eq!(rows.len(), models.len(), "every entry has its row");
        let mut wrong = Vec::new();
        for row in rows {
            let (columns, why) = row.split_once('|').unwrap();
            let c: Vec<&str> = columns.split_whitespace().collect();
            let got = named(&models, c[0]).facts;
            let want = ModelFacts {
                context_length: c[1].parse().ok(),
                prompt_price: c[2].parse().ok(),
                completion_price: c[3].parse().ok(),
                tools: Some(c[4] == "yes").filter(|_| c[4] != "-"),
                // What an entry takes as input has a test of its own.
                video: got.video,
            };
            if got != want {
                wrong.push(format!("{} — {}: {got:?}", c[0], why.trim()));
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    /// The name and the last day are the gateway's to publish, and an empty
    /// string or a `null` is not either of them — 24 of its entries carry an
    /// `expiration_date`, the rest a `null`.
    #[test]
    fn the_gateways_name_and_last_day_are_kept_where_it_published_them() {
        let models = gateway();
        let said = |id: &str| {
            let m = named(&models, id);
            (m.display.as_deref(), m.retiring.as_deref())
        };
        assert_eq!(
            said("anthropic/claude-haiku-4.5"),
            (Some("Anthropic: Claude Haiku 4.5"), None)
        );
        assert_eq!(
            said("openai/gpt-4-turbo"),
            (Some("OpenAI: GPT-4 Turbo"), Some("2026-11-01"))
        );
        assert_eq!(
            said("google/gemini-3.5-flash").1,
            None,
            "an empty date is no date"
        );
        assert_eq!(said("openai/gpt-image-2").0, None, "an empty name is none");
        assert_eq!(said("odd/window"), (None, None));
    }

    /// Fork F7, the sources: the account's own list where there is an account
    /// to ask about — `/models/user` is narrowed by its privacy and provider
    /// settings — the public one where there is not, and the embedding models
    /// from a list of their own, key or no key.
    #[test]
    fn the_gateways_route_follows_the_key_and_the_slot() {
        let url = |shape, key: Option<&str>| {
            CatalogueRequest {
                shape,
                base: "https://openrouter.ai/api/v1/".to_string(),
                key: key.map(str::to_string),
                attribution: false,
            }
            .url()
        };
        assert_eq!(
            url(CatalogueShape::OpenRouter, Some("k")),
            "https://openrouter.ai/api/v1/models/user"
        );
        assert_eq!(
            url(CatalogueShape::OpenRouter, None),
            "https://openrouter.ai/api/v1/models"
        );
        for key in [None, Some("k")] {
            assert_eq!(
                url(CatalogueShape::OpenRouterEmbeddings, key),
                "https://openrouter.ai/api/v1/embeddings/models"
            );
        }
        // The models that take video: the public list under the gateway's
        // filter, and the account's list whole — which does not take that
        // filter, and is narrowed by what its entries say.
        assert_eq!(
            url(CatalogueShape::OpenRouterVideo, Some("k")),
            "https://openrouter.ai/api/v1/models/user"
        );
        assert_eq!(
            url(CatalogueShape::OpenRouterVideo, None),
            "https://openrouter.ai/api/v1/models?input_modalities=video"
        );
        // The speech models are the chat list under the gateway's own filter,
        // so the key moves them to the account's list as it moves the chat's.
        assert_eq!(
            url(CatalogueShape::OpenRouterSpeech, Some("k")),
            "https://openrouter.ai/api/v1/models/user?output_modalities=speech"
        );
        assert_eq!(
            url(CatalogueShape::OpenRouterSpeech, None),
            "https://openrouter.ai/api/v1/models?output_modalities=speech"
        );
        // No other shape asks a different route for having a key.
        assert_eq!(
            url(CatalogueShape::OpenAi, Some("k")),
            url(CatalogueShape::OpenAi, None)
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
            attribution: false,
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
        let chat = for_slot(models.clone(), ModelSlot::Assistant);
        assert_eq!(chat.len(), models.len(), "silence narrows nothing");
        eprintln!(
            "openai, ordered for chat: {:?}",
            chat.iter()
                .take(5)
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            !name_hints(&chat[0].id, &ANOTHER_JOB_IN_A_NAME),
            "the list opens on a chat model, not on this month's image one: {}",
            chat[0].id
        );
        let embed = for_slot(models, ModelSlot::Embedder);
        eprintln!("openai, ordered for the embedder: {}", embed[0].id);
        assert!(
            name_hints(&embed[0].id, &EMBEDDING_IN_A_NAME),
            "and the embedder's row opens on one that embeds: {}",
            embed[0].id
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

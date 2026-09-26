//! The inference/embeddings server supervisor for the orchestrator: (re)launches a
//! managed process or connects to an external one per [`EngineSettings`]/
//! [`EmbedSettings`] settings. Hidden behind the [`ServerSupervisor`] trait for a mock
//! in tests — changing the model in settings restarts the server (spec §11.6, DoD M8).
//!
//! Lives in `app`: it's composition glue that knows both about `shared/config`
//! (settings) and about `shared/api` (the engine/process launch) — both lower in FSD.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::shared::api::llama_args::ManagedRole;
use crate::shared::api::managed::ChildExit;
use crate::shared::api::{
    AnthropicClient, Embedder, EngineBackend, GeminiClient, ManagedConfig, OpenAiClient,
    ResponsesClient, ServerHandle, UnavailableEmbedder, retry, wait_until_ready,
};
use crate::shared::config::{
    CloudProvider, EmbedSettings, EngineSettings, ImpersonationEngineSettings, ImpersonationMode,
    ManagedEmbedSettings, ManagedSettings, ServerMode,
};
use crate::shared::i18n::Locale;
use crate::shared::paths::Paths;
use crate::shared::server::ServerStatus;

/// A generous readiness timeout for the managed server: loading the model can take
/// minutes.
pub(crate) const MANAGED_READY_TIMEOUT: Duration = Duration::from_secs(600);
/// A short readiness timeout for the external server (it should already be up).
const EXTERNAL_READY_TIMEOUT: Duration = Duration::from_secs(15);

/// Health re-check cadence while the server looks healthy. Deliberately slow: the
/// poll buys little here, since a failure would be reported by the next real request
/// anyway (docs/server-health-monitoring.md, F2).
const HEALTHY_POLL: Duration = Duration::from_secs(60);
/// Cadence while the server is down — or while a failure streak is pending. Here the
/// poll *is* the recovery mechanism: until it succeeds, generation stays blocked.
const RECHECK_POLL: Duration = Duration::from_secs(5);
/// Consecutive failed probes before a healthy server is declared unavailable.
const FAILURES_TO_UNHEALTHY: u32 = 3;

/// The result of setting up the chat server: the engine, a handle to the process
/// (managed), and status.
pub struct ChatSetup {
    pub backend: Option<Arc<dyn EngineBackend>>,
    /// The owner of the child process (managed). `None` — external/not configured.
    pub handle: Option<ServerHandle>,
    pub status: ServerStatus,
}

/// The result of setting up the embedding server: the embeddings source, a handle to
/// the process, and status. Like [`ChatSetup`], the status is immediate
/// (`Connecting`/`NotConfigured`) for managed/external — real readiness arrives from
/// a background `/health` probe; the cloud is `Ready` at once (nothing to load),
/// `NotConfigured` — `UnavailableEmbedder` (the chip is hidden).
pub struct EmbedSetup {
    pub embedder: Arc<dyn Embedder>,
    pub handle: Option<ServerHandle>,
    pub status: ServerStatus,
}

/// (Re)connect/launch servers per settings. Behind a trait — for a mock test of the
/// restart on model change.
///
/// `stored_key` on every method is the already-decrypted saved API key of whatever
/// the slot's **active mode** reads — the cloud provider's key, or, for `external`,
/// that slot's own (`AppConfig::api_keys`, see `shared::secrets::SecretKey`). Which
/// one it is was decided by [`super::orchestrator`]; the supervisor knows neither the
/// storage format nor the addressing. `None` — the key isn't saved on this machine,
/// then the env fallback (`api_key_env`) applies. See
/// docs/research/api-key-storage.md, docs/history/external-api-key.md.
pub trait ServerSupervisor: Send + Sync {
    /// (Re)connects to the chat server. Returns the engine and status immediately
    /// (`Connecting`/`NotConfigured`/`Disconnected`), while managed/external readiness
    /// is sent to `status_tx` by a background probe. `cancel` marks the probe as stale:
    /// on a quick mode switch (managed→external→openai), a late result from the
    /// previous probe shouldn't overwrite the new server's status.
    fn apply_chat(
        &self,
        settings: &EngineSettings,
        stored_key: Option<&str>,
        cancel: CancellationToken,
        status_tx: UnboundedSender<ServerStatus>,
        loc: &'static Locale,
    ) -> ChatSetup;

    /// (Re)connects to/launches the embedding server. Returns the embeddings source and
    /// an immediate status; managed/external readiness is sent to `status_tx` by a
    /// background `/health` probe (`cancel` invalidates a stale one — as on
    /// [`Self::apply_chat`]).
    ///
    /// The probe doesn't make embeddings eager: it's a `/health` GET, the embedder
    /// itself is still touched only on a real call (ADR 0002). Without it the status
    /// would be derived from configuration alone and would read `Ready` for an
    /// unreachable host — the failure would surface only on the first `rag_search`.
    fn apply_embed(
        &self,
        settings: &EmbedSettings,
        stored_key: Option<&str>,
        cancel: CancellationToken,
        status_tx: UnboundedSender<ServerStatus>,
        loc: &'static Locale,
    ) -> EmbedSetup;

    /// (Re)connects to/launches the impersonation server for the `managed`/
    /// `external` modes. For `shared` it is NOT called by the orchestrator (it reuses
    /// the assistant's chat server); if it is called anyway — `NotConfigured`. See
    /// spec §11.8. `cancel` — like on [`Self::apply_chat`] (invalidating a stale
    /// probe). `loc` — the interface language for displayed unavailability reasons
    /// (cloud modes).
    fn apply_impersonation(
        &self,
        settings: &ImpersonationEngineSettings,
        stored_key: Option<&str>,
        cancel: CancellationToken,
        status_tx: UnboundedSender<ServerStatus>,
        loc: &'static Locale,
    ) -> ChatSetup;
}

/// Where the supervisor looks for a `llama-server` the settings do not name
/// outright (spec §3.4): the builds `mindfork llama setup` unpacked under
/// `data/llama/`, and the application's own directory.
///
/// Empty by `Default` — which is what the tests use, and which reduces the
/// resolution to "an explicit path or nothing", the behaviour they were written
/// against.
#[derive(Debug, Clone, Default)]
pub struct BinaryLookup {
    /// `Paths::llama_dir()` — the downloaded builds.
    pub llama_dir: Option<PathBuf>,
    /// `Paths::exe_dir()` — beside the application binary.
    pub exe_dir: Option<PathBuf>,
}

impl BinaryLookup {
    /// The two directories a real installation has.
    pub fn from_paths(paths: &Paths) -> Self {
        Self {
            llama_dir: Some(paths.llama_dir()),
            exe_dir: paths.exe_dir().map(Path::to_path_buf),
        }
    }

    /// The launchable path for a configured setting, or `None` — see
    /// [`crate::features::llama_setup::resolve_binary`].
    pub(crate) fn resolve(&self, configured: Option<&str>) -> Option<PathBuf> {
        crate::features::llama_setup::resolve_binary(
            configured,
            self.exe_dir.as_deref(),
            self.llama_dir.as_deref(),
        )
    }
}

/// The production supervisor: external — by URL (any OpenAI server), managed —
/// a child `llama-server` process (llama.cpp).
#[derive(Default)]
pub struct LlamaSupervisor {
    lookup: BinaryLookup,
}

impl LlamaSupervisor {
    /// The supervisor a real run gets: it can find a downloaded build, or one
    /// unpacked beside the application, when the settings name neither.
    pub fn new(paths: &Paths) -> Self {
        Self {
            lookup: BinaryLookup::from_paths(paths),
        }
    }
}

impl ServerSupervisor for LlamaSupervisor {
    fn apply_chat(
        &self,
        settings: &EngineSettings,
        stored_key: Option<&str>,
        cancel: CancellationToken,
        status_tx: UnboundedSender<ServerStatus>,
        loc: &'static Locale,
    ) -> ChatSetup {
        match settings.mode {
            ServerMode::External => external_chat_setup(
                settings.external.url.as_deref(),
                settings.external.model_name.as_deref(),
                stored_key,
                settings.external.api_key_env.as_deref(),
                cancel,
                status_tx,
                loc,
            ),
            ServerMode::Managed => {
                let cfg = managed_config(&settings.managed, ManagedRole::Assistant, &self.lookup);
                managed_chat_setup(cfg, cancel, status_tx, loc)
            }
            ServerMode::OpenAi | ServerMode::Gemini | ServerMode::Claude | ServerMode::Grok => {
                let cloud = settings.cloud().expect("cloud mode");
                cloud_chat_setup(
                    settings.mode.cloud_provider().expect("cloud mode"),
                    cloud.url.as_deref(),
                    stored_key,
                    cloud.api_key_env.as_deref(),
                    cloud.model_name.as_deref(),
                    loc,
                )
            }
        }
    }

    fn apply_impersonation(
        &self,
        settings: &ImpersonationEngineSettings,
        stored_key: Option<&str>,
        cancel: CancellationToken,
        status_tx: UnboundedSender<ServerStatus>,
        loc: &'static Locale,
    ) -> ChatSetup {
        match settings.mode {
            // `shared` is served by the orchestrator (the assistant's chat server).
            ImpersonationMode::Shared => not_configured(),
            ImpersonationMode::External => external_chat_setup(
                settings.external.url.as_deref(),
                settings.external.model_name.as_deref(),
                stored_key,
                settings.external.api_key_env.as_deref(),
                cancel,
                status_tx,
                loc,
            ),
            ImpersonationMode::Managed => {
                let cfg =
                    managed_config(&settings.managed, ManagedRole::Impersonation, &self.lookup);
                managed_chat_setup(cfg, cancel, status_tx, loc)
            }
            ImpersonationMode::OpenAi
            | ImpersonationMode::Gemini
            | ImpersonationMode::Claude
            | ImpersonationMode::Grok => {
                let cloud = settings.cloud().expect("cloud mode");
                cloud_chat_setup(
                    settings.mode.cloud_provider().expect("cloud mode"),
                    cloud.url.as_deref(),
                    stored_key,
                    cloud.api_key_env.as_deref(),
                    cloud.model_name.as_deref(),
                    loc,
                )
            }
        }
    }

    fn apply_embed(
        &self,
        settings: &EmbedSettings,
        stored_key: Option<&str>,
        cancel: CancellationToken,
        status_tx: UnboundedSender<ServerStatus>,
        loc: &'static Locale,
    ) -> EmbedSetup {
        match settings.mode {
            ServerMode::External => match settings.external.url.as_deref() {
                Some(url) if !url.is_empty() => {
                    let client = Arc::new(
                        OpenAiClient::new(url)
                            .with_api_key(
                                resolve_api_key(
                                    stored_key,
                                    settings.external.api_key_env.as_deref(),
                                )
                                .ok(),
                            )
                            // The same fix as on the chat half: a multi-model
                            // embedding endpoint routes on this field, and an
                            // unset one sends nothing at all.
                            .with_model(settings.external.model_name.clone()),
                    );
                    spawn_probe(
                        client.clone(),
                        EXTERNAL_READY_TIMEOUT,
                        None,
                        cancel,
                        status_tx,
                        loc,
                    );
                    EmbedSetup {
                        embedder: client,
                        handle: None,
                        status: ServerStatus::Connecting,
                    }
                }
                _ => unavailable_embed(),
            },
            // The embedder is the same `llama-server` with `--embeddings`, so it
            // resolves its binary the same way the chat server does — one
            // downloaded build serves both.
            ServerMode::Managed => match self.lookup.resolve(settings.managed.binary.as_deref()) {
                Some(bin) => {
                    let cfg = managed_embed_config(&settings.managed, bin);
                    // The same refusal the chat server makes: without a model file
                    // `llama-server` starts a *router* that answers `/health` and
                    // refuses every embedding request (§2.1 of
                    // docs/research/robustness-and-defaults.md). The chip is hidden
                    // instead, exactly as for a binary that cannot be found.
                    if !cfg.is_runnable() {
                        return unavailable_embed();
                    }
                    match ServerHandle::launch(&cfg, loc) {
                        Ok(handle) => {
                            let client = Arc::new(OpenAiClient::new(handle.base_url()));
                            // A large GGUF loads for seconds; the probe accounts for an
                            // early process exit (corrupt model/OOM) instead of waiting
                            // out the timeout — as for the chat server.
                            spawn_probe(
                                client.clone(),
                                MANAGED_READY_TIMEOUT,
                                Some(handle.exited()),
                                cancel,
                                status_tx,
                                loc,
                            );
                            EmbedSetup {
                                embedder: client,
                                handle: Some(handle),
                                status: ServerStatus::Connecting,
                            }
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, "failed to launch the embedding server; RAG unavailable");
                            // The launch error is displayable (a missing model file and
                            // the like) — surface it in the chip, don't hide it behind
                            // "not configured".
                            EmbedSetup {
                                embedder: Arc::new(UnavailableEmbedder),
                                handle: None,
                                status: ServerStatus::Disconnected(err.to_string()),
                            }
                        }
                    }
                }
                _ => unavailable_embed(),
            },
            ServerMode::OpenAi | ServerMode::Gemini => {
                let cloud = settings.cloud().expect("cloud mode");
                cloud_embed_setup(
                    settings.mode.cloud_provider().expect("cloud mode"),
                    cloud.url.as_deref(),
                    stored_key,
                    cloud.api_key_env.as_deref(),
                    cloud.model_name.as_deref(),
                )
            }
            // Anthropic and xAI have no embeddings API — RAG uses a separate embedder
            // (ADR 0002; docs/research/grok-xai-provider.md §2.7: xAI's
            // `/v1/embedding-models` returns an empty list).
            ServerMode::Claude | ServerMode::Grok => {
                tracing::warn!(
                    mode = ?settings.mode,
                    "this provider has no embeddings API; set a different embedder for RAG"
                );
                unavailable_embed()
            }
        }
    }
}

/// External chat setup: connect by URL (any OpenAI-compatible server), a background
/// probe. The Bearer key (for an authenticated proxy/gateway) is resolved by the same
/// [`resolve_api_key`] chain as a cloud one — `stored_key` first, then the variable
/// named by `api_key_env` — and having **no** key is not an error, unlike in a cloud
/// mode: a local `llama-server` needs none, and then no `Authorization` header is sent
/// (docs/history/external-api-key.md F2/F3).
///
/// Until that document, saved keys applied to cloud providers only, on the argument
/// that an arbitrary URL can't be bound to a provider
/// (docs/research/api-key-storage.md, decision point R4). It is bound to its **slot**
/// instead — `secrets::ExternalSlot` — and the orchestrator has already resolved
/// which one by the time it calls here.
///
/// `model_name` is the section's "Model (opt.)" field, and it goes **on the
/// wire**: a multi-model endpoint — llama.cpp's own router mode, LiteLLM,
/// OpenRouter — routes on the request's `model` and answers
/// `400 "model name is missing from the request"` without it, which is what this
/// mode's documented gateway setups (install.md §3.1) used to hit. A
/// single-model `llama-server` ignores the field (measured: a request naming a
/// model it has never heard of is answered by the loaded one), and an unset
/// field sends no `model` key at all — byte-identical to the request shape that
/// shipped before. See docs/research/external-model-name.md §2.3.
fn external_chat_setup(
    url: Option<&str>,
    model_name: Option<&str>,
    stored_key: Option<&str>,
    api_key_env: Option<&str>,
    cancel: CancellationToken,
    status_tx: UnboundedSender<ServerStatus>,
    loc: &'static Locale,
) -> ChatSetup {
    match url {
        Some(url) if !url.is_empty() => {
            let key = resolve_api_key(stored_key, api_key_env).ok();
            let client = Arc::new(
                OpenAiClient::new(url)
                    .with_api_key(key)
                    .with_model(model_name.map(str::to_string)),
            );
            spawn_probe(
                client.clone(),
                EXTERNAL_READY_TIMEOUT,
                None,
                cancel,
                status_tx,
                loc,
            );
            ChatSetup {
                // Retried like a cloud, because that is what it usually is: this
                // mode's URL is as often a proxy or a gateway (LiteLLM,
                // OpenRouter) as a local `llama-server`, and those answer `429`/
                // `502` while we own neither the process nor its capacity. A
                // genuinely local server loses nothing — its `503 Loading model`
                // is pre-stream and transient, so it rides out in one wait
                // (fork F2(a)).
                backend: Some(retry::RetryBackend::wrap(client)),
                handle: None,
                status: ServerStatus::Connecting,
            }
        }
        _ => not_configured(),
    }
}

/// Managed chat setup: launch a child `llama-server`, a background probe (accounting
/// for an early process exit). An empty binary **or an unset model** →
/// `NotConfigured` ([`ManagedConfig::is_runnable`], which carries the measurement
/// behind the model half).
fn managed_chat_setup(
    cfg: ManagedConfig,
    cancel: CancellationToken,
    status_tx: UnboundedSender<ServerStatus>,
    loc: &'static Locale,
) -> ChatSetup {
    if !cfg.is_runnable() {
        return not_configured();
    }
    match ServerHandle::launch(&cfg, loc) {
        Ok(handle) => {
            let client = Arc::new(OpenAiClient::new(handle.base_url()));
            spawn_probe(
                client.clone(),
                MANAGED_READY_TIMEOUT,
                Some(handle.exited()),
                cancel,
                status_tx,
                loc,
            );
            ChatSetup {
                backend: Some(client),
                handle: Some(handle),
                status: ServerStatus::Connecting,
            }
        }
        Err(err) => ChatSetup {
            backend: None,
            handle: None,
            status: ServerStatus::Disconnected(err.to_string()),
        },
    }
}

/// The embedding server's launch config: the same `llama-server` with
/// `--embeddings`, its binary already resolved by the caller.
///
/// A function rather than a literal inside `embed_setup` so that `mindfork setup
/// --verify` starts **exactly** what the app will (`app::verify`) — a second
/// copy of these fields would drift the first time one of them changed.
pub(crate) fn managed_embed_config(m: &ManagedEmbedSettings, binary: PathBuf) -> ManagedConfig {
    ManagedConfig {
        binary,
        model_path: m.model_path.clone(),
        // An embedding model has no image encoder — the projector is a
        // chat-server setting only.
        mmproj: None,
        gpu_layers: m.gpu_layers,
        context_size: crate::shared::config::DEFAULT_CONTEXT_SIZE,
        // Its batch is the context size (`build_args`), not this.
        batch_size: None,
        // Embeddings are one request at a time; the chat server's session
        // budget is not this server's.
        parallel: 1,
        jinja: false, // the embedding server doesn't need a chat template
        reasoning_format: None,
        embeddings: true,
        no_mmap: false,
        // Speculative decoding/FlashAttention aren't applicable to the
        // embedding server (it doesn't generate tokens).
        flash_attn: None,
        spec_type: None,
        draft_model: None,
        draft_gpu_layers: None,
        draft_n_max: None,
        draft_n_min: None,
        host: "127.0.0.1".into(),
        port: m.port,
        role: ManagedRole::Embedder,
        extra_args: m.extra_args.clone(),
    }
}

/// Builds a [`ManagedConfig`] (`llama-server`) from the engine's managed subsection
/// (shared by the assistant's chat server and the impersonation server — `role`
/// says which, since their sections show different fields and the extra
/// arguments are judged against them).
///
/// The binary goes through [`BinaryLookup`]: an explicit path is used as
/// written, a bare name may be found beside the application, and an empty
/// setting resolves to the build installed last under `data/llama/`. Nothing
/// found leaves the path empty, which `managed_chat_setup` reads as
/// `NotConfigured` exactly as before.
pub(crate) fn managed_config(
    s: &ManagedSettings,
    role: ManagedRole,
    lookup: &BinaryLookup,
) -> ManagedConfig {
    ManagedConfig {
        binary: lookup.resolve(s.binary.as_deref()).unwrap_or_default(),
        model_path: s.model_path.clone(),
        mmproj: s.mmproj.clone(),
        gpu_layers: s.gpu_layers,
        context_size: s.context_size,
        batch_size: s.batch_size,
        parallel: s.sessions,
        jinja: s.jinja,
        reasoning_format: s.reasoning_format.clone(),
        embeddings: false,
        no_mmap: s.no_mmap,
        flash_attn: s.flash_attn.as_arg().map(str::to_string),
        spec_type: s.spec_type.as_arg().map(str::to_string),
        draft_model: s.draft_model.clone(),
        draft_gpu_layers: s.draft_gpu_layers,
        draft_n_max: s.draft_n_max,
        draft_n_min: s.draft_n_min,
        host: s.host.clone(),
        port: s.port,
        role,
        extra_args: s.extra_args.clone(),
    }
}

/// A structured API-key resolution error (no locale — the caller localizes it
/// itself, see [`cloud_chat_setup`]). `NoName` — the key isn't saved and the name of
/// the env variable isn't set; `Missing` carries the name of a variable missing from
/// the environment.
#[derive(Debug)]
pub(crate) enum ApiKeyError {
    NoName,
    Missing(String),
}

/// Resolves the API key. Order (docs/research/api-key-storage.md §5, decision
/// point R3):
///
/// 1. **the saved key of this machine** (`stored`, already decrypted by the caller) —
///    it was entered by an explicit action in settings, the target user never sees env;
/// 2. fallback — the env variable named `api_key_env` (CI, power users, systems
///    without a machine-id).
///
/// The secret itself still doesn't sit on disk in plaintext: the saved key is
/// encrypted with the machine key (`shared::secrets`), the config holds ciphertext.
pub(crate) fn resolve_api_key(
    stored: Option<&str>,
    api_key_env: Option<&str>,
) -> Result<String, ApiKeyError> {
    if let Some(key) = stored.filter(|k| !k.is_empty()) {
        return Ok(key.to_string());
    }
    let var = api_key_env
        .filter(|v| !v.is_empty())
        .ok_or(ApiKeyError::NoName)?;
    std::env::var(var).map_err(|_| ApiKeyError::Missing(var.to_string()))
}

/// Builds a cloud chat backend (OpenAI/Gemini-compat): the provider's base URL (with
/// a possible override via `url`), a Bearer key from env, the model name, a strict
/// OpenAI dialect. The cloud doesn't "load a model" — the status is immediately
/// `Ready` (no probe). If the model isn't specified or the key is unavailable —
/// `Disconnected` with a clear message (so as not to hit a `400` already mid-request).
/// See ADR 0004.
fn cloud_chat_setup(
    provider: CloudProvider,
    url_override: Option<&str>,
    stored_key: Option<&str>,
    api_key_env: Option<&str>,
    model_name: Option<&str>,
    loc: &'static Locale,
) -> ChatSetup {
    let disconnected = |msg: String| ChatSetup {
        backend: None,
        handle: None,
        status: ServerStatus::Disconnected(msg),
    };
    let Some(model) = model_name.filter(|m| !m.is_empty()) else {
        return disconnected(loc.t("ui.err.server.no_model").into());
    };
    let key = match resolve_api_key(stored_key, api_key_env) {
        Ok(k) => k,
        Err(ApiKeyError::NoName) => {
            return disconnected(loc.t("ui.err.server.no_api_key").into());
        }
        Err(ApiKeyError::Missing(var)) => {
            return disconnected(loc.tf("ui.err.server.env_missing", &[("var", &var)]));
        }
    };
    // The chat URL is the provider's native path (Gemini's `…/v1beta`, not
    // compat-embeddings).
    let base = url_override
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| provider.chat_base_url());
    // The backend by the provider's protocol: OpenAI Responses API
    // (`ResponsesClient` — reasoning summaries, reasoning.effort, verbosity), Gemini
    // via native generateContent (`GeminiClient` — "thoughts" summaries,
    // thinkingLevel/thinkingBudget), the Anthropic Messages API (Claude), or —
    // uniquely — plain Chat Completions for Grok: xAI is the one cloud whose
    // OpenAI-compatible path already carries reasoning (`delta.reasoning_content`,
    // the field `OpenAiClient` parses for llama.cpp) and needs no thinking-signature
    // round-trip on tool use. See docs/research/grok-xai-provider.md.
    let backend: Arc<dyn EngineBackend> = match provider {
        CloudProvider::OpenAi => Arc::new(ResponsesClient::new(base, key, model.to_string())),
        CloudProvider::Gemini => Arc::new(GeminiClient::new(base, key, model.to_string())),
        CloudProvider::Claude => Arc::new(AnthropicClient::new(base, key, model.to_string())),
        CloudProvider::Grok => Arc::new(
            OpenAiClient::new(base)
                .with_api_key(Some(key))
                .with_model(Some(model.to_string()))
                .with_effort_none_omitted(true),
        ),
    };
    ChatSetup {
        // Every cloud provider sheds load as a matter of course, and a cloud
        // backend is never monitored or relaunched (there is nothing to relaunch
        // and probing costs money), so a per-request retry is the only recovery
        // mechanism it will ever have. See docs/research/cloud-retry-backoff.md §3.
        backend: Some(retry::RetryBackend::wrap(backend)),
        handle: None,
        status: ServerStatus::Ready,
    }
}

/// Builds a cloud embedding backend (OpenAI/Gemini). On an incomplete configuration
/// (no model or key) — `UnavailableEmbedder` (RAG returns a clear error, doesn't
/// crash), same as for other unconfigured embedders.
fn cloud_embed_setup(
    provider: CloudProvider,
    url_override: Option<&str>,
    stored_key: Option<&str>,
    api_key_env: Option<&str>,
    model_name: Option<&str>,
) -> EmbedSetup {
    let (Some(model), Ok(key)) = (
        model_name.filter(|m| !m.is_empty()),
        resolve_api_key(stored_key, api_key_env),
    ) else {
        tracing::warn!("cloud embeddings not configured (model/key); RAG unavailable");
        return unavailable_embed();
    };
    let base = url_override
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| provider.base_url());
    let client = OpenAiClient::new(base)
        .with_api_key(Some(key))
        .with_model(Some(model.to_string()));
    EmbedSetup {
        status: ServerStatus::Ready,
        embedder: Arc::new(client),
        handle: None,
    }
}

fn not_configured() -> ChatSetup {
    ChatSetup {
        backend: None,
        handle: None,
        status: ServerStatus::NotConfigured,
    }
}

fn unavailable_embed() -> EmbedSetup {
    EmbedSetup {
        embedder: Arc::new(UnavailableEmbedder),
        handle: None,
        status: ServerStatus::NotConfigured,
    }
}

/// The health-state machine of one monitored server, factored out so the transition
/// rules are testable without a server or a clock.
///
/// Asymmetric by design (see docs/server-health-monitoring.md, F3): going *down*
/// takes [`FAILURES_TO_UNHEALTHY`] consecutive failures, going *up* takes one
/// success. A single missed poll isn't evidence a server is down — it can be one
/// dropped packet or a slot-contention `503` on some builds — and a chip that
/// flickers red teaches the user to ignore it. A success needs no corroboration:
/// the server answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Health {
    healthy: bool,
    failures: u32,
}

impl Health {
    fn new(healthy: bool) -> Self {
        Self {
            healthy,
            failures: 0,
        }
    }

    /// How long to wait before the next poll. Fast whenever something might be
    /// wrong — while down the poll *is* the recovery mechanism, and mid-streak it
    /// decides a pending verdict; otherwise slow, since a healthy server's failure
    /// would be reported by the next real request anyway.
    fn poll_delay(&self) -> Duration {
        if self.healthy && self.failures == 0 {
            HEALTHY_POLL
        } else {
            RECHECK_POLL
        }
    }

    /// Records one probe outcome. Returns `Some(healthy)` when the verdict actually
    /// flipped — the caller only publishes a status on a flip, so a steady server
    /// doesn't wake the UI every minute.
    fn record(&mut self, ok: bool) -> Option<bool> {
        if ok {
            self.failures = 0;
            return (!std::mem::replace(&mut self.healthy, true)).then_some(true);
        }
        self.failures += 1;
        (self.healthy && self.failures >= FAILURES_TO_UNHEALTHY).then(|| {
            self.healthy = false;
            false
        })
    }
}

/// A background health monitor for one server: reports the first verdict, then keeps
/// watching so the status can't go stale. If the monitor is marked stale (`cancel`)
/// it stops without sending anything: otherwise a late result from the previous
/// server (e.g. a timeout of an intermediate external while flipping between modes)
/// would overwrite the new server's status.
///
/// Two things it is *not*: it doesn't gate anything (the chat gate reads the last
/// published status, exactly as before), and it doesn't relaunch anything — a dead
/// managed child is the orchestrator's business (it owns the handle), see
/// `Orchestrator::relaunch_dead_managed_servers`.
fn spawn_probe(
    client: Arc<OpenAiClient>,
    timeout: Duration,
    exited: Option<ChildExit>,
    cancel: CancellationToken,
    status_tx: UnboundedSender<ServerStatus>,
    loc: &'static Locale,
) {
    tokio::spawn(async move {
        // Phase 1 — the first verdict. A managed GGUF can load for minutes, so this
        // keeps its own generous retry loop; the steady-state cadence below is a
        // different question and starts only once we know where we stand.
        let status = tokio::select! {
            biased;
            _ = cancel.cancelled() => return,
            res = wait_until_ready(&client, timeout, exited.clone(), loc) => match res {
                Ok(()) => ServerStatus::Ready,
                Err(err) => ServerStatus::Disconnected(err.to_string()),
            },
        };
        // While the probe was finishing, it may have been invalidated by a mode switch.
        if cancel.is_cancelled() {
            return;
        }
        let mut health = Health::new(matches!(status, ServerStatus::Ready));
        if status_tx.send(status).is_err() {
            return; // the orchestrator is gone
        }

        // Phase 2 — keep watching. Without this the status would describe the moment
        // the server was configured rather than the present: a host that went down
        // would stay green, and — the half users actually feel — a server that came
        // up *after* the app would stay unusable, since the chat gate reads this
        // status (see docs/server-health-monitoring.md §3).
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return,
                // A managed child that exited will never answer again: report at once
                // instead of waiting out a probe, and stop — reviving it means
                // relaunching the process, which the orchestrator does.
                exit = wait_for_exit(&exited) => {
                    let _ = status_tx.send(ServerStatus::Disconnected(exit.message(loc)));
                    return;
                }
                _ = tokio::time::sleep(health.poll_delay()) => {}
            }
            if cancel.is_cancelled() {
                return;
            }
            let outcome = client.probe().await;
            let reason = outcome.as_ref().err().map(|e| e.to_string());
            if let Some(healthy) = health.record(outcome.is_ok()) {
                let next = if healthy {
                    ServerStatus::Ready
                } else {
                    ServerStatus::Disconnected(reason.unwrap_or_default())
                };
                if status_tx.send(next).is_err() {
                    return;
                }
            }
        }
    });
}

/// Waits for a managed child's exit signal; for a server we don't own (external)
/// there is none, so this never resolves and simply never wins its `select!` arm.
async fn wait_for_exit(exited: &Option<ChildExit>) -> &ChildExit {
    match exited {
        Some(exit) => {
            exit.wait().await;
            exit
        }
        None => std::future::pending().await,
    }
}

/// The demo mode's supervisor (`mindfork demo`): a scripted backend for chat
/// and impersonation and a deterministic in-process embedder — `Ready`
/// immediately, no child processes, no network, no background probes (the
/// statuses are true by construction, there is nothing to probe). Isolation
/// from the real data root is the caller's job: `main` boots it on a
/// throwaway root. Unlike [`MockSupervisor`] it carries no test machinery —
/// no call counters, no unavailability switches.
pub struct DemoSupervisor {
    backend: Arc<dyn EngineBackend>,
}

impl DemoSupervisor {
    pub fn new(backend: Arc<dyn EngineBackend>) -> Self {
        Self { backend }
    }
}

impl ServerSupervisor for DemoSupervisor {
    fn apply_chat(
        &self,
        _settings: &EngineSettings,
        _stored_key: Option<&str>,
        _cancel: CancellationToken,
        _status_tx: UnboundedSender<ServerStatus>,
        _loc: &'static Locale,
    ) -> ChatSetup {
        ChatSetup {
            backend: Some(self.backend.clone()),
            handle: None,
            status: ServerStatus::Ready,
        }
    }

    fn apply_embed(
        &self,
        _settings: &EmbedSettings,
        _stored_key: Option<&str>,
        _cancel: CancellationToken,
        _status_tx: UnboundedSender<ServerStatus>,
        _loc: &'static Locale,
    ) -> EmbedSetup {
        EmbedSetup {
            embedder: Arc::new(crate::shared::api::mock::MockEmbedder::new(16)),
            handle: None,
            status: ServerStatus::Ready,
        }
    }

    fn apply_impersonation(
        &self,
        _settings: &ImpersonationEngineSettings,
        _stored_key: Option<&str>,
        _cancel: CancellationToken,
        _status_tx: UnboundedSender<ServerStatus>,
        _loc: &'static Locale,
    ) -> ChatSetup {
        ChatSetup {
            backend: Some(self.backend.clone()),
            handle: None,
            status: ServerStatus::Ready,
        }
    }
}

/// A mock supervisor for orchestrator tests: hands back a given chat backend and a
/// deterministic embedder, counts `apply_chat` calls (checking a restart on model
/// change, DoD M8). No real processes.
#[cfg(test)]
pub struct MockSupervisor {
    backend: Option<Arc<dyn EngineBackend>>,
    chat_calls: std::sync::atomic::AtomicUsize,
    /// The `stored_key` of every `apply_chat`, in order — what the **orchestrator**
    /// resolved for the active mode. A mode addressing the wrong secret (a provider's
    /// key for an external server, or the other way round) is otherwise invisible from
    /// outside: the supervisor would simply be handed the wrong string.
    chat_keys: std::sync::Mutex<Vec<Option<String>>>,
    embed_dim: usize,
    /// An optional **real** embedder (live smokes — bge-m3 from MINDFORK_EMBED_URL
    /// and so on); `None` → a deterministic `MockEmbedder`.
    embedder: Option<Arc<dyn Embedder>>,
    /// Report `UnavailableEmbedder` instead of the `MockEmbedder` fallback — the
    /// only way to express "no embedder at all" (see `with_backend_no_embedder`).
    embed_unavailable: bool,
}

#[cfg(test)]
impl MockSupervisor {
    /// A supervisor returning `backend` for chat (the in-process mock is ready right
    /// away — status `Ready` synchronously, no background probe, so tests don't
    /// depend on a race).
    pub fn with_backend(backend: Option<Arc<dyn EngineBackend>>) -> Self {
        Self {
            backend,
            chat_calls: std::sync::atomic::AtomicUsize::new(0),
            chat_keys: std::sync::Mutex::new(Vec::new()),
            embed_dim: 16,
            embedder: None,
            embed_unavailable: false,
        }
    }

    /// Like [`Self::with_backend`], but with a given **real** embedder (live smokes
    /// against a real embedding server). `None` → the test `MockEmbedder`.
    pub fn with_backend_and_embedder(
        backend: Option<Arc<dyn EngineBackend>>,
        embedder: Option<Arc<dyn Embedder>>,
    ) -> Self {
        Self {
            backend,
            chat_calls: std::sync::atomic::AtomicUsize::new(0),
            chat_keys: std::sync::Mutex::new(Vec::new()),
            embed_dim: 16,
            embedder,
            embed_unavailable: false,
        }
    }

    /// A supervisor that reports **no embedder at all** (`UnavailableEmbedder`).
    ///
    /// Distinct from passing `None` above, which falls back to a `MockEmbedder`
    /// — convenient for most tests, but it means "no embedder" cannot be
    /// expressed that way. Tests that need the *absence* (a RAG/attachment path
    /// degrading, a route the model must not be offered) need this.
    pub fn with_backend_no_embedder(backend: Option<Arc<dyn EngineBackend>>) -> Self {
        Self {
            embed_unavailable: true,
            ..Self::with_backend_and_embedder(backend, None)
        }
    }

    /// How many times `apply_chat` was called (≥2 after a restart on model change).
    pub fn chat_call_count(&self) -> usize {
        self.chat_calls.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The keys `apply_chat` was called with, in order (see [`Self::chat_keys`]).
    pub fn chat_keys(&self) -> Vec<Option<String>> {
        self.chat_keys.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl ServerSupervisor for MockSupervisor {
    fn apply_chat(
        &self,
        _settings: &EngineSettings,
        stored_key: Option<&str>,
        _cancel: CancellationToken,
        _status_tx: UnboundedSender<ServerStatus>,
        _loc: &'static Locale,
    ) -> ChatSetup {
        self.chat_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.chat_keys
            .lock()
            .unwrap()
            .push(stored_key.map(str::to_string));
        let backend = self.backend.clone();
        // The mock engine is ready instantly: we hand back `Ready` as the immediate
        // status (rather than `Connecting` + an async probe), otherwise the
        // orchestrator could handle a generation command before the readiness event
        // and reject it (a race in tests).
        let status = if backend.is_some() {
            ServerStatus::Ready
        } else {
            ServerStatus::NotConfigured
        };
        ChatSetup {
            backend,
            handle: None,
            status,
        }
    }

    fn apply_impersonation(
        &self,
        _settings: &ImpersonationEngineSettings,
        _stored_key: Option<&str>,
        _cancel: CancellationToken,
        _status_tx: UnboundedSender<ServerStatus>,
        _loc: &'static Locale,
    ) -> ChatSetup {
        // The mock hands back the same backend, ready right away (like apply_chat) —
        // for tests of the managed/external impersonation modes.
        let backend = self.backend.clone();
        let status = if backend.is_some() {
            ServerStatus::Ready
        } else {
            ServerStatus::NotConfigured
        };
        ChatSetup {
            backend,
            handle: None,
            status,
        }
    }

    fn apply_embed(
        &self,
        _settings: &EmbedSettings,
        _stored_key: Option<&str>,
        _cancel: CancellationToken,
        _status_tx: UnboundedSender<ServerStatus>,
        _loc: &'static Locale,
    ) -> EmbedSetup {
        if self.embed_unavailable {
            return unavailable_embed();
        }
        let embedder = self.embedder.clone().unwrap_or_else(|| {
            Arc::new(crate::shared::api::mock::MockEmbedder::new(self.embed_dim))
        });
        // Ready right away, no async probe — same reasoning as `apply_chat`: tests
        // must not depend on a status race.
        EmbedSetup {
            embedder,
            handle: None,
            status: ServerStatus::Ready,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::api::EmbedRole;
    use crate::shared::config::ManagedEmbedSettings;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::mpsc::unbounded_channel;

    /// The production path end to end: a managed section with **no** binary at
    /// all, a real `data/llama/` with a build `mindfork llama setup` put there,
    /// and a real model — the app must launch that build and reach `Ready`.
    /// Unit tests settle the resolution rule; only this settles that what the
    /// rule points at is something `ServerHandle::launch` can actually run.
    ///
    ///     MINDFORK_LLAMA_DIR=…/data/llama MINDFORK_MODEL=…/small.gguf \
    ///       cargo test empty_binary_launches -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "requires a downloaded build (MINDFORK_LLAMA_DIR) and a model (MINDFORK_MODEL)"]
    async fn empty_binary_launches_the_downloaded_build_live() {
        let (Ok(llama_dir), Ok(model)) = (
            std::env::var("MINDFORK_LLAMA_DIR"),
            std::env::var("MINDFORK_MODEL"),
        ) else {
            eprintln!("skip: MINDFORK_LLAMA_DIR / MINDFORK_MODEL not set");
            return;
        };
        let settings = EngineSettings {
            mode: ServerMode::Managed,
            managed: ManagedSettings {
                binary: None, // the whole point: nothing configured
                model_path: Some(model),
                gpu_layers: 0,
                context_size: 2048,
                port: 18126,
                ..Default::default()
            },
            ..Default::default()
        };
        let supervisor = LlamaSupervisor {
            lookup: BinaryLookup {
                llama_dir: Some(llama_dir.clone().into()),
                exe_dir: None,
            },
        };
        eprintln!(
            "resolved: {:?}",
            supervisor
                .lookup
                .resolve(None)
                .expect("a build to resolve to")
        );

        let (tx, mut rx) = unbounded_channel();
        let setup = supervisor.apply_chat(&settings, None, CancellationToken::new(), tx, ru());
        assert_eq!(
            setup.status,
            ServerStatus::Connecting,
            "an empty field with a build on disk must not read as NotConfigured"
        );
        let _handle = setup.handle.expect("a child process");
        let status = tokio::time::timeout(Duration::from_secs(600), rx.recv())
            .await
            .expect("the probe should report within the readiness timeout")
            .expect("the probe channel should not close");
        assert_eq!(status, ServerStatus::Ready, "the resolved build must run");
    }

    /// A downloaded build under `data/llama/` is what an empty binary field
    /// resolves to (spec §3.4). Without a lookup — the shape every other test
    /// in this module is written against — an empty field is still nothing.
    #[test]
    fn an_empty_binary_resolves_to_a_downloaded_build() {
        let data = tempfile::tempdir().unwrap();
        let install = data.path().join("vulkan-b10883");
        std::fs::create_dir_all(&install).unwrap();
        let binary = install.join(crate::features::llama_setup::server_binary_name());
        std::fs::write(&binary, b"x").unwrap();
        let lookup = BinaryLookup {
            llama_dir: Some(data.path().to_path_buf()),
            exe_dir: None,
        };

        let cfg = managed_config(&ManagedSettings::default(), ManagedRole::Assistant, &lookup);
        assert_eq!(cfg.binary, binary);

        let bare = managed_config(
            &ManagedSettings::default(),
            ManagedRole::Assistant,
            &BinaryLookup::default(),
        );
        assert!(
            bare.binary.as_os_str().is_empty(),
            "no lookup, no path — `managed_chat_setup` reads that as NotConfigured"
        );
    }

    /// The embedder is the same binary with `--embeddings`, so one downloaded
    /// build serves it too — an empty `embed.managed.binary` must stop being
    /// `NotConfigured` once there is something to find.
    #[test]
    fn the_embedder_resolves_its_binary_the_same_way() {
        let data = tempfile::tempdir().unwrap();
        let install = data.path().join("cpu-b10883");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::write(
            install.join(crate::features::llama_setup::server_binary_name()),
            b"x",
        )
        .unwrap();
        // As for the chat server, a model is what makes this a launch attempt at all.
        let model = tempfile::NamedTempFile::new().unwrap();
        let settings = EmbedSettings {
            mode: ServerMode::Managed,
            managed: ManagedEmbedSettings {
                model_path: Some(model.path().display().to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let (tx, _rx) = unbounded_channel();
        let without = LlamaSupervisor::default().apply_embed(
            &settings,
            None,
            CancellationToken::new(),
            tx,
            ru(),
        );
        assert_eq!(without.status, ServerStatus::NotConfigured);

        let (tx, _rx) = unbounded_channel();
        let with = LlamaSupervisor {
            lookup: BinaryLookup {
                llama_dir: Some(data.path().to_path_buf()),
                exe_dir: None,
            },
        }
        .apply_embed(&settings, None, CancellationToken::new(), tx, ru());
        assert_ne!(
            with.status,
            ServerStatus::NotConfigured,
            "a build was found, so this is a launch attempt, not a missing setting"
        );
    }

    /// A managed engine with a build but **no model** is "not configured", not a
    /// server that is about to start: measured, `llama-server` without `-m` comes
    /// up as a router whose `/health` says `ok` while every completion is a `400`,
    /// and whose `models_autoload` would fetch a model from Hugging Face on a name
    /// match (docs/research/robustness-and-defaults.md §2.1, D2).
    #[tokio::test]
    async fn managed_without_a_model_is_not_configured() {
        for model in [None, Some(String::new()), Some("   ".to_string())] {
            let (tx, _rx) = unbounded_channel();
            let s = EngineSettings {
                mode: ServerMode::Managed,
                managed: ManagedSettings {
                    binary: Some("llama-server".into()),
                    model_path: model.clone(),
                    ..Default::default()
                },
                ..Default::default()
            };
            let setup =
                LlamaSupervisor::default().apply_chat(&s, None, CancellationToken::new(), tx, ru());
            assert!(setup.backend.is_none(), "{model:?}");
            assert!(
                setup.handle.is_none(),
                "no process is started for {model:?}"
            );
            assert_eq!(setup.status, ServerStatus::NotConfigured, "{model:?}");
        }
    }

    /// The same refusal one screen further: the embedding server takes a model the
    /// same way, and a router that answers `/health` and refuses every embedding
    /// request is the same failure (N2).
    #[tokio::test]
    async fn the_embedder_without_a_model_is_unavailable() {
        let data = tempfile::tempdir().unwrap();
        let install = data.path().join("cpu-b10883");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::write(
            install.join(crate::features::llama_setup::server_binary_name()),
            b"x",
        )
        .unwrap();
        let settings = EmbedSettings {
            mode: ServerMode::Managed,
            ..Default::default()
        };
        let (tx, _rx) = unbounded_channel();
        let setup = LlamaSupervisor {
            lookup: BinaryLookup {
                llama_dir: Some(data.path().to_path_buf()),
                exe_dir: None,
            },
        }
        .apply_embed(&settings, None, CancellationToken::new(), tx, ru());
        assert!(setup.handle.is_none(), "no process is started");
        assert_eq!(setup.status, ServerStatus::NotConfigured);
    }

    /// The reference (Russian) locale for displayed unavailability reasons:
    /// assertions on Russian substrings are pinned byte-for-byte.
    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    /// Spawns a throwaway local server on an ephemeral port and returns its base URL
    /// plus a switch: while it's `true` the server answers `/health` with `200`,
    /// while it's `false` it accepts the connection and hangs up (the probe fails).
    /// Flipping the switch mid-test is how a server "goes down" and "comes back".
    ///
    /// It hangs up rather than the test pointing at a *closed* port on purpose:
    /// connecting to a closed port costs ~2s per attempt on Windows and the probe
    /// retries until its timeout — which turned an early version of this into a
    /// minute-long test.
    async fn spawn_stub_server(healthy: bool) -> (String, Arc<AtomicBool>) {
        let switch = Arc::new(AtomicBool::new(healthy));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let flag = switch.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let flag = flag.clone();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    // Drain the request first: closing a socket with unread data
                    // pending sends an RST, and the client then loses the response.
                    let mut buf = [0u8; 1024];
                    let _ = sock.read(&mut buf).await;
                    if flag.load(Ordering::SeqCst) {
                        let _ = sock
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                            .await;
                        let _ = sock.shutdown().await;
                    }
                    // Otherwise: drop the socket — the client sees the connection close.
                });
            }
        });
        (format!("http://{addr}/v1"), switch)
    }

    /// `apply_embed` with the probe plumbing defaulted — for tests that only care
    /// about the embedder/the immediate status. Dropping the receiver is harmless:
    /// a probe that can't deliver its status just fails the send.
    fn embed_setup(s: &EmbedSettings) -> EmbedSetup {
        let (tx, _rx) = unbounded_channel();
        LlamaSupervisor::default().apply_embed(s, None, CancellationToken::new(), tx, ru())
    }

    fn embed_external(url: &str) -> EmbedSettings {
        EmbedSettings {
            mode: ServerMode::External,
            external: crate::shared::config::ExternalSettings {
                url: Some(url.into()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn external(url: Option<&str>) -> EngineSettings {
        EngineSettings {
            mode: ServerMode::External,
            external: crate::shared::config::ExternalSettings {
                url: url.map(String::from),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn external_with_url_yields_backend_connecting() {
        let (tx, _rx) = unbounded_channel();
        let setup = LlamaSupervisor::default().apply_chat(
            &external(Some("http://127.0.0.1:9/v1")),
            None,
            CancellationToken::new(),
            tx,
            ru(),
        );
        assert!(setup.backend.is_some());
        assert!(setup.handle.is_none());
        assert_eq!(setup.status, ServerStatus::Connecting);
    }

    /// A one-request stub: returns its URL and a handle yielding the `Authorization`
    /// header the request carried (lowercased; empty when there was none). What makes
    /// the request is the readiness probe the external setup spawns, so these tests
    /// observe the key **on the wire** rather than the resolution in isolation.
    fn auth_probe_stub() -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            // Read to the end of the headers rather than one bounded chunk: a key can
            // come from a variable of any length (`PATH` alone is past 2 KiB here), and
            // a truncated buffer would silently cut the value under comparison.
            let mut req = Vec::new();
            let mut buf = [0u8; 512];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                match sock.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => req.extend_from_slice(&buf[..n]),
                }
            }
            // Answer only after draining: writing first turns the close into an RST
            // that discards the response (lessons.md §2).
            sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
            String::from_utf8_lossy(&req)
                .to_ascii_lowercase()
                .lines()
                .find(|l| l.starts_with("authorization:"))
                .unwrap_or_default()
                .trim()
                .to_string()
        });
        (format!("http://{addr}/v1"), seen)
    }

    /// Waits for the stub off the runtime thread: a bare `join()` would block the
    /// executor, and then the probe task that has to make the request never runs.
    async fn header(seen: std::thread::JoinHandle<String>) -> String {
        tokio::task::spawn_blocking(move || seen.join().unwrap())
            .await
            .unwrap()
    }

    /// The chat key for one external server, resolved and sent.
    async fn external_chat_header(stored: Option<&str>, api_key_env: Option<&str>) -> String {
        let (url, seen) = auth_probe_stub();
        let mut settings = external(Some(&url));
        settings.external.api_key_env = api_key_env.map(String::from);
        let (tx, _rx) = unbounded_channel();
        let setup = LlamaSupervisor::default().apply_chat(
            &settings,
            stored,
            CancellationToken::new(),
            tx,
            ru(),
        );
        assert!(setup.backend.is_some(), "external mode yields a backend");
        header(seen).await
    }

    /// The external server's key follows the same chain as a cloud one — a key stored
    /// for this slot first, then the variable named in settings — and having neither
    /// stays legitimate rather than an error: that is the local `llama-server` path,
    /// and it must keep sending no `Authorization` at all
    /// (docs/history/external-api-key.md F2/F3).
    #[tokio::test]
    async fn external_sends_the_stored_key_and_falls_back_to_env() {
        // `PATH` is read rather than set: `set_var` is `unsafe` in edition 2024 and
        // races every other test in the binary (the trick the sibling tests use).
        let from_env = std::env::var("PATH").unwrap().to_ascii_lowercase();
        assert_eq!(
            external_chat_header(Some("sk-stored"), Some("PATH")).await,
            "authorization: bearer sk-stored",
            "a stored key wins over the named variable"
        );
        assert_eq!(
            external_chat_header(None, Some("PATH")).await,
            format!("authorization: bearer {from_env}"),
            "with nothing stored, the named variable is read"
        );
        assert_eq!(
            external_chat_header(None, None).await,
            "",
            "no key anywhere — no header, as before this feature existed"
        );
    }

    /// Reads one HTTP request off `sock`: headers to their `\r\n\r\n` end, then
    /// exactly `Content-Length` more bytes — a bounded single read would cut the
    /// body under comparison. `(head, body)`, or `None` when the peer goes away
    /// mid-headers.
    fn read_request(sock: &mut std::net::TcpStream) -> Option<(String, String)> {
        use std::io::Read;
        let mut req = Vec::new();
        let mut buf = [0u8; 1024];
        let head_end = loop {
            match sock.read(&mut buf) {
                Ok(0) | Err(_) => return None,
                Ok(n) => req.extend_from_slice(&buf[..n]),
            }
            if let Some(i) = req.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
        };
        let head = String::from_utf8_lossy(&req[..head_end]).to_string();
        let len = content_length(&head);
        while req.len() < head_end + len {
            match sock.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => req.extend_from_slice(&buf[..n]),
            }
        }
        let body = String::from_utf8_lossy(&req[head_end..]).to_string();
        Some((head, body))
    }

    /// The `Content-Length` a request head announces; `0` when absent.
    fn content_length(head: &str) -> usize {
        head.lines()
            .find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse().ok())
            })
            .unwrap_or(0)
    }

    /// Answers connections until a `POST` arrives, and reports that request's
    /// body. The readiness probe's `GET /health` lands on the same stub and is
    /// answered and skipped — it is not the request under test.
    fn chat_body_stub() -> (String, std::thread::JoinHandle<String>) {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = std::thread::spawn(move || {
            loop {
                let Ok((mut sock, _)) = listener.accept() else {
                    return String::new();
                };
                let Some((head, body)) = read_request(&mut sock) else {
                    continue;
                };
                // Answer only after draining (lessons.md §2).
                let _ = sock.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
                if head.starts_with("POST") {
                    return body;
                }
            }
        });
        (format!("http://{addr}/v1"), seen)
    }

    /// The body one turn sends to an external server, with the section's "Model
    /// (opt.)" field set to `model_name`.
    async fn external_chat_body(model_name: Option<&str>) -> String {
        let (url, seen) = chat_body_stub();
        let mut settings = external(Some(&url));
        settings.external.model_name = model_name.map(String::from);
        let (tx, _rx) = unbounded_channel();
        let setup = LlamaSupervisor::default().apply_chat(
            &settings,
            None,
            CancellationToken::new(),
            tx,
            ru(),
        );
        let backend = setup.backend.expect("external mode yields a backend");
        // The stream is never polled — the request is sent before it exists, and
        // that request is the whole subject of the test.
        let _ = backend
            .chat_stream(
                crate::shared::api::ChatRequest {
                    messages: vec![crate::shared::api::ApiMessage::user("hi")],
                    ..Default::default()
                },
                CancellationToken::new(),
            )
            .await;
        tokio::task::spawn_blocking(move || seen.join().unwrap())
            .await
            .unwrap()
    }

    /// The configured name goes **on the wire**, which is what a multi-model
    /// endpoint routes on (llama.cpp's router mode, LiteLLM, OpenRouter answer
    /// `400 "model name is missing from the request"` without it). Until
    /// docs/research/external-model-name.md §2.3 the value never left settings.
    #[tokio::test]
    async fn external_chat_sends_the_configured_model() {
        let body = external_chat_body(Some("qwen-3.6-27b")).await;
        assert!(
            body.contains(r#""model":"qwen-3.6-27b""#),
            "the configured name must reach the request: {body}"
        );
    }

    /// …and a blank field still sends no `model` key at all — byte-identical to
    /// the request shape that shipped before, which is what keeps a bare local
    /// `llama-server` unaffected.
    #[tokio::test]
    async fn a_blank_model_field_sends_no_model_key() {
        for blank in [None, Some("")] {
            let body = external_chat_body(blank).await;
            assert!(
                !body.contains(r#""model""#),
                "blank={blank:?} must send no model key: {body}"
            );
        }
    }

    /// The embedding slot resolves its own key the same way. A second slot rather than
    /// a second mechanism: the point is that `apply_embed` now reads the key it is
    /// handed instead of only the variable named in settings.
    #[tokio::test]
    async fn external_embeddings_send_the_stored_key() {
        let (url, seen) = auth_probe_stub();
        let (tx, _rx) = unbounded_channel();
        let setup = LlamaSupervisor::default().apply_embed(
            &embed_external(&url),
            Some("sk-embed"),
            CancellationToken::new(),
            tx,
            ru(),
        );
        assert_eq!(setup.status, ServerStatus::Connecting);
        assert_eq!(
            header(seen).await,
            "authorization: bearer sk-embed",
            "the embedding probe must carry the slot's stored key"
        );
    }

    #[tokio::test]
    async fn superseded_probe_sends_no_status() {
        // A probe marked stale (mode switch managed→external→openai) shouldn't send a
        // status: otherwise a late timeout of the previous external would overwrite
        // `Ready` of the new cloud server, and the cloud "wouldn't work" until a
        // restart.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancellationToken::new();
        cancel.cancel(); // the probe is already stale before the background task starts
        let setup = external_chat_setup(
            Some("http://127.0.0.1:9/v1"),
            None,
            None,
            None,
            cancel,
            tx,
            ru(),
        );
        assert_eq!(setup.status, ServerStatus::Connecting); // the immediate status as usual
        // Give the background task a chance to run; a stale probe sends nothing.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(rx.try_recv().is_err(), "a stale probe sent a status");
    }

    #[tokio::test]
    async fn external_without_url_is_not_configured() {
        let (tx, _rx) = unbounded_channel();
        let setup = LlamaSupervisor::default().apply_chat(
            &external(None),
            None,
            CancellationToken::new(),
            tx,
            ru(),
        );
        assert!(setup.backend.is_none());
        assert_eq!(setup.status, ServerStatus::NotConfigured);
    }

    #[tokio::test]
    async fn managed_without_binary_is_not_configured() {
        let (tx, _rx) = unbounded_channel();
        let s = EngineSettings {
            mode: ServerMode::Managed,
            ..Default::default()
        };
        let setup =
            LlamaSupervisor::default().apply_chat(&s, None, CancellationToken::new(), tx, ru());
        assert_eq!(setup.status, ServerStatus::NotConfigured);
    }

    #[tokio::test]
    async fn managed_with_bogus_binary_is_disconnected() {
        // A model file that exists: without one the answer would be
        // `NotConfigured` (no model is nothing to run), and the binary — the thing
        // this test is about — would never be reached.
        let model = tempfile::NamedTempFile::new().unwrap();
        let (tx, _rx) = unbounded_channel();
        let s = EngineSettings {
            mode: ServerMode::Managed,
            managed: ManagedSettings {
                binary: Some("definitely-not-a-real-binary-xyz".into()),
                model_path: Some(model.path().display().to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let setup =
            LlamaSupervisor::default().apply_chat(&s, None, CancellationToken::new(), tx, ru());
        assert!(setup.backend.is_none());
        assert!(matches!(setup.status, ServerStatus::Disconnected(_)));
    }

    #[tokio::test]
    async fn managed_with_missing_model_is_disconnected() {
        // The binary is present (spawn would succeed), but the model file is
        // missing: previously this hung the UI at "connecting…" until the
        // timeout; now — an immediate `Disconnected` with a clear message.
        let (tx, _rx) = unbounded_channel();
        let s = EngineSettings {
            mode: ServerMode::Managed,
            managed: ManagedSettings {
                binary: Some("llama-server".into()),
                model_path: Some("no/such/model.gguf".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let setup =
            LlamaSupervisor::default().apply_chat(&s, None, CancellationToken::new(), tx, ru());
        assert!(setup.backend.is_none());
        match setup.status {
            ServerStatus::Disconnected(msg) => assert!(msg.contains("файл модели"), "{msg}"),
            other => panic!("expected Disconnected, got {other:?}"),
        }
    }

    /// The projector reaches the child process, and a typo in it is reported the
    /// same way a missing GGUF is — rather than starting a server that quietly has
    /// no vision and only fails much later, at the first image send.
    #[tokio::test]
    async fn managed_carries_the_projector_and_reports_a_missing_one() {
        const MISSING: &str = "no/such/mmproj.gguf";
        // A model that exists, so the two refusals ahead of the projector's are out
        // of the way: an unset model is `NotConfigured` (it starts a *router*, see
        // `ManagedConfig::is_runnable`), and a model file that is named but absent
        // is its own `Disconnected`.
        let model = tempfile::NamedTempFile::new().unwrap();
        let managed = ManagedSettings {
            binary: Some("llama-server".into()),
            model_path: Some(model.path().display().to_string()),
            mmproj: Some(MISSING.into()),
            ..Default::default()
        };
        assert_eq!(
            managed_config(&managed, ManagedRole::Assistant, &BinaryLookup::default())
                .mmproj
                .as_deref(),
            Some(MISSING),
            "the setting must reach the launch config"
        );

        let (tx, _rx) = unbounded_channel();
        let s = EngineSettings {
            mode: ServerMode::Managed,
            managed,
            ..Default::default()
        };
        let setup =
            LlamaSupervisor::default().apply_chat(&s, None, CancellationToken::new(), tx, ru());
        assert!(setup.backend.is_none());
        match setup.status {
            ServerStatus::Disconnected(msg) => {
                let expected = ru().tf("ui.err.managed.mmproj_not_found", &[("path", MISSING)]);
                assert!(msg.contains(&expected), "{msg}");
            }
            other => panic!("expected Disconnected, got {other:?}"),
        }
    }

    /// The raw arguments reach both launch configs with the role each is judged
    /// by, and a refused one stops the managed chat server at the supervisor's
    /// real seam — the path `settings.json`, `--set` and a restore take, none of
    /// which passes the settings editor (docs/research/managed-extra-args.md F5).
    #[tokio::test]
    async fn extra_arguments_reach_the_launch_and_a_refused_one_stops_it() {
        let model = tempfile::NamedTempFile::new().unwrap();
        let managed = ManagedSettings {
            binary: Some("llama-server".into()),
            model_path: Some(model.path().display().to_string()),
            extra_args: vec!["--n-cpu-moe".into(), "20".into()],
            ..Default::default()
        };
        let cfg = managed_config(
            &managed,
            ManagedRole::Impersonation,
            &BinaryLookup::default(),
        );
        assert_eq!(cfg.extra_args, ["--n-cpu-moe", "20"]);
        assert_eq!(cfg.role, ManagedRole::Impersonation);
        let embed = ManagedEmbedSettings {
            extra_args: vec!["-c".into(), "4096".into()],
            ..Default::default()
        };
        let cfg = managed_embed_config(&embed, "llama-server".into());
        assert_eq!(
            (cfg.role, cfg.extra_args),
            (ManagedRole::Embedder, embed.extra_args)
        );

        let (tx, _rx) = unbounded_channel();
        let s = EngineSettings {
            mode: ServerMode::Managed,
            managed: ManagedSettings {
                extra_args: vec!["--api-key".into(), "k".into()],
                ..managed
            },
            ..Default::default()
        };
        let setup =
            LlamaSupervisor::default().apply_chat(&s, None, CancellationToken::new(), tx, ru());
        assert!(setup.backend.is_none() && setup.handle.is_none());
        match setup.status {
            ServerStatus::Disconnected(msg) => {
                assert!(
                    msg.contains("--api-key") && msg.contains("«Внешний»"),
                    "{msg}"
                )
            }
            other => panic!("expected Disconnected, got {other:?}"),
        }
    }

    #[test]
    fn resolve_api_key_reads_env_and_reports_missing() {
        // PATH is set on every OS — a guaranteed positive case with no env mutation.
        assert!(resolve_api_key(None, Some("PATH")).is_ok());
        assert!(matches!(
            resolve_api_key(None, None),
            Err(ApiKeyError::NoName)
        ));
        match resolve_api_key(None, Some("MINDFORK_DEFINITELY_UNSET_VAR_42")) {
            Err(ApiKeyError::Missing(var)) => assert_eq!(var, "MINDFORK_DEFINITELY_UNSET_VAR_42"),
            _ => panic!("expected ApiKeyError::Missing"),
        }
    }

    /// A saved key takes priority over env (decision point R3) and works on its own —
    /// without the name of an env variable, which the target user never sets at all.
    #[test]
    fn stored_key_wins_over_env_and_works_without_it() {
        // Both a saved key and env exist → we take the saved one.
        assert_eq!(
            resolve_api_key(Some("sk-stored"), Some("PATH")).unwrap(),
            "sk-stored"
        );
        // Only the saved one (the env variable isn't set at all) → it's used.
        assert_eq!(
            resolve_api_key(Some("sk-stored"), None).unwrap(),
            "sk-stored"
        );
        // A missing env variable doesn't get in the way of the saved key.
        assert_eq!(
            resolve_api_key(Some("sk-stored"), Some("MINDFORK_DEFINITELY_UNSET_VAR_42")).unwrap(),
            "sk-stored"
        );
        // An empty saved key is equivalent to absence → fallback to env.
        assert!(resolve_api_key(Some(""), Some("PATH")).is_ok());
        assert!(matches!(
            resolve_api_key(Some(""), None),
            Err(ApiKeyError::NoName)
        ));
    }

    /// Live: an external server that **requires** a key is reachable on a key entered
    /// in settings alone, with nothing in the environment — the whole point of
    /// docs/history/external-api-key.md. The control arm is what makes it mean anything: the
    /// same settings with no key must *fail*, or the smoke would pass against a server
    /// that never checked (`llama-server` without `--api-key`, a stray env variable).
    ///
    /// Stack: `MINDFORK_ENGINE_URL` pointed at a server started with
    /// `--api-key <MINDFORK_ENGINE_KEY>`.
    #[tokio::test]
    #[ignore = "requires an authenticated external server (MINDFORK_ENGINE_URL started with --api-key MINDFORK_ENGINE_KEY)"]
    async fn external_authenticated_server_takes_the_stored_key_live() {
        use crate::shared::api::ChatChunk;
        use futures_util::StreamExt;
        let (Ok(url), Ok(key)) = (
            std::env::var("MINDFORK_ENGINE_URL"),
            std::env::var("MINDFORK_ENGINE_KEY"),
        ) else {
            eprintln!("skip: MINDFORK_ENGINE_URL/MINDFORK_ENGINE_KEY not set");
            return;
        };
        // `api_key_env` deliberately unset: only a key stored for this slot can make
        // the turn work, so the env path cannot be what is being measured.
        let settings = external(Some(&url));
        assert_eq!(settings.external.api_key_env, None);

        let turn = |stored: Option<&str>| {
            let (tx, _rx) = unbounded_channel();
            let setup = LlamaSupervisor::default().apply_chat(
                &settings,
                stored,
                CancellationToken::new(),
                tx,
                ru(),
            );
            let backend = setup.backend.expect("external mode yields a backend");
            async move {
                let req = crate::shared::api::ChatRequest {
                    continue_final: false,
                    system: None,
                    messages: vec![crate::shared::api::ApiMessage::user("Say OK.".to_string())],
                    sampling: crate::entities::sampling::SamplingConfig {
                        // Not 16. What is under test is whether the *stored key*
                        // authenticates the turn, and on a reasoning model a
                        // ceiling that tight is spent entirely in the thinking
                        // channel, leaving empty text and a red smoke that says
                        // nothing about keys (measured on gpt-oss-120b; the same
                        // shape as the Qwen ceiling in
                        // docs/history/e2e-second-chat-model.md §2). Muting is not
                        // the fix here: `reasoning_budget: 0` is a no-op on the
                        // harmony template.
                        max_tokens: Some(512),
                        ..Default::default()
                    },
                    tools: Vec::new(),
                };
                let mut stream = backend.chat_stream(req, CancellationToken::new()).await?;
                let mut text = String::new();
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        ChatChunk::Text(t) => text.push_str(&t),
                        ChatChunk::Error { message, .. } => anyhow::bail!("stream: {message}"),
                        _ => {}
                    }
                }
                Ok::<String, anyhow::Error>(text)
            }
        };

        let with_key = turn(Some(&key)).await;
        eprintln!("live: stored key -> {with_key:?}");
        let answer = with_key.expect("the stored key must authenticate the turn");
        assert!(
            !answer.trim().is_empty(),
            "authenticated turn produced no text"
        );

        // Control: no key anywhere → the server refuses. If this *succeeds*, the
        // server is not enforcing a key and the arm above proved nothing.
        let without = turn(None).await;
        eprintln!("live: no key -> {without:?}");
        assert!(
            without.is_err(),
            "the server accepted an unauthenticated turn — it is not enforcing \
             --api-key, so this smoke measured nothing"
        );
    }

    /// A cloud mode comes up on a single saved key — without `api_key_env`
    /// (the feature's main scenario: the user entered a key in settings).
    #[tokio::test]
    async fn cloud_chat_with_stored_key_and_no_env_is_ready() {
        let (tx, _rx) = unbounded_channel();
        let s = EngineSettings {
            mode: ServerMode::OpenAi,
            openai: crate::shared::config::CloudSettings {
                model_name: Some("gpt-5.5".into()),
                api_key_env: None, // the env-variable name isn't set at all
                ..Default::default()
            },
            ..Default::default()
        };
        let setup = LlamaSupervisor::default().apply_chat(
            &s,
            Some("sk-stored"),
            CancellationToken::new(),
            tx,
            ru(),
        );
        assert_eq!(setup.status, ServerStatus::Ready);
        assert!(setup.backend.is_some());
    }

    #[tokio::test]
    async fn cloud_chat_without_model_is_disconnected() {
        let (tx, _rx) = unbounded_channel();
        let s = EngineSettings {
            mode: ServerMode::OpenAi,
            openai: crate::shared::config::CloudSettings {
                api_key_env: Some("PATH".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        match LlamaSupervisor::default()
            .apply_chat(&s, None, CancellationToken::new(), tx, ru())
            .status
        {
            ServerStatus::Disconnected(m) => assert!(m.contains("модел"), "{m}"),
            other => panic!("expected Disconnected, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cloud_chat_missing_key_env_is_disconnected() {
        let (tx, _rx) = unbounded_channel();
        let s = EngineSettings {
            mode: ServerMode::OpenAi,
            openai: crate::shared::config::CloudSettings {
                model_name: Some("gpt-4o".into()),
                api_key_env: Some("MINDFORK_DEFINITELY_UNSET_VAR_42".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        match LlamaSupervisor::default()
            .apply_chat(&s, None, CancellationToken::new(), tx, ru())
            .status
        {
            ServerStatus::Disconnected(m) => {
                assert!(m.contains("MINDFORK_DEFINITELY_UNSET_VAR_42"), "{m}")
            }
            other => panic!("expected Disconnected, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cloud_chat_with_model_and_key_is_ready() {
        // We use PATH as the "key": all that matters is that the env variable
        // resolves.
        let (tx, _rx) = unbounded_channel();
        let s = EngineSettings {
            mode: ServerMode::Gemini,
            gemini: crate::shared::config::CloudSettings {
                model_name: Some("gemini-2.5-pro".into()),
                api_key_env: Some("PATH".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let setup =
            LlamaSupervisor::default().apply_chat(&s, None, CancellationToken::new(), tx, ru());
        assert!(setup.backend.is_some());
        assert!(setup.handle.is_none(), "the cloud has no child process");
        assert_eq!(setup.status, ServerStatus::Ready);
    }

    #[tokio::test]
    async fn cloud_chat_claude_with_model_and_key_is_ready() {
        // Claude uses a separate protocol (AnthropicClient), but the setup contract is
        // the same: model + key from env → Ready, with no child process.
        let (tx, _rx) = unbounded_channel();
        let s = EngineSettings {
            mode: ServerMode::Claude,
            claude: crate::shared::config::CloudSettings {
                model_name: Some("claude-opus-4-8".into()),
                api_key_env: Some("PATH".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let setup =
            LlamaSupervisor::default().apply_chat(&s, None, CancellationToken::new(), tx, ru());
        assert!(setup.backend.is_some());
        assert!(setup.handle.is_none());
        assert_eq!(setup.status, ServerStatus::Ready);
    }

    /// Grok is the one cloud served by `OpenAiClient` rather than a protocol-specific
    /// client, so this pins the setup contract it shares with the others: model + key
    /// → `Ready`, no child process, no probe.
    #[tokio::test]
    async fn cloud_chat_grok_with_model_and_key_is_ready() {
        let (tx, _rx) = unbounded_channel();
        let s = EngineSettings {
            mode: ServerMode::Grok,
            grok: crate::shared::config::CloudSettings {
                model_name: Some("grok-4.5".into()),
                api_key_env: Some("PATH".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let setup =
            LlamaSupervisor::default().apply_chat(&s, None, CancellationToken::new(), tx, ru());
        assert!(setup.backend.is_some());
        assert!(setup.handle.is_none());
        assert_eq!(setup.status, ServerStatus::Ready);
    }

    #[tokio::test]
    async fn grok_embed_is_unavailable() {
        // xAI ships no embedding model (`/v1/embedding-models` is empty) — RAG is
        // unavailable, exactly as for Anthropic.
        let s = EmbedSettings {
            mode: ServerMode::Grok,
            grok: crate::shared::config::CloudSettings {
                model_name: Some("x".into()),
                api_key_env: Some("PATH".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let err = embed_setup(&s)
            .embedder
            .embed(vec!["x".into()], EmbedRole::Passage)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not configured"));
    }

    #[tokio::test]
    async fn claude_embed_is_unavailable() {
        // Anthropic has no embeddings — RAG is unavailable (like other unavailable
        // cases).
        let s = EmbedSettings {
            mode: ServerMode::Claude,
            claude: crate::shared::config::CloudSettings {
                model_name: Some("x".into()),
                api_key_env: Some("PATH".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let err = embed_setup(&s)
            .embedder
            .embed(vec!["x".into()], EmbedRole::Passage)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not configured"));
    }

    #[tokio::test]
    async fn cloud_embed_unconfigured_is_unavailable() {
        // Cloud embeddings without a model → RAG unavailable (like other unavailable
        // cases).
        let s = EmbedSettings {
            mode: ServerMode::OpenAi,
            openai: crate::shared::config::CloudSettings {
                api_key_env: Some("PATH".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let setup = embed_setup(&s);
        let err = setup
            .embedder
            .embed(vec!["x".into()], EmbedRole::Passage)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not configured"));
    }

    /// Going down needs corroboration; coming up doesn't. `FAILURES_TO_UNHEALTHY - 1`
    /// failures must **not** flip the verdict — that's the whole point of hysteresis,
    /// and it's the assertion that fails if someone "simplifies" the counter away.
    #[test]
    fn health_flips_down_only_after_a_streak_and_up_at_once() {
        let mut h = Health::new(true);
        for _ in 0..FAILURES_TO_UNHEALTHY - 1 {
            assert_eq!(h.record(false), None, "flipped before the streak completed");
        }
        assert_eq!(
            h.record(false),
            Some(false),
            "the streak should flip it down"
        );
        assert_eq!(
            h.record(true),
            Some(true),
            "one success should bring it back"
        );
    }

    /// A success mid-streak clears it: three failures spread across a healthy day
    /// aren't a streak.
    #[test]
    fn health_success_resets_the_streak() {
        let mut h = Health::new(true);
        h.record(false);
        assert_eq!(h.record(true), None, "still healthy — nothing to publish");
        for _ in 0..FAILURES_TO_UNHEALTHY - 1 {
            assert_eq!(h.record(false), None);
        }
        assert_eq!(h.record(false), Some(false));
    }

    /// A steady server publishes nothing — otherwise it would wake the UI every
    /// minute forever.
    #[test]
    fn health_publishes_only_on_a_flip() {
        let mut h = Health::new(true);
        assert_eq!(h.record(true), None);
        let mut down = Health::new(false);
        assert_eq!(down.record(false), None);
    }

    /// The cadence follows suspicion, not just state: a pending streak polls fast, so
    /// hysteresis costs ~10s of detection latency rather than ~3 minutes.
    #[test]
    fn health_polls_fast_while_anything_looks_wrong() {
        assert_eq!(Health::new(true).poll_delay(), HEALTHY_POLL);
        assert_eq!(Health::new(false).poll_delay(), RECHECK_POLL);
        let mut pending = Health::new(true);
        pending.record(false);
        assert_eq!(
            pending.poll_delay(),
            RECHECK_POLL,
            "a pending failure streak must not wait a full healthy interval"
        );
    }

    /// The whole point of the feature, end to end: a server that was `Ready` goes
    /// away, the monitor notices *on its own* (nobody asked it to), and when the
    /// server comes back it recovers *on its own* too — no restart, no settings edit.
    /// Time is paused, so the 60s/5s intervals cost nothing; the stub hangs up rather
    /// than closing the port, so a failing probe is instant.
    #[tokio::test(start_paused = true)]
    async fn monitor_notices_a_server_going_down_and_coming_back() {
        let (url, switch) = spawn_stub_server(true).await;
        let (tx, mut rx) = unbounded_channel();
        LlamaSupervisor::default().apply_embed(
            &embed_external(&url),
            None,
            CancellationToken::new(),
            tx,
            ru(),
        );
        assert_eq!(
            rx.recv().await,
            Some(ServerStatus::Ready),
            "initial verdict"
        );

        switch.store(false, Ordering::SeqCst); // the server goes away
        match rx.recv().await {
            Some(ServerStatus::Disconnected(_)) => {}
            other => panic!("expected the monitor to notice the outage, got {other:?}"),
        }

        switch.store(true, Ordering::SeqCst); // …and comes back
        assert_eq!(
            rx.recv().await,
            Some(ServerStatus::Ready),
            "the monitor should recover without a restart"
        );
    }

    /// A steady server publishes nothing after its first verdict — the monitor must
    /// not wake the UI on every poll. (Paused time makes "a while" free: 10 minutes of
    /// virtual time is ~10 healthy polls.)
    #[tokio::test(start_paused = true)]
    async fn monitor_stays_quiet_while_the_server_is_steady() {
        let (url, _switch) = spawn_stub_server(true).await;
        let (tx, mut rx) = unbounded_channel();
        LlamaSupervisor::default().apply_embed(
            &embed_external(&url),
            None,
            CancellationToken::new(),
            tx,
            ru(),
        );
        assert_eq!(rx.recv().await, Some(ServerStatus::Ready));
        tokio::time::sleep(HEALTHY_POLL * 10).await;
        assert!(
            rx.try_recv().is_err(),
            "a steady server should publish nothing after its first verdict"
        );
    }

    /// The regression: a configured embedding server must not report `Ready` on the
    /// strength of a non-empty URL alone. Before the probe, a configured-but-
    /// unreachable host showed a green chip, and the failure only surfaced on the
    /// first `rag_search`.
    #[tokio::test]
    async fn embed_external_is_connecting_not_ready() {
        let setup = embed_setup(&embed_external("http://127.0.0.1:9/v1"));
        assert_eq!(setup.status, ServerStatus::Connecting);
    }

    /// …and the probe actually reports through the channel — here, the happy path:
    /// a local listener answers `/health` with `200`, so `Ready` arrives.
    #[tokio::test]
    async fn embed_probe_reports_ready_when_server_answers() {
        let (url, _switch) = spawn_stub_server(true).await;
        let (tx, mut rx) = unbounded_channel();
        let setup = LlamaSupervisor::default().apply_embed(
            &embed_external(&url),
            None,
            CancellationToken::new(),
            tx,
            ru(),
        );
        assert_eq!(setup.status, ServerStatus::Connecting);
        assert_eq!(rx.recv().await, Some(ServerStatus::Ready));
    }

    /// The other half: a server that accepts and hangs up answers nothing, and the
    /// probe reports `Disconnected` rather than leaving the chip green. Time is paused
    /// so the retry sleeps are virtual; the connection itself fails fast because the
    /// port *is* listening (a connect to a closed port costs seconds on Windows).
    #[tokio::test(start_paused = true)]
    async fn embed_probe_reports_disconnected_when_server_is_silent() {
        let (url, _switch) = spawn_stub_server(false).await;
        let (tx, mut rx) = unbounded_channel();
        let setup = LlamaSupervisor::default().apply_embed(
            &embed_external(&url),
            None,
            CancellationToken::new(),
            tx,
            ru(),
        );
        assert_eq!(setup.status, ServerStatus::Connecting);
        match rx.recv().await {
            Some(ServerStatus::Disconnected(_)) => {}
            other => panic!("expected Disconnected from the probe, got {other:?}"),
        }
    }

    /// A stale embeddings probe doesn't overwrite the new server's status — the same
    /// invalidation the chat server has (a quick sequence of settings edits).
    #[tokio::test(start_paused = true)]
    async fn embed_superseded_probe_sends_no_status() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancellationToken::new();
        cancel.cancel(); // already stale before the background task starts
        let setup = LlamaSupervisor::default().apply_embed(
            &embed_external("http://127.0.0.1:9/v1"),
            None,
            cancel,
            tx,
            ru(),
        );
        assert_eq!(setup.status, ServerStatus::Connecting);
        tokio::time::sleep(Duration::from_secs(60)).await; // well past the probe timeout
        assert!(rx.try_recv().is_err(), "a stale probe sent a status");
    }

    /// A managed embedding server that can't even be launched (a missing model file)
    /// reports the reason instead of masquerading as "not configured".
    #[tokio::test]
    async fn embed_managed_with_missing_model_is_disconnected() {
        let s = EmbedSettings {
            mode: ServerMode::Managed,
            managed: crate::shared::config::ManagedEmbedSettings {
                binary: Some("llama-server".into()),
                model_path: Some("no/such/model.gguf".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        match embed_setup(&s).status {
            ServerStatus::Disconnected(msg) => assert!(msg.contains("файл модели"), "{msg}"),
            other => panic!("expected Disconnected, got {other:?}"),
        }
    }

    /// The cloud has nothing to load and no `/health` — it stays `Ready` right away,
    /// with no probe (as for the chat server).
    #[tokio::test(start_paused = true)]
    async fn cloud_embed_configured_is_ready_without_probe() {
        let (tx, mut rx) = unbounded_channel();
        let s = EmbedSettings {
            mode: ServerMode::OpenAi,
            openai: crate::shared::config::CloudSettings {
                model_name: Some("text-embedding-3-small".into()),
                api_key_env: Some("PATH".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let setup =
            LlamaSupervisor::default().apply_embed(&s, None, CancellationToken::new(), tx, ru());
        assert_eq!(setup.status, ServerStatus::Ready);
        assert!(setup.handle.is_none(), "the cloud has no child process");
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert!(rx.try_recv().is_err(), "the cloud shouldn't be probed");
    }

    /// The one thing unit tests can't settle: that a **real** `llama-server
    /// --embeddings` answers the `/health` endpoint the probe relies on. If it
    /// didn't, the probe would mark a perfectly working embedder as unavailable —
    /// a worse failure than the green-chip bug it fixes. (`probe` treats `404` as
    /// alive, so a server without `/health` is fine too; this pins the real one.)
    ///
    ///     MINDFORK_EMBED_URL=http://127.0.0.1:8001/v1 \
    ///       cargo test embed_probe_reaches_ready_on_live_server -- --ignored --nocapture
    /// The PID listening on `port`, via `netstat` (Windows). `None` if nothing is.
    #[cfg(windows)]
    fn pid_on_port(port: u16) -> Option<u32> {
        let out = std::process::Command::new("netstat")
            .args(["-ano", "-p", "TCP"])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.lines()
            .filter(|l| l.contains("LISTENING") && l.contains(&format!(":{port} ")))
            .find_map(|l| l.split_whitespace().last()?.parse().ok())
    }

    /// A **real** `llama-server` and no model: nothing starts, and the port stays
    /// free. The refusal exists because of what this binary does *without* `-m` —
    /// it comes up as a router whose `/health` says `ok` while every completion is
    /// a `400`, and whose `models_autoload` would fetch a model from Hugging Face
    /// on a name match (docs/research/robustness-and-defaults.md §2.1). The unit
    /// tests can only assert the decision; this asserts that no process appears.
    ///
    ///     MINDFORK_LLAMA_BIN=.../llama-server.exe MINDFORK_MODEL=.../model.gguf \
    ///       cargo test managed_without_a_model_starts_nothing_e2e_live -- --ignored --nocapture
    #[tokio::test]
    #[cfg(windows)]
    #[ignore = "requires a local llama-server binary + model (MINDFORK_LLAMA_BIN, MINDFORK_MODEL)"]
    async fn managed_without_a_model_starts_nothing_e2e_live() {
        let (Ok(bin), Ok(model)) = (
            std::env::var("MINDFORK_LLAMA_BIN"),
            std::env::var("MINDFORK_MODEL"),
        ) else {
            eprintln!("skip: MINDFORK_LLAMA_BIN / MINDFORK_MODEL not set");
            return;
        };
        const PORT: u16 = 18097;
        let settings = |model: Option<String>| EngineSettings {
            mode: ServerMode::Managed,
            managed: ManagedSettings {
                binary: Some(bin.clone()),
                model_path: model,
                port: PORT,
                gpu_layers: 0,
                context_size: 4096,
                ..Default::default()
            },
            ..Default::default()
        };

        // Arm 1 — no model.
        let (tx, _rx) = unbounded_channel();
        let refused = LlamaSupervisor::default().apply_chat(
            &settings(None),
            None,
            CancellationToken::new(),
            tx,
            ru(),
        );
        assert_eq!(refused.status, ServerStatus::NotConfigured);
        assert!(refused.handle.is_none(), "no child is owned");
        // The port is the assertion the unit tests cannot make: a router would be
        // listening here, answering `/health` with `ok`.
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(
            pid_on_port(PORT),
            None,
            "nothing must be listening on {PORT}"
        );

        // Arm 2 — the same settings with the model: a server, as before.
        let (tx, mut rx) = unbounded_channel();
        let started = LlamaSupervisor::default().apply_chat(
            &settings(Some(model)),
            None,
            CancellationToken::new(),
            tx,
            ru(),
        );
        let _handle = started.handle.expect("a managed server owns its child");
        let status = rx.recv().await;
        println!("with a model: {status:?}");
        assert_eq!(status, Some(ServerStatus::Ready), "the model loaded");
        assert!(pid_on_port(PORT).is_some(), "the server is listening");
    }

    /// The reported defect, end to end: a managed server with **"No mmap" ticked**
    /// must come up. It did not — `llama-server` answered
    /// `error: invalid argument: --no-mmap` and exited, because llama.cpp removed
    /// the flag on 2026-09-09 (`14a9d09f7`) in favour of `--load-mode`, and even
    /// `b10883`, the build `mindfork llama setup` installs, no longer takes it.
    ///
    /// The arm that matters is the *first* one: the unit tests can pin which
    /// spelling is chosen, but only a real binary can say whether it accepts it.
    ///
    ///     MINDFORK_LLAMA_BIN=.../llama-server.exe MINDFORK_MODEL=.../model.gguf \
    ///       cargo test managed_without_mmap_starts_e2e_live -- --ignored --nocapture
    #[tokio::test]
    #[cfg(windows)]
    #[ignore = "requires a local llama-server binary + model (MINDFORK_LLAMA_BIN, MINDFORK_MODEL)"]
    async fn managed_without_mmap_starts_e2e_live() {
        let (Ok(bin), Ok(model)) = (
            std::env::var("MINDFORK_LLAMA_BIN"),
            std::env::var("MINDFORK_MODEL"),
        ) else {
            eprintln!("skip: MINDFORK_LLAMA_BIN / MINDFORK_MODEL not set");
            return;
        };
        const PORT: u16 = 18098;
        // What the binary itself says it takes — printed, because this is the
        // measurement the fix rests on.
        let spelling = crate::shared::api::managed::no_mmap_spelling(std::path::Path::new(&bin));
        println!("{bin} spells no-mmap as {spelling:?}");

        let settings = EngineSettings {
            mode: ServerMode::Managed,
            managed: ManagedSettings {
                binary: Some(bin),
                model_path: Some(model),
                port: PORT,
                gpu_layers: 0,
                context_size: 4096,
                no_mmap: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let (tx, mut rx) = unbounded_channel();
        let started = LlamaSupervisor::default().apply_chat(
            &settings,
            None,
            CancellationToken::new(),
            tx,
            ru(),
        );
        let _handle = started
            .handle
            .expect("a managed server owns its child even with the box ticked");
        let status = rx.recv().await;
        println!("with no mmap: {status:?}");
        assert_eq!(
            status,
            Some(ServerStatus::Ready),
            "the server must come up with the setting on"
        );
        assert!(pid_on_port(PORT).is_some(), "and be listening");
    }

    /// A **real** managed child that dies on its own must be noticed at once — from
    /// the exit signal, not by waiting out a probe. That's the entire reason the
    /// monitor watches `exited` alongside its timer.
    ///
    /// It's killed from *outside* deliberately: dropping the handle would be **us**
    /// stopping the server, which the process monitor treats differently (and
    /// rightly so — that's what a re-`apply` does, and it must stay silent). An
    /// earlier version of this test dropped the handle and measured 76 s: the exit
    /// signal never fired and the monitor found out by probing (60 s healthy poll +
    /// 3 × 5 s recheck). Wrong premise, right code — but worth keeping written down.
    ///
    ///     MINDFORK_LLAMA_BIN=.../llama-server.exe MINDFORK_EMBED_MODEL=.../bge-m3.gguf \
    ///       cargo test managed_child_death_is_noticed_at_once -- --ignored --nocapture
    #[tokio::test]
    #[cfg(windows)]
    #[ignore = "requires a local llama-server binary + model (MINDFORK_LLAMA_BIN, MINDFORK_EMBED_MODEL)"]
    async fn managed_child_death_is_noticed_at_once() {
        let (Ok(bin), Ok(model)) = (
            std::env::var("MINDFORK_LLAMA_BIN"),
            std::env::var("MINDFORK_EMBED_MODEL"),
        ) else {
            eprintln!("skip: MINDFORK_LLAMA_BIN / MINDFORK_EMBED_MODEL not set");
            return;
        };
        const PORT: u16 = 18099;
        let s = EmbedSettings {
            mode: ServerMode::Managed,
            managed: crate::shared::config::ManagedEmbedSettings {
                binary: Some(bin),
                model_path: Some(model),
                port: PORT,
                ..Default::default()
            },
            ..Default::default()
        };
        let (tx, mut rx) = unbounded_channel();
        let setup =
            LlamaSupervisor::default().apply_embed(&s, None, CancellationToken::new(), tx, ru());
        let _handle = setup.handle.expect("a managed server owns its child");
        assert_eq!(rx.recv().await, Some(ServerStatus::Ready), "model loaded");

        let pid = pid_on_port(PORT).expect("the server should be listening");
        let killed = std::time::Instant::now();
        std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output()
            .expect("taskkill");
        let status = rx.recv().await;
        let noticed = killed.elapsed();
        println!("child death noticed in {noticed:?}: {status:?}");
        assert!(
            matches!(status, Some(ServerStatus::Disconnected(_))),
            "a dead child must be reported, got {status:?}"
        );
        assert!(
            noticed < Duration::from_secs(10),
            "should come from the exit signal, not a probe — took {noticed:?}"
        );
    }

    /// Against a **real** server over a **real** network. The unit tests use a stub
    /// that answers instantly and never hiccups, so they can't speak to the failure
    /// mode that actually matters in the field: a status that flaps for no reason and
    /// teaches the user to ignore the chip. Runs in real time across a full healthy
    /// interval — slow by design.
    ///
    ///     MINDFORK_EMBED_URL=http://127.0.0.1:8001/v1 \
    ///       cargo test monitor_does_not_flap_against_a_live_server -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "requires a live embedding server (MINDFORK_EMBED_URL); takes ~70s"]
    async fn monitor_does_not_flap_against_a_live_server() {
        let Ok(url) = std::env::var("MINDFORK_EMBED_URL") else {
            eprintln!("skip: MINDFORK_EMBED_URL not set");
            return;
        };
        let (tx, mut rx) = unbounded_channel();
        LlamaSupervisor::default().apply_embed(
            &embed_external(&url),
            None,
            CancellationToken::new(),
            tx,
            ru(),
        );
        assert_eq!(
            rx.recv().await,
            Some(ServerStatus::Ready),
            "initial verdict"
        );
        let watch = HEALTHY_POLL + Duration::from_secs(10);
        println!("watching {url} for {watch:?} — any status published here is a flap");
        tokio::time::sleep(watch).await;
        match rx.try_recv() {
            Err(_) => println!("no flap: the monitor stayed quiet"),
            Ok(s) => panic!("the monitor flapped against a healthy server: {s:?}"),
        }
    }

    #[tokio::test]
    #[ignore = "requires a live embedding server (MINDFORK_EMBED_URL)"]
    async fn embed_probe_reaches_ready_on_live_server() {
        let Ok(url) = std::env::var("MINDFORK_EMBED_URL") else {
            eprintln!("skip: MINDFORK_EMBED_URL not set");
            return;
        };
        let (tx, mut rx) = unbounded_channel();
        let setup = LlamaSupervisor::default().apply_embed(
            &embed_external(&url),
            None,
            CancellationToken::new(),
            tx,
            ru(),
        );
        assert_eq!(setup.status, ServerStatus::Connecting);
        let status = rx.recv().await;
        println!("live embedding server {url} probed as: {status:?}");
        assert_eq!(status, Some(ServerStatus::Ready));
    }

    #[tokio::test]
    async fn embed_external_url_is_available() {
        let setup = embed_setup(&embed_external("http://127.0.0.1:9/v1"));
        assert!(setup.handle.is_none());
        // The embeddings source is configured (not UnavailableEmbedder).
        // Checked indirectly: embed against a "dead" URL returns a connection error,
        // while UnavailableEmbedder would return its fixed unavailability message.
        let err = setup
            .embedder
            .embed(vec!["x".into()], EmbedRole::Passage)
            .await
            .unwrap_err();
        assert!(!err.to_string().contains("не настроен"), "{err}");
    }

    #[tokio::test]
    async fn embed_unconfigured_is_unavailable() {
        let setup = embed_setup(&EmbedSettings::default());
        let err = setup
            .embedder
            .embed(vec!["x".into()], EmbedRole::Passage)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not configured"));
    }
}

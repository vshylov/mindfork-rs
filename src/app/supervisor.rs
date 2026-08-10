//! The inference/embeddings server supervisor for the orchestrator: (re)launches a
//! managed process or connects to an external one per [`EngineSettings`]/
//! [`EmbedSettings`] settings. Hidden behind the [`ServerSupervisor`] trait for a mock
//! in tests — changing the model in settings restarts the server (spec §11.6, DoD M8).
//!
//! Lives in `app`: it's composition glue that knows both about `shared/config`
//! (settings) and about `shared/api` (the engine/process launch) — both lower in FSD.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::shared::api::{
    AnthropicClient, Embedder, EngineBackend, GeminiClient, ManagedConfig, OpenAiClient,
    ResponsesClient, ServerHandle, UnavailableEmbedder, wait_until_ready,
};
use crate::shared::config::{
    CloudProvider, EmbedSettings, EngineSettings, ImpersonationEngineSettings, ImpersonationMode,
    ManagedSettings, ServerMode,
};
use crate::shared::i18n::Locale;
use crate::shared::server::ServerStatus;

/// A generous readiness timeout for the managed server: loading the model can take
/// minutes.
const MANAGED_READY_TIMEOUT: Duration = Duration::from_secs(600);
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
/// `stored_key` on every method is the already-decrypted saved API key of the active
/// cloud provider (`AppConfig::api_keys`, see `shared::secrets`); the resolution is
/// owned by [`super::orchestrator`], the supervisor knows nothing about the storage
/// format. `None` — the key isn't saved on this machine, then the env fallback
/// (`api_key_env`) applies. See docs/research/api-key-storage.md.
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

/// The production supervisor: external — by URL (any OpenAI server), managed —
/// a child `llama-server` process (llama.cpp).
pub struct LlamaSupervisor;

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
                settings.external.api_key_env.as_deref(),
                cancel,
                status_tx,
                loc,
            ),
            ServerMode::Managed => {
                managed_chat_setup(managed_config(&settings.managed), cancel, status_tx, loc)
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
                settings.external.api_key_env.as_deref(),
                cancel,
                status_tx,
                loc,
            ),
            ImpersonationMode::Managed => {
                managed_chat_setup(managed_config(&settings.managed), cancel, status_tx, loc)
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
                        OpenAiClient::new(url).with_api_key(
                            settings
                                .external
                                .api_key_env
                                .as_deref()
                                .and_then(|e| resolve_api_key(None, Some(e)).ok()),
                        ),
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
            ServerMode::Managed => match settings.managed.binary.as_deref() {
                Some(bin) if !bin.is_empty() => {
                    let m = &settings.managed;
                    let cfg = ManagedConfig {
                        binary: bin.into(),
                        model_path: m.model_path.clone(),
                        gpu_layers: m.gpu_layers,
                        context_size: crate::shared::config::DEFAULT_CONTEXT_SIZE,
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
                        extra_args: vec![],
                    };
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
/// probe. Optional `api_key_env` — the name of an env variable holding a Bearer key
/// (for an OpenAI-compatible authenticated proxy/gateway); a missing/unresolvable key
/// is not an error (a local `llama-server` needs no key). Saved keys
/// (`shared::secrets`) apply only to cloud providers: external has an arbitrary
/// URL that can't be bound to a provider, so the env path stays here
/// (docs/research/api-key-storage.md, decision point R4).
fn external_chat_setup(
    url: Option<&str>,
    api_key_env: Option<&str>,
    cancel: CancellationToken,
    status_tx: UnboundedSender<ServerStatus>,
    loc: &'static Locale,
) -> ChatSetup {
    match url {
        Some(url) if !url.is_empty() => {
            let key = api_key_env.and_then(|e| resolve_api_key(None, Some(e)).ok());
            let client = Arc::new(OpenAiClient::new(url).with_api_key(key));
            spawn_probe(
                client.clone(),
                EXTERNAL_READY_TIMEOUT,
                None,
                cancel,
                status_tx,
                loc,
            );
            ChatSetup {
                backend: Some(client),
                handle: None,
                status: ServerStatus::Connecting,
            }
        }
        _ => not_configured(),
    }
}

/// Managed chat setup: launch a child `llama-server`, a background probe (accounting
/// for an early process exit). An empty binary → `NotConfigured`.
fn managed_chat_setup(
    cfg: ManagedConfig,
    cancel: CancellationToken,
    status_tx: UnboundedSender<ServerStatus>,
    loc: &'static Locale,
) -> ChatSetup {
    if cfg.binary.as_os_str().is_empty() {
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

/// Builds a [`ManagedConfig`] (`llama-server`) from the engine's managed subsection
/// (shared by the assistant's chat server and the impersonation server).
fn managed_config(s: &ManagedSettings) -> ManagedConfig {
    ManagedConfig {
        binary: s.binary.clone().unwrap_or_default().into(),
        model_path: s.model_path.clone(),
        gpu_layers: s.gpu_layers,
        context_size: s.context_size,
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
        extra_args: vec![],
    }
}

/// A structured API-key resolution error (no locale — the caller localizes it
/// itself, see [`cloud_chat_setup`]). `NoName` — the key isn't saved and the name of
/// the env variable isn't set; `Missing` carries the name of a variable missing from
/// the environment.
#[derive(Debug)]
pub(super) enum ApiKeyError {
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
fn resolve_api_key(stored: Option<&str>, api_key_env: Option<&str>) -> Result<String, ApiKeyError> {
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
        backend: Some(backend),
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
    exited: Option<CancellationToken>,
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
                _ = wait_for_exit(&exited) => {
                    let _ = status_tx.send(ServerStatus::Disconnected(
                        loc.t("ui.err.managed.early_exit").to_string(),
                    ));
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
async fn wait_for_exit(exited: &Option<CancellationToken>) {
    match exited {
        Some(token) => token.cancelled().await,
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
}

#[cfg(test)]
impl ServerSupervisor for MockSupervisor {
    fn apply_chat(
        &self,
        _settings: &EngineSettings,
        _stored_key: Option<&str>,
        _cancel: CancellationToken,
        _status_tx: UnboundedSender<ServerStatus>,
        _loc: &'static Locale,
    ) -> ChatSetup {
        self.chat_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::mpsc::unbounded_channel;

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
        LlamaSupervisor.apply_embed(s, None, CancellationToken::new(), tx, ru())
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
        let setup = LlamaSupervisor.apply_chat(
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

    #[tokio::test]
    async fn superseded_probe_sends_no_status() {
        // A probe marked stale (mode switch managed→external→openai) shouldn't send a
        // status: otherwise a late timeout of the previous external would overwrite
        // `Ready` of the new cloud server, and the cloud "wouldn't work" until a
        // restart.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancellationToken::new();
        cancel.cancel(); // the probe is already stale before the background task starts
        let setup = external_chat_setup(Some("http://127.0.0.1:9/v1"), None, cancel, tx, ru());
        assert_eq!(setup.status, ServerStatus::Connecting); // the immediate status as usual
        // Give the background task a chance to run; a stale probe sends nothing.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(rx.try_recv().is_err(), "a stale probe sent a status");
    }

    #[tokio::test]
    async fn external_without_url_is_not_configured() {
        let (tx, _rx) = unbounded_channel();
        let setup =
            LlamaSupervisor.apply_chat(&external(None), None, CancellationToken::new(), tx, ru());
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
        let setup = LlamaSupervisor.apply_chat(&s, None, CancellationToken::new(), tx, ru());
        assert_eq!(setup.status, ServerStatus::NotConfigured);
    }

    #[tokio::test]
    async fn managed_with_bogus_binary_is_disconnected() {
        let (tx, _rx) = unbounded_channel();
        let s = EngineSettings {
            mode: ServerMode::Managed,
            managed: ManagedSettings {
                binary: Some("definitely-not-a-real-binary-xyz".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let setup = LlamaSupervisor.apply_chat(&s, None, CancellationToken::new(), tx, ru());
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
        let setup = LlamaSupervisor.apply_chat(&s, None, CancellationToken::new(), tx, ru());
        assert!(setup.backend.is_none());
        match setup.status {
            ServerStatus::Disconnected(msg) => assert!(msg.contains("файл модели"), "{msg}"),
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
        let setup =
            LlamaSupervisor.apply_chat(&s, Some("sk-stored"), CancellationToken::new(), tx, ru());
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
        match LlamaSupervisor
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
        match LlamaSupervisor
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
        let setup = LlamaSupervisor.apply_chat(&s, None, CancellationToken::new(), tx, ru());
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
        let setup = LlamaSupervisor.apply_chat(&s, None, CancellationToken::new(), tx, ru());
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
        let setup = LlamaSupervisor.apply_chat(&s, None, CancellationToken::new(), tx, ru());
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
        LlamaSupervisor.apply_embed(
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
        LlamaSupervisor.apply_embed(
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
        let setup = LlamaSupervisor.apply_embed(
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
        let setup = LlamaSupervisor.apply_embed(
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
        let setup = LlamaSupervisor.apply_embed(
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
        let setup = LlamaSupervisor.apply_embed(&s, None, CancellationToken::new(), tx, ru());
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
        let setup = LlamaSupervisor.apply_embed(&s, None, CancellationToken::new(), tx, ru());
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
        LlamaSupervisor.apply_embed(
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
        let setup = LlamaSupervisor.apply_embed(
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

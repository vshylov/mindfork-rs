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
            ServerMode::OpenAi | ServerMode::Gemini | ServerMode::Claude => {
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
            ImpersonationMode::OpenAi | ImpersonationMode::Gemini | ImpersonationMode::Claude => {
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
            // Anthropic has no embeddings API — RAG uses a separate embedder (ADR 0002).
            ServerMode::Claude => {
                tracing::warn!("Anthropic has no embeddings API; set a different embedder for RAG");
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
    // thinkingLevel/thinkingBudget), or the Anthropic Messages API (Claude).
    let backend: Arc<dyn EngineBackend> = match provider {
        CloudProvider::OpenAi => Arc::new(ResponsesClient::new(base, key, model.to_string())),
        CloudProvider::Gemini => Arc::new(GeminiClient::new(base, key, model.to_string())),
        CloudProvider::Claude => Arc::new(AnthropicClient::new(base, key, model.to_string())),
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

/// A background readiness probe: sends `Ready`/`Disconnected` on completion. If the
/// probe is marked stale (`cancel`) — it aborts without sending a status: otherwise a
/// late result from the previous server (e.g. a timeout of an intermediate external
/// while flipping between modes) would overwrite the new server's status.
fn spawn_probe(
    client: Arc<OpenAiClient>,
    timeout: Duration,
    exited: Option<CancellationToken>,
    cancel: CancellationToken,
    status_tx: UnboundedSender<ServerStatus>,
    loc: &'static Locale,
) {
    tokio::spawn(async move {
        let status = tokio::select! {
            biased;
            _ = cancel.cancelled() => return,
            res = wait_until_ready(&client, timeout, exited, loc) => match res {
                Ok(()) => ServerStatus::Ready,
                Err(err) => ServerStatus::Disconnected(err.to_string()),
            },
        };
        // While the probe was finishing, it may have been invalidated by a mode switch.
        if cancel.is_cancelled() {
            return;
        }
        let _ = status_tx.send(status);
    });
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
    use tokio::sync::mpsc::unbounded_channel;

    /// The reference (Russian) locale for displayed unavailability reasons:
    /// assertions on Russian substrings are pinned byte-for-byte.
    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    /// Spawns a throwaway local server on an ephemeral port and returns its base URL.
    ///
    /// `healthy` — answer `200` (the probe reads that as ready) or accept and hang up
    /// (the probe fails). We hang up rather than pointing at a *closed* port on
    /// purpose: connecting to a closed port costs ~2s per attempt on Windows, and the
    /// probe retries until its timeout — which turned this test into a minute.
    async fn spawn_stub_server(healthy: bool) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    // Drain the request first: closing a socket with unread data
                    // pending sends an RST, and the client then loses the response.
                    let mut buf = [0u8; 1024];
                    let _ = sock.read(&mut buf).await;
                    if healthy {
                        let _ = sock
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                            .await;
                        let _ = sock.shutdown().await;
                    }
                    // Otherwise: drop the socket — the client sees the connection close.
                });
            }
        });
        format!("http://{addr}/v1")
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
        let url = spawn_stub_server(true).await;
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
        let url = spawn_stub_server(false).await;
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

//! Супервайзер серверов инференса/эмбеддингов для оркестратора: (пере)запуск
//! managed-процесса или подключение к external по настройкам [`EngineSettings`]/
//! [`EmbedSettings`]. Спрятан за трейтом [`ServerSupervisor`] ради mock в тестах —
//! смена модели в настройках перезапускает сервер (spec §11.6, DoD M8).
//!
//! Живёт в `app`: это композиционный клей, знающий и про `shared/config`
//! (настройки), и про `shared/api` (движок/запуск процесса) — оба ниже по FSD.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::shared::api::{
    AnthropicClient, Embedder, EngineBackend, ManagedConfig, OpenAiClient, ServerHandle,
    UnavailableEmbedder, WireDialect, wait_until_ready,
};
use crate::shared::config::{
    CloudProvider, EmbedSettings, EngineSettings, ImpersonationEngineSettings, ImpersonationMode,
    ManagedSettings, ServerMode,
};
use crate::shared::server::ServerStatus;

/// Щедрый таймаут готовности managed-сервера: загрузка модели может занять минуты.
const MANAGED_READY_TIMEOUT: Duration = Duration::from_secs(600);
/// Короткий таймаут готовности external-сервера (он уже должен быть поднят).
const EXTERNAL_READY_TIMEOUT: Duration = Duration::from_secs(15);

/// Результат настройки chat-сервера: движок, опора на процесс (managed) и статус.
pub struct ChatSetup {
    pub backend: Option<Arc<dyn EngineBackend>>,
    /// Владелец дочернего процесса (managed). `None` — external/не настроен.
    pub handle: Option<ServerHandle>,
    pub status: ServerStatus,
}

/// Результат настройки embedding-сервера: источник эмбеддингов, опора на процесс
/// и статус. Probe эмбеддингов пока нет (RAG ленив), поэтому статус двухзначный:
/// `Ready` — эмбеддер настроен, `NotConfigured` — `UnavailableEmbedder` (чип скрыт).
pub struct EmbedSetup {
    pub embedder: Arc<dyn Embedder>,
    pub handle: Option<ServerHandle>,
    pub status: ServerStatus,
}

/// (Пере)подключение/запуск серверов по настройкам. За трейтом — ради mock-теста
/// перезапуска при смене модели.
pub trait ServerSupervisor: Send + Sync {
    /// (Пере)подключается к chat-серверу. Возвращает движок и статус немедленно
    /// (`Connecting`/`NotConfigured`/`Disconnected`), а готовность managed/external
    /// досылает в `status_tx` фоновым probe. `cancel` помечает probe устаревшим: при
    /// быстрой смене режима (managed→external→openai) поздний результат прежнего probe
    /// не должен перезаписать статус нового сервера.
    fn apply_chat(
        &self,
        settings: &EngineSettings,
        cancel: CancellationToken,
        status_tx: UnboundedSender<ServerStatus>,
    ) -> ChatSetup;

    /// (Пере)подключается к embedding-серверу (RAG ленив — без probe).
    fn apply_embed(&self, settings: &EmbedSettings) -> EmbedSetup;

    /// (Пере)подключается/запускает сервер имперсонации для режимов `managed`/
    /// `external`. Для `shared` НЕ вызывается оркестратором (он переиспользует
    /// chat-сервер ассистента); если всё же вызван — `NotConfigured`. См. spec §11.8.
    /// `cancel` — как у [`Self::apply_chat`] (инвалидация устаревшего probe).
    fn apply_impersonation(
        &self,
        settings: &ImpersonationEngineSettings,
        cancel: CancellationToken,
        status_tx: UnboundedSender<ServerStatus>,
    ) -> ChatSetup;
}

/// Боевой супервайзер: external — по URL (любой OpenAI-сервер), managed —
/// дочерний процесс `llama-server` (llama.cpp).
pub struct LlamaSupervisor;

impl ServerSupervisor for LlamaSupervisor {
    fn apply_chat(
        &self,
        settings: &EngineSettings,
        cancel: CancellationToken,
        status_tx: UnboundedSender<ServerStatus>,
    ) -> ChatSetup {
        match settings.mode {
            ServerMode::External => {
                external_chat_setup(settings.external.url.as_deref(), cancel, status_tx)
            }
            ServerMode::Managed => {
                managed_chat_setup(managed_config(&settings.managed), cancel, status_tx)
            }
            ServerMode::OpenAi | ServerMode::Gemini | ServerMode::Claude => {
                let cloud = settings.cloud().expect("облачный режим");
                cloud_chat_setup(
                    settings.mode.cloud_provider().expect("облачный режим"),
                    cloud.url.as_deref(),
                    cloud.api_key_env.as_deref(),
                    cloud.model_name.as_deref(),
                )
            }
        }
    }

    fn apply_impersonation(
        &self,
        settings: &ImpersonationEngineSettings,
        cancel: CancellationToken,
        status_tx: UnboundedSender<ServerStatus>,
    ) -> ChatSetup {
        match settings.mode {
            // `shared` обслуживается оркестратором (chat-сервер ассистента).
            ImpersonationMode::Shared => not_configured(),
            ImpersonationMode::External => {
                external_chat_setup(settings.external.url.as_deref(), cancel, status_tx)
            }
            ImpersonationMode::Managed => {
                managed_chat_setup(managed_config(&settings.managed), cancel, status_tx)
            }
            ImpersonationMode::OpenAi | ImpersonationMode::Gemini | ImpersonationMode::Claude => {
                let cloud = settings.cloud().expect("облачный режим");
                cloud_chat_setup(
                    settings.mode.cloud_provider().expect("облачный режим"),
                    cloud.url.as_deref(),
                    cloud.api_key_env.as_deref(),
                    cloud.model_name.as_deref(),
                )
            }
        }
    }

    fn apply_embed(&self, settings: &EmbedSettings) -> EmbedSetup {
        match settings.mode {
            ServerMode::External => match settings.external.url.as_deref() {
                Some(url) if !url.is_empty() => EmbedSetup {
                    embedder: Arc::new(OpenAiClient::new(url)),
                    handle: None,
                    status: ServerStatus::Ready,
                },
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
                        jinja: false, // embedding-серверу chat-template не нужен
                        reasoning_format: None,
                        embeddings: true,
                        no_mmap: false,
                        // Спекулятивное декодирование/FlashAttention для эмбеддинг-
                        // сервера не применимы (он не генерирует токены).
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
                    match ServerHandle::launch(&cfg) {
                        Ok(handle) => EmbedSetup {
                            embedder: Arc::new(OpenAiClient::new(handle.base_url())),
                            handle: Some(handle),
                            status: ServerStatus::Ready,
                        },
                        Err(err) => {
                            tracing::warn!(error = %err, "не удалось запустить embedding-сервер; RAG недоступен");
                            unavailable_embed()
                        }
                    }
                }
                _ => unavailable_embed(),
            },
            ServerMode::OpenAi | ServerMode::Gemini => {
                let cloud = settings.cloud().expect("облачный режим");
                cloud_embed_setup(
                    settings.mode.cloud_provider().expect("облачный режим"),
                    cloud.url.as_deref(),
                    cloud.api_key_env.as_deref(),
                    cloud.model_name.as_deref(),
                )
            }
            // У Anthropic нет embeddings API — RAG берёт отдельный эмбеддер (ADR 0002).
            ServerMode::Claude => {
                tracing::warn!("у Anthropic нет embeddings API; для RAG задайте другой эмбеддер");
                unavailable_embed()
            }
        }
    }
}

/// External chat-setup: подключение по URL (любой OpenAI-сервер), фоновый probe.
fn external_chat_setup(
    url: Option<&str>,
    cancel: CancellationToken,
    status_tx: UnboundedSender<ServerStatus>,
) -> ChatSetup {
    match url {
        Some(url) if !url.is_empty() => {
            let client = Arc::new(OpenAiClient::new(url));
            spawn_probe(
                client.clone(),
                EXTERNAL_READY_TIMEOUT,
                None,
                cancel,
                status_tx,
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

/// Managed chat-setup: запуск дочернего `llama-server`, фоновый probe (с учётом
/// раннего выхода процесса). Пустой бинарник → `NotConfigured`.
fn managed_chat_setup(
    cfg: ManagedConfig,
    cancel: CancellationToken,
    status_tx: UnboundedSender<ServerStatus>,
) -> ChatSetup {
    if cfg.binary.as_os_str().is_empty() {
        return not_configured();
    }
    match ServerHandle::launch(&cfg) {
        Ok(handle) => {
            let client = Arc::new(OpenAiClient::new(handle.base_url()));
            spawn_probe(
                client.clone(),
                MANAGED_READY_TIMEOUT,
                Some(handle.exited()),
                cancel,
                status_tx,
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

/// Строит [`ManagedConfig`] (`llama-server`) из managed-под-секции движка (общий
/// для chat-сервера ассистента и сервера имперсонации).
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

/// Резолвит API-ключ из env-переменной по её имени. `Err` с понятным сообщением,
/// если имя не задано или переменная отсутствует в окружении. Секрет на диск не
/// пишется (ADR 0004) — хранится только имя переменной.
fn resolve_api_key(api_key_env: Option<&str>) -> Result<String, String> {
    let var = api_key_env
        .filter(|v| !v.is_empty())
        .ok_or_else(|| "не задано имя env-переменной с API-ключом".to_string())?;
    std::env::var(var).map_err(|_| format!("переменная окружения {var} не задана"))
}

/// Строит облачный chat-backend (OpenAI/Gemini-compat): базовый URL провайдера (с
/// возможным override через `url`), Bearer-ключ из env, имя модели, строгий
/// OpenAI-диалект. Облако не «загружает модель» — статус сразу `Ready` (без probe).
/// Если модель не указана или ключ недоступен — `Disconnected` с понятным текстом
/// (чтобы не ловить `400` уже в ходе запроса). См. ADR 0004.
fn cloud_chat_setup(
    provider: CloudProvider,
    url_override: Option<&str>,
    api_key_env: Option<&str>,
    model_name: Option<&str>,
) -> ChatSetup {
    let disconnected = |msg: String| ChatSetup {
        backend: None,
        handle: None,
        status: ServerStatus::Disconnected(msg),
    };
    let Some(model) = model_name.filter(|m| !m.is_empty()) else {
        return disconnected("укажите имя модели для облачного провайдера".into());
    };
    let key = match resolve_api_key(api_key_env) {
        Ok(k) => k,
        Err(e) => return disconnected(e),
    };
    let base = url_override
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| provider.base_url());
    // Бэкенд по протоколу провайдера: OpenAI-совместимый клиент (OpenAI/Gemini, с
    // соответствующим диалектом лимита токенов) либо Anthropic Messages API (Claude).
    let backend: Arc<dyn EngineBackend> = match provider {
        CloudProvider::OpenAi => Arc::new(
            OpenAiClient::new(base)
                .with_api_key(Some(key))
                .with_model(Some(model.to_string()))
                .with_dialect(WireDialect::OpenAi),
        ),
        CloudProvider::Gemini => Arc::new(
            OpenAiClient::new(base)
                .with_api_key(Some(key))
                .with_model(Some(model.to_string()))
                .with_dialect(WireDialect::Gemini),
        ),
        CloudProvider::Claude => Arc::new(AnthropicClient::new(base, key, model.to_string())),
    };
    ChatSetup {
        backend: Some(backend),
        handle: None,
        status: ServerStatus::Ready,
    }
}

/// Строит облачный embedding-backend (OpenAI/Gemini). При неполной конфигурации
/// (нет модели или ключа) — `UnavailableEmbedder` (RAG отдаёт понятную ошибку, не
/// падает), как и для прочих несконфигурированных эмбеддеров.
fn cloud_embed_setup(
    provider: CloudProvider,
    url_override: Option<&str>,
    api_key_env: Option<&str>,
    model_name: Option<&str>,
) -> EmbedSetup {
    let (Some(model), Ok(key)) = (
        model_name.filter(|m| !m.is_empty()),
        resolve_api_key(api_key_env),
    ) else {
        tracing::warn!("облачные эмбеддинги не настроены (модель/ключ); RAG недоступен");
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

/// Фоновый probe готовности: по завершении шлёт `Ready`/`Disconnected`. Если probe
/// помечен устаревшим (`cancel`) — прерывается, не отправляя статус: иначе поздний
/// результат прежнего сервера (напр. таймаут промежуточного external при
/// перещёлкивании режимов) перезаписал бы статус нового сервера.
fn spawn_probe(
    client: Arc<OpenAiClient>,
    timeout: Duration,
    exited: Option<CancellationToken>,
    cancel: CancellationToken,
    status_tx: UnboundedSender<ServerStatus>,
) {
    tokio::spawn(async move {
        let status = tokio::select! {
            biased;
            _ = cancel.cancelled() => return,
            res = wait_until_ready(&client, timeout, exited) => match res {
                Ok(()) => ServerStatus::Ready,
                Err(err) => ServerStatus::Disconnected(err.to_string()),
            },
        };
        // Пока probe завершался, его могли успеть инвалидировать сменой режима.
        if cancel.is_cancelled() {
            return;
        }
        let _ = status_tx.send(status);
    });
}

/// Mock-супервайзер для тестов оркестратора: отдаёт заданный chat-backend и
/// детерминированный эмбеддер, считает вызовы `apply_chat` (проверка перезапуска
/// при смене модели, DoD M8). Без реальных процессов.
#[cfg(test)]
pub struct MockSupervisor {
    backend: Option<Arc<dyn EngineBackend>>,
    chat_calls: std::sync::atomic::AtomicUsize,
    embed_dim: usize,
}

#[cfg(test)]
impl MockSupervisor {
    /// Супервайзер, возвращающий `backend` для chat (in-process mock готов сразу —
    /// статус `Ready` синхронно, без фонового probe, чтобы тесты не зависели от гонки).
    pub fn with_backend(backend: Option<Arc<dyn EngineBackend>>) -> Self {
        Self {
            backend,
            chat_calls: std::sync::atomic::AtomicUsize::new(0),
            embed_dim: 16,
        }
    }

    /// Сколько раз вызывали `apply_chat` (≥2 после перезапуска по смене модели).
    pub fn chat_call_count(&self) -> usize {
        self.chat_calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
impl ServerSupervisor for MockSupervisor {
    fn apply_chat(
        &self,
        _settings: &EngineSettings,
        _cancel: CancellationToken,
        _status_tx: UnboundedSender<ServerStatus>,
    ) -> ChatSetup {
        self.chat_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let backend = self.backend.clone();
        // Mock-движок готов мгновенно: отдаём `Ready` как немедленный статус (а не
        // `Connecting` + async-probe), иначе оркестратор мог бы обработать команду
        // генерации раньше события готовности и отклонить её (гонка в тестах).
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
        _cancel: CancellationToken,
        _status_tx: UnboundedSender<ServerStatus>,
    ) -> ChatSetup {
        // Mock отдаёт тот же backend готовым сразу (как apply_chat) — для тестов
        // managed/external режимов имперсонации.
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

    fn apply_embed(&self, _settings: &EmbedSettings) -> EmbedSetup {
        EmbedSetup {
            embedder: Arc::new(crate::shared::api::mock::MockEmbedder::new(self.embed_dim)),
            handle: None,
            status: ServerStatus::Ready,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

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
            CancellationToken::new(),
            tx,
        );
        assert!(setup.backend.is_some());
        assert!(setup.handle.is_none());
        assert_eq!(setup.status, ServerStatus::Connecting);
    }

    #[tokio::test]
    async fn superseded_probe_sends_no_status() {
        // probe, помеченный устаревшим (смена режима managed→external→openai), не
        // должен слать статус: иначе поздний таймаут прежнего external перезаписал бы
        // `Ready` нового облачного сервера, и облако «не работало» до перезапуска.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancellationToken::new();
        cancel.cancel(); // probe устарел ещё до старта фоновой задачи
        let setup = external_chat_setup(Some("http://127.0.0.1:9/v1"), cancel, tx);
        assert_eq!(setup.status, ServerStatus::Connecting); // немедленный статус как обычно
        // Даём фоновой задаче шанс выполниться; устаревший probe ничего не присылает.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(rx.try_recv().is_err(), "устаревший probe прислал статус");
    }

    #[tokio::test]
    async fn external_without_url_is_not_configured() {
        let (tx, _rx) = unbounded_channel();
        let setup = LlamaSupervisor.apply_chat(&external(None), CancellationToken::new(), tx);
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
        let setup = LlamaSupervisor.apply_chat(&s, CancellationToken::new(), tx);
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
        let setup = LlamaSupervisor.apply_chat(&s, CancellationToken::new(), tx);
        assert!(setup.backend.is_none());
        assert!(matches!(setup.status, ServerStatus::Disconnected(_)));
    }

    #[tokio::test]
    async fn managed_with_missing_model_is_disconnected() {
        // Бинарник есть (spawn бы прошёл), но файл модели отсутствует: раньше это
        // вешало UI в «подключение…» до таймаута; теперь — сразу `Disconnected`
        // с понятным сообщением.
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
        let setup = LlamaSupervisor.apply_chat(&s, CancellationToken::new(), tx);
        assert!(setup.backend.is_none());
        match setup.status {
            ServerStatus::Disconnected(msg) => assert!(msg.contains("файл модели"), "{msg}"),
            other => panic!("ожидался Disconnected, получили {other:?}"),
        }
    }

    #[test]
    fn resolve_api_key_reads_env_and_reports_missing() {
        // PATH задана в любой ОС — гарантированный положительный случай без мутации env.
        assert!(resolve_api_key(Some("PATH")).is_ok());
        assert!(resolve_api_key(None).is_err());
        assert!(
            resolve_api_key(Some("MINDFORK_DEFINITELY_UNSET_VAR_42"))
                .unwrap_err()
                .contains("MINDFORK_DEFINITELY_UNSET_VAR_42")
        );
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
            .apply_chat(&s, CancellationToken::new(), tx)
            .status
        {
            ServerStatus::Disconnected(m) => assert!(m.contains("модел"), "{m}"),
            other => panic!("ожидался Disconnected, получили {other:?}"),
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
            .apply_chat(&s, CancellationToken::new(), tx)
            .status
        {
            ServerStatus::Disconnected(m) => {
                assert!(m.contains("MINDFORK_DEFINITELY_UNSET_VAR_42"), "{m}")
            }
            other => panic!("ожидался Disconnected, получили {other:?}"),
        }
    }

    #[tokio::test]
    async fn cloud_chat_with_model_and_key_is_ready() {
        // Используем PATH как «ключ»: важно лишь, что env-переменная резолвится.
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
        let setup = LlamaSupervisor.apply_chat(&s, CancellationToken::new(), tx);
        assert!(setup.backend.is_some());
        assert!(setup.handle.is_none(), "облако без дочернего процесса");
        assert_eq!(setup.status, ServerStatus::Ready);
    }

    #[tokio::test]
    async fn cloud_chat_claude_with_model_and_key_is_ready() {
        // Claude использует отдельный протокол (AnthropicClient), но контракт настройки
        // тот же: модель + ключ из env → Ready, без дочернего процесса.
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
        let setup = LlamaSupervisor.apply_chat(&s, CancellationToken::new(), tx);
        assert!(setup.backend.is_some());
        assert!(setup.handle.is_none());
        assert_eq!(setup.status, ServerStatus::Ready);
    }

    #[tokio::test]
    async fn claude_embed_is_unavailable() {
        // У Anthropic нет embeddings — RAG недоступен (как прочие unavailable).
        let s = EmbedSettings {
            mode: ServerMode::Claude,
            claude: crate::shared::config::CloudSettings {
                model_name: Some("x".into()),
                api_key_env: Some("PATH".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let err = LlamaSupervisor
            .apply_embed(&s)
            .embedder
            .embed(vec!["x".into()])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("не настроен"));
    }

    #[tokio::test]
    async fn cloud_embed_unconfigured_is_unavailable() {
        // Облачные эмбеддинги без модели → RAG недоступен (как прочие unavailable).
        let s = EmbedSettings {
            mode: ServerMode::OpenAi,
            openai: crate::shared::config::CloudSettings {
                api_key_env: Some("PATH".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let setup = LlamaSupervisor.apply_embed(&s);
        let err = setup.embedder.embed(vec!["x".into()]).await.unwrap_err();
        assert!(err.to_string().contains("не настроен"));
    }

    #[tokio::test]
    async fn embed_external_url_is_available() {
        let s = EmbedSettings {
            mode: ServerMode::External,
            external: crate::shared::config::ExternalSettings {
                url: Some("http://127.0.0.1:9/v1".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let setup = LlamaSupervisor.apply_embed(&s);
        assert!(setup.handle.is_none());
        // Источник эмбеддингов сконфигурирован (не UnavailableEmbedder).
        // Проверяем косвенно: embed на «мёртвый» URL вернёт ошибку соединения,
        // а UnavailableEmbedder — фиксированное «не настроен».
        let err = setup.embedder.embed(vec!["x".into()]).await.unwrap_err();
        assert!(!err.to_string().contains("не настроен"), "{err}");
    }

    #[tokio::test]
    async fn embed_unconfigured_is_unavailable() {
        let setup = LlamaSupervisor.apply_embed(&EmbedSettings::default());
        let err = setup.embedder.embed(vec!["x".into()]).await.unwrap_err();
        assert!(err.to_string().contains("не настроен"));
    }
}

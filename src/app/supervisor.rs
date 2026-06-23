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
    Embedder, EngineBackend, ManagedConfig, OpenAiClient, ServerHandle, UnavailableEmbedder,
    wait_until_ready,
};
use crate::shared::config::{
    EmbedSettings, EngineSettings, ImpersonationEngineSettings, ImpersonationMode, ServerMode,
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

/// Результат настройки embedding-сервера: источник эмбеддингов и опора на процесс.
pub struct EmbedSetup {
    pub embedder: Arc<dyn Embedder>,
    pub handle: Option<ServerHandle>,
}

/// (Пере)подключение/запуск серверов по настройкам. За трейтом — ради mock-теста
/// перезапуска при смене модели.
pub trait ServerSupervisor: Send + Sync {
    /// (Пере)подключается к chat-серверу. Возвращает движок и статус немедленно
    /// (`Connecting`/`NotConfigured`/`Disconnected`), а готовность managed/external
    /// досылает в `status_tx` фоновым probe.
    fn apply_chat(
        &self,
        settings: &EngineSettings,
        status_tx: UnboundedSender<ServerStatus>,
    ) -> ChatSetup;

    /// (Пере)подключается к embedding-серверу (RAG ленив — без probe).
    fn apply_embed(&self, settings: &EmbedSettings) -> EmbedSetup;

    /// (Пере)подключается/запускает сервер имперсонации для режимов `managed`/
    /// `external`. Для `shared` НЕ вызывается оркестратором (он переиспользует
    /// chat-сервер ассистента); если всё же вызван — `NotConfigured`. См. spec §11.8.
    fn apply_impersonation(
        &self,
        settings: &ImpersonationEngineSettings,
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
        status_tx: UnboundedSender<ServerStatus>,
    ) -> ChatSetup {
        match settings.mode {
            ServerMode::External => match settings.url.as_deref() {
                Some(url) if !url.is_empty() => {
                    let client = Arc::new(OpenAiClient::new(url));
                    spawn_probe(client.clone(), EXTERNAL_READY_TIMEOUT, None, status_tx);
                    ChatSetup {
                        backend: Some(client),
                        handle: None,
                        status: ServerStatus::Connecting,
                    }
                }
                _ => not_configured(),
            },
            ServerMode::Managed => match settings.binary.as_deref() {
                Some(bin) if !bin.is_empty() => {
                    let cfg = managed_config(settings);
                    match ServerHandle::launch(&cfg) {
                        Ok(handle) => {
                            let client = Arc::new(OpenAiClient::new(handle.base_url()));
                            spawn_probe(
                                client.clone(),
                                MANAGED_READY_TIMEOUT,
                                Some(handle.exited()),
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
                _ => not_configured(),
            },
        }
    }

    fn apply_impersonation(
        &self,
        settings: &ImpersonationEngineSettings,
        status_tx: UnboundedSender<ServerStatus>,
    ) -> ChatSetup {
        match settings.mode {
            // `shared` обслуживается оркестратором (chat-сервер ассистента).
            ImpersonationMode::Shared => not_configured(),
            ImpersonationMode::External => match settings.url.as_deref() {
                Some(url) if !url.is_empty() => {
                    let client = Arc::new(OpenAiClient::new(url));
                    spawn_probe(client.clone(), EXTERNAL_READY_TIMEOUT, None, status_tx);
                    ChatSetup {
                        backend: Some(client),
                        handle: None,
                        status: ServerStatus::Connecting,
                    }
                }
                _ => not_configured(),
            },
            ImpersonationMode::Managed => match settings.binary.as_deref() {
                Some(bin) if !bin.is_empty() => {
                    let cfg = impersonation_managed_config(settings, bin);
                    match ServerHandle::launch(&cfg) {
                        Ok(handle) => {
                            let client = Arc::new(OpenAiClient::new(handle.base_url()));
                            spawn_probe(
                                client.clone(),
                                MANAGED_READY_TIMEOUT,
                                Some(handle.exited()),
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
                _ => not_configured(),
            },
        }
    }

    fn apply_embed(&self, settings: &EmbedSettings) -> EmbedSetup {
        match settings.mode {
            ServerMode::External => match settings.url.as_deref() {
                Some(url) if !url.is_empty() => EmbedSetup {
                    embedder: Arc::new(OpenAiClient::new(url)),
                    handle: None,
                },
                _ => unavailable_embed(),
            },
            ServerMode::Managed => match settings.binary.as_deref() {
                Some(bin) if !bin.is_empty() => {
                    let cfg = ManagedConfig {
                        binary: bin.into(),
                        model_path: settings.model_path.clone(),
                        gpu_layers: settings.gpu_layers,
                        context_size: crate::shared::config::DEFAULT_CONTEXT_SIZE,
                        jinja: false, // embedding-серверу chat-template не нужен
                        reasoning_format: None,
                        embeddings: true,
                        no_mmap: false,
                        host: "127.0.0.1".into(),
                        port: settings.port,
                        extra_args: vec![],
                    };
                    match ServerHandle::launch(&cfg) {
                        Ok(handle) => EmbedSetup {
                            embedder: Arc::new(OpenAiClient::new(handle.base_url())),
                            handle: Some(handle),
                        },
                        Err(err) => {
                            tracing::warn!(error = %err, "не удалось запустить embedding-сервер; RAG недоступен");
                            unavailable_embed()
                        }
                    }
                }
                _ => unavailable_embed(),
            },
        }
    }
}

/// Строит [`ManagedConfig`] (`llama-server`) из настроек chat-сервера.
fn managed_config(s: &EngineSettings) -> ManagedConfig {
    ManagedConfig {
        binary: s.binary.clone().unwrap_or_default().into(),
        model_path: s.model_path.clone(),
        gpu_layers: s.gpu_layers,
        context_size: s.context_size,
        jinja: s.jinja,
        reasoning_format: s.reasoning_format.clone(),
        embeddings: false,
        no_mmap: s.no_mmap,
        host: s.host.clone(),
        port: s.port,
        extra_args: vec![],
    }
}

/// Строит [`ManagedConfig`] (`llama-server`) из настроек сервера имперсонации.
fn impersonation_managed_config(s: &ImpersonationEngineSettings, bin: &str) -> ManagedConfig {
    ManagedConfig {
        binary: bin.into(),
        model_path: s.model_path.clone(),
        gpu_layers: s.gpu_layers,
        context_size: s.context_size,
        jinja: s.jinja,
        reasoning_format: s.reasoning_format.clone(),
        embeddings: false,
        no_mmap: s.no_mmap,
        host: s.host.clone(),
        port: s.port,
        extra_args: vec![],
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
    }
}

/// Фоновый probe готовности: по завершении шлёт `Ready`/`Disconnected`.
fn spawn_probe(
    client: Arc<OpenAiClient>,
    timeout: Duration,
    exited: Option<CancellationToken>,
    status_tx: UnboundedSender<ServerStatus>,
) {
    tokio::spawn(async move {
        let status = match wait_until_ready(&client, timeout, exited).await {
            Ok(()) => ServerStatus::Ready,
            Err(err) => ServerStatus::Disconnected(err.to_string()),
        };
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
            url: url.map(String::from),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn external_with_url_yields_backend_connecting() {
        let (tx, _rx) = unbounded_channel();
        let setup = LlamaSupervisor.apply_chat(&external(Some("http://127.0.0.1:9/v1")), tx);
        assert!(setup.backend.is_some());
        assert!(setup.handle.is_none());
        assert_eq!(setup.status, ServerStatus::Connecting);
    }

    #[tokio::test]
    async fn external_without_url_is_not_configured() {
        let (tx, _rx) = unbounded_channel();
        let setup = LlamaSupervisor.apply_chat(&external(None), tx);
        assert!(setup.backend.is_none());
        assert_eq!(setup.status, ServerStatus::NotConfigured);
    }

    #[tokio::test]
    async fn managed_without_binary_is_not_configured() {
        let (tx, _rx) = unbounded_channel();
        let s = EngineSettings {
            mode: ServerMode::Managed,
            binary: None,
            ..Default::default()
        };
        let setup = LlamaSupervisor.apply_chat(&s, tx);
        assert_eq!(setup.status, ServerStatus::NotConfigured);
    }

    #[tokio::test]
    async fn managed_with_bogus_binary_is_disconnected() {
        let (tx, _rx) = unbounded_channel();
        let s = EngineSettings {
            mode: ServerMode::Managed,
            binary: Some("definitely-not-a-real-binary-xyz".into()),
            ..Default::default()
        };
        let setup = LlamaSupervisor.apply_chat(&s, tx);
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
            binary: Some("llama-server".into()),
            model_path: Some("no/such/model.gguf".into()),
            ..Default::default()
        };
        let setup = LlamaSupervisor.apply_chat(&s, tx);
        assert!(setup.backend.is_none());
        match setup.status {
            ServerStatus::Disconnected(msg) => assert!(msg.contains("файл модели"), "{msg}"),
            other => panic!("ожидался Disconnected, получили {other:?}"),
        }
    }

    #[tokio::test]
    async fn embed_external_url_is_available() {
        let s = EmbedSettings {
            mode: ServerMode::External,
            url: Some("http://127.0.0.1:9/v1".into()),
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

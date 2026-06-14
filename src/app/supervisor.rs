//! Супервайзер серверов инференса/эмбеддингов для оркестратора: (пере)запуск
//! managed-процесса или подключение к external по настройкам [`XinferSettings`]/
//! [`EmbedSettings`]. Спрятан за трейтом [`ServerSupervisor`] ради mock в тестах —
//! смена модели в настройках перезапускает сервер (spec §11.6, DoD M8).
//!
//! Живёт в `app`: это композиционный клей, знающий и про `shared/config`
//! (настройки), и про `shared/api` (движок/запуск процесса) — оба ниже по FSD.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

use crate::shared::api::{
    Embedder, EngineBackend, ManagedConfig, ServerHandle, UnavailableEmbedder, XinferClient,
    wait_until_ready,
};
use crate::shared::config::{EmbedSettings, ServerMode, XinferSettings};
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
        settings: &XinferSettings,
        status_tx: UnboundedSender<ServerStatus>,
    ) -> ChatSetup;

    /// (Пере)подключается к embedding-серверу (RAG ленив — без probe).
    fn apply_embed(&self, settings: &EmbedSettings) -> EmbedSetup;
}

/// Боевой супервайзер xinfer: external — по URL, managed — дочерний процесс.
pub struct XinferSupervisor;

impl ServerSupervisor for XinferSupervisor {
    fn apply_chat(
        &self,
        settings: &XinferSettings,
        status_tx: UnboundedSender<ServerStatus>,
    ) -> ChatSetup {
        match settings.mode {
            ServerMode::External => match settings.url.as_deref() {
                Some(url) if !url.is_empty() => {
                    let client = Arc::new(XinferClient::new(url));
                    spawn_probe(client.clone(), EXTERNAL_READY_TIMEOUT, status_tx);
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
                            let client = Arc::new(XinferClient::new(handle.base_url()));
                            spawn_probe(client.clone(), MANAGED_READY_TIMEOUT, status_tx);
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
                    embedder: Arc::new(XinferClient::new(url)),
                    handle: None,
                },
                _ => unavailable_embed(),
            },
            ServerMode::Managed => match settings.binary.as_deref() {
                Some(bin) if !bin.is_empty() => {
                    let cfg = ManagedConfig {
                        binary: bin.into(),
                        model_id: settings.model_id.clone(),
                        weight_path: None,
                        weight_file: None,
                        isq: None,
                        device_ids: vec![0],
                        cpu: false,
                        port: settings.port,
                        extra_args: vec![],
                    };
                    match ServerHandle::launch(&cfg) {
                        Ok(handle) => EmbedSetup {
                            embedder: Arc::new(XinferClient::new(handle.base_url())),
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

/// Строит [`ManagedConfig`] из настроек chat-сервера.
fn managed_config(s: &XinferSettings) -> ManagedConfig {
    ManagedConfig {
        binary: s.binary.clone().unwrap_or_default().into(),
        model_id: s.model_id.clone(),
        weight_path: s.weight_path.clone(),
        weight_file: s.weight_file.clone(),
        isq: s.isq.clone(),
        device_ids: s.device_ids.clone(),
        cpu: s.cpu,
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
    client: Arc<XinferClient>,
    timeout: Duration,
    status_tx: UnboundedSender<ServerStatus>,
) {
    tokio::spawn(async move {
        let status = match wait_until_ready(&client, timeout).await {
            Ok(()) => ServerStatus::Ready,
            Err(err) => ServerStatus::Disconnected(err.to_string()),
        };
        let _ = status_tx.send(status);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    fn external(url: Option<&str>) -> XinferSettings {
        XinferSettings {
            mode: ServerMode::External,
            url: url.map(String::from),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn external_with_url_yields_backend_connecting() {
        let (tx, _rx) = unbounded_channel();
        let setup = XinferSupervisor.apply_chat(&external(Some("http://127.0.0.1:9/v1")), tx);
        assert!(setup.backend.is_some());
        assert!(setup.handle.is_none());
        assert_eq!(setup.status, ServerStatus::Connecting);
    }

    #[tokio::test]
    async fn external_without_url_is_not_configured() {
        let (tx, _rx) = unbounded_channel();
        let setup = XinferSupervisor.apply_chat(&external(None), tx);
        assert!(setup.backend.is_none());
        assert_eq!(setup.status, ServerStatus::NotConfigured);
    }

    #[tokio::test]
    async fn managed_without_binary_is_not_configured() {
        let (tx, _rx) = unbounded_channel();
        let s = XinferSettings {
            mode: ServerMode::Managed,
            binary: None,
            ..Default::default()
        };
        let setup = XinferSupervisor.apply_chat(&s, tx);
        assert_eq!(setup.status, ServerStatus::NotConfigured);
    }

    #[tokio::test]
    async fn managed_with_bogus_binary_is_disconnected() {
        let (tx, _rx) = unbounded_channel();
        let s = XinferSettings {
            mode: ServerMode::Managed,
            binary: Some("definitely-not-a-real-binary-xyz".into()),
            ..Default::default()
        };
        let setup = XinferSupervisor.apply_chat(&s, tx);
        assert!(setup.backend.is_none());
        assert!(matches!(setup.status, ServerStatus::Disconnected(_)));
    }

    #[tokio::test]
    async fn embed_external_url_is_available() {
        let s = EmbedSettings {
            mode: ServerMode::External,
            url: Some("http://127.0.0.1:9/v1".into()),
            ..Default::default()
        };
        let setup = XinferSupervisor.apply_embed(&s);
        assert!(setup.handle.is_none());
        // Источник эмбеддингов сконфигурирован (не UnavailableEmbedder).
        // Проверяем косвенно: embed на «мёртвый» URL вернёт ошибку соединения,
        // а UnavailableEmbedder — фиксированное «не настроен».
        let err = setup.embedder.embed(vec!["x".into()]).await.unwrap_err();
        assert!(!err.to_string().contains("не настроен"), "{err}");
    }

    #[tokio::test]
    async fn embed_unconfigured_is_unavailable() {
        let setup = XinferSupervisor.apply_embed(&EmbedSettings::default());
        let err = setup.embedder.embed(vec!["x".into()]).await.unwrap_err();
        assert!(err.to_string().contains("не настроен"));
    }
}

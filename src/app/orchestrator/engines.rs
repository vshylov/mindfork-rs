//! [`EngineManager`] — жизненный цикл серверов инференса/эмбеддингов: владеет
//! движками (`backend`/`imp_backend`/`embedder`), опорами на managed-процессы
//! (`*_handle`, `kill_on_drop`), статусами готовности и каналами фонового probe.
//! Выделен из оркестратора (Фаза 3): группирует ~11 полей и серверную логику в
//! когезивную единицу, пара к трейту [`ServerSupervisor`]. Оркестратор остаётся
//! единственным владельцем `Chat`; здесь — только серверы, без доменного состояния.

use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::app::events::ServerStatus;
use crate::app::supervisor::ServerSupervisor;
use crate::shared::api::{Embedder, EngineBackend, ServerHandle};
use crate::shared::config::{
    EmbedSettings, EngineSettings, ImpersonationEngineSettings, ImpersonationMode,
};

pub(super) struct EngineManager {
    /// Супервайзер серверов (для перезапуска при смене модели/сервера).
    supervisor: Arc<dyn ServerSupervisor>,
    /// Движок ассистента. `None` — external/не настроен.
    pub(super) backend: Option<Arc<dyn EngineBackend>>,
    /// Опора на managed chat-процесс (drop → kill).
    chat_handle: Option<ServerHandle>,
    /// Опора на managed embedding-процесс.
    embed_handle: Option<ServerHandle>,
    /// Движок имперсонации для режимов managed/external (`None` в режиме `shared` —
    /// тогда используется `backend` ассистента). См. spec §11.8.
    imp_backend: Option<Arc<dyn EngineBackend>>,
    /// Опора на managed-процесс сервера имперсонации.
    imp_handle: Option<ServerHandle>,
    /// Текущий статус chat-сервера. Генерация стартует только в `Ready`: запрос к
    /// ещё загружающемуся (`Connecting`) managed-серверу вернул бы 503 («error
    /// status»), а для перегенерации — ещё и снёс бы прежний ответ впустую.
    pub(super) server_status: ServerStatus,
    /// Статус сервера имперсонации (для managed/external; в `shared` не используется).
    imp_status: ServerStatus,
    /// Канал статуса chat-сервера для фонового probe супервайзера.
    status_tx: UnboundedSender<ServerStatus>,
    /// Канал статуса сервера имперсонации (фоновый probe).
    imp_status_tx: UnboundedSender<ServerStatus>,
    /// Токен инвалидации фонового probe текущего chat-сервера: смена режима/модели
    /// помечает прежний probe устаревшим, чтобы его поздний результат (напр. таймаут
    /// промежуточного external при перещёлкивании managed→external→openai) не
    /// перезаписал статус нового сервера.
    chat_probe_cancel: Option<CancellationToken>,
    /// Токен инвалидации фонового probe сервера имперсонации (аналогично).
    imp_probe_cancel: Option<CancellationToken>,
    /// Источник эмбеддингов для RAG (выделенный сервер — ADR 0002).
    pub(super) embedder: Arc<dyn Embedder>,
}

impl EngineManager {
    /// Создаёт менеджер без поднятых серверов (статусы `NotConfigured`, эмбеддер —
    /// [`UnavailableEmbedder`]). Серверы поднимаются последующими `apply_*`.
    pub(super) fn new(
        supervisor: Arc<dyn ServerSupervisor>,
        status_tx: UnboundedSender<ServerStatus>,
        imp_status_tx: UnboundedSender<ServerStatus>,
    ) -> Self {
        Self {
            supervisor,
            backend: None,
            chat_handle: None,
            embed_handle: None,
            imp_backend: None,
            imp_handle: None,
            server_status: ServerStatus::NotConfigured,
            imp_status: ServerStatus::NotConfigured,
            status_tx,
            imp_status_tx,
            chat_probe_cancel: None,
            imp_probe_cancel: None,
            embedder: Arc::new(crate::shared::api::UnavailableEmbedder),
        }
    }

    /// (Пере)поднимает chat-сервер по настройкам: гасит прежний managed-процесс,
    /// просит супервайзер настроить новый. Возвращает немедленный статус — вызывающий
    /// эмитит его в UI (`AppEvent::ServerStatus`).
    pub(super) fn apply_chat(&mut self, settings: &EngineSettings) -> ServerStatus {
        self.chat_handle = None; // drop старого managed-процесса (kill_on_drop)
        // Инвалидируем probe прежнего сервера и заводим новый токен.
        if let Some(tok) = self.chat_probe_cancel.take() {
            tok.cancel();
        }
        let cancel = CancellationToken::new();
        self.chat_probe_cancel = Some(cancel.clone());
        let setup = self
            .supervisor
            .apply_chat(settings, cancel, self.status_tx.clone());
        self.backend = setup.backend;
        self.chat_handle = setup.handle;
        self.server_status = setup.status.clone();
        setup.status
    }

    /// (Пере)поднимает embedding-сервер по настройкам.
    pub(super) fn apply_embed(&mut self, settings: &EmbedSettings) {
        self.embed_handle = None;
        let setup = self.supervisor.apply_embed(settings);
        self.embedder = setup.embedder;
        self.embed_handle = setup.handle;
    }

    /// (Пере)поднимает сервер имперсонации. В режиме `shared` отдельный сервер не
    /// нужен — переиспользуется chat-сервер ассистента.
    pub(super) fn apply_impersonation(&mut self, settings: &ImpersonationEngineSettings) {
        self.imp_handle = None; // drop прежнего managed-процесса (kill_on_drop)
        // Инвалидируем probe прежнего сервера имперсонации (как у chat-сервера).
        if let Some(tok) = self.imp_probe_cancel.take() {
            tok.cancel();
        }
        match settings.mode {
            ImpersonationMode::Shared => {
                self.imp_backend = None;
                self.imp_status = ServerStatus::NotConfigured;
            }
            _ => {
                let cancel = CancellationToken::new();
                self.imp_probe_cancel = Some(cancel.clone());
                let setup = self.supervisor.apply_impersonation(
                    settings,
                    cancel,
                    self.imp_status_tx.clone(),
                );
                self.imp_backend = setup.backend;
                self.imp_handle = setup.handle;
                self.imp_status = setup.status;
            }
        }
    }

    /// Обновляет статус chat-сервера (из фонового probe).
    pub(super) fn set_chat_status(&mut self, status: ServerStatus) {
        self.server_status = status;
    }

    /// Обновляет статус сервера имперсонации (из фонового probe).
    pub(super) fn set_imp_status(&mut self, status: ServerStatus) {
        self.imp_status = status;
    }

    /// Возвращает движок ассистента, если chat-сервер готов (`Ready`); иначе — `Err`
    /// с понятным текстом (не настроен / ещё подключается / недоступен). Сам ничего
    /// не эмитит — вызывающий решает, куда направить ошибку. См. spec §7.
    pub(super) fn backend_if_ready(&self) -> Result<Arc<dyn EngineBackend>, String> {
        match &self.server_status {
            ServerStatus::Ready => self
                .backend
                .clone()
                .ok_or_else(|| "LLM-сервер не настроен".to_string()),
            ServerStatus::Connecting => {
                Err("Сервер ещё подключается — дождитесь готовности и повторите".into())
            }
            ServerStatus::NotConfigured => Err("LLM-сервер не настроен".into()),
            ServerStatus::Disconnected(reason) => Err(format!("Сервер недоступен: {reason}")),
        }
    }

    /// Возвращает движок имперсонации, если он готов; иначе — `Err` с понятным
    /// текстом. В режиме `shared` используется chat-сервер ассистента.
    pub(super) fn impersonation_backend_if_ready(
        &self,
        mode: ImpersonationMode,
    ) -> Result<Arc<dyn EngineBackend>, String> {
        if mode == ImpersonationMode::Shared {
            return self.backend_if_ready();
        }
        match &self.imp_status {
            ServerStatus::Ready => self
                .imp_backend
                .clone()
                .ok_or_else(|| "Сервер имперсонации не настроен".to_string()),
            ServerStatus::Connecting => {
                Err("Сервер имперсонации ещё подключается — повторите позже".into())
            }
            ServerStatus::NotConfigured => Err("Сервер имперсонации не настроен".into()),
            ServerStatus::Disconnected(reason) => {
                Err(format!("Сервер имперсонации недоступен: {reason}"))
            }
        }
    }

    /// Клон источника эмбеддингов для фоновых задач RAG / контекста инструментов.
    pub(super) fn embedder(&self) -> Arc<dyn Embedder> {
        self.embedder.clone()
    }
}

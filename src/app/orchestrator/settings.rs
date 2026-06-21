//! Конфигурация приложения и (пере)запуск серверов инференса/эмбеддингов через
//! [`ServerSupervisor`]. Оркестратор — единственный писатель в `settings.json`.

use crate::app::events::{AppEvent, ServerStatus};
use crate::shared::config::{AppConfig, ImpersonationMode};

use super::{Orchestrator, build_registry};

impl Orchestrator {
    /// Применяет правки конфигурации: сохраняет, перезапускает сервер/реестр при
    /// необходимости и переэмитит настройки. Единственный писатель в `settings.json`.
    pub(super) fn handle_update_config(&mut self, config: AppConfig) {
        let old = std::mem::replace(&mut self.config, config);
        if let Err(err) = self.storage.json().save_config(&self.config) {
            let _ = self.evt_tx.send(AppEvent::Error(format!(
                "Не удалось сохранить настройки: {err}"
            )));
            self.config = old; // откат к прежнему состоянию
            return;
        }
        // Смена настроек chat-сервера (модель/режим/порт/…) — перезапуск (spec §11.6).
        if self.config.engine != old.engine {
            self.apply_chat_settings();
        }
        // Смена настроек сервера имперсонации — пере-подключение/перезапуск.
        if self.config.impersonation_engine != old.impersonation_engine {
            self.apply_impersonation_settings();
        }
        // Смена настроек embedding-сервера — пере-подключение/перезапуск.
        if self.config.embed != old.embed {
            self.apply_embed_settings();
        }
        // Смена параметров инструментов — пересборка реестра (python_path, лимиты).
        if self.config.tools != old.tools {
            self.registry = std::sync::Arc::new(build_registry(&self.config));
        }
        self.emit_settings();
    }

    /// (Пере)поднимает chat-сервер по `config.engine`: гасит прежний процесс,
    /// просит супервайзер настроить новый, эмитит немедленный статус.
    pub(super) fn apply_chat_settings(&mut self) {
        self.chat_handle = None; // drop старого managed-процесса (kill_on_drop)
        let setup = self
            .supervisor
            .apply_chat(&self.config.engine, self.status_tx.clone());
        self.backend = setup.backend;
        self.chat_handle = setup.handle;
        self.server_status = setup.status.clone();
        let _ = self.evt_tx.send(AppEvent::ServerStatus(setup.status));
    }

    /// (Пере)поднимает embedding-сервер по `config.embed`.
    pub(super) fn apply_embed_settings(&mut self) {
        self.embed_handle = None;
        let setup = self.supervisor.apply_embed(&self.config.embed);
        self.embedder = setup.embedder;
        self.embed_handle = setup.handle;
    }

    /// (Пере)поднимает сервер имперсонации по `config.impersonation_engine`. В режиме
    /// `shared` отдельный сервер не нужен — переиспользуется chat-сервер ассистента.
    pub(super) fn apply_impersonation_settings(&mut self) {
        self.imp_handle = None; // drop прежнего managed-процесса (kill_on_drop)
        match self.config.impersonation_engine.mode {
            ImpersonationMode::Shared => {
                self.imp_backend = None;
                self.imp_status = ServerStatus::NotConfigured;
            }
            _ => {
                let setup = self.supervisor.apply_impersonation(
                    &self.config.impersonation_engine,
                    self.imp_status_tx.clone(),
                );
                self.imp_backend = setup.backend;
                self.imp_handle = setup.handle;
                self.imp_status = setup.status;
            }
        }
    }
}

//! Конфигурация приложения и (пере)запуск серверов инференса/эмбеддингов через
//! [`ServerSupervisor`]. Оркестратор — единственный писатель в `settings.json`.

use crate::app::events::AppEvent;
use crate::shared::config::AppConfig;

use super::{Orchestrator, build_registry};

impl Orchestrator {
    /// Применяет правки конфигурации: сохраняет, перезапускает сервер/реестр при
    /// необходимости и переэмитит настройки. Единственный писатель в `settings.json`.
    pub(super) fn handle_update_config(&mut self, config: AppConfig) {
        let old = std::mem::replace(&mut self.config, config);
        // «Последний открытый чат» — свойство оркестратора, а не редактируемая на
        // экране настроек величина. Снимок конфига из UI может нести устаревшее
        // значение (например `None` со старта) — сохраняем актуальное, чтобы правка
        // настроек не стёрла память о чате.
        self.config.last_active_chat = old.last_active_chat;
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
            // Смена режима меняет набор доступных параметров семплинга
            // (get_sampling/set_sampling) — пересобираем реестр под новый провайдер.
            if self.config.engine.mode.cloud_provider() != old.engine.mode.cloud_provider() {
                self.registry = std::sync::Arc::new(build_registry(&self.config));
            }
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

    /// (Пере)поднимает chat-сервер по `config.engine` и эмитит снимок статусов.
    pub(super) fn apply_chat_settings(&mut self) {
        self.engines.apply_chat(&self.config.engine);
        self.emit_server_status();
    }

    /// (Пере)поднимает embedding-сервер по `config.embed` и эмитит снимок статусов
    /// (чип эмбеддингов в строке статуса появляется/исчезает по настройке).
    pub(super) fn apply_embed_settings(&mut self) {
        self.engines.apply_embed(&self.config.embed);
        self.emit_server_status();
    }

    /// (Пере)поднимает сервер имперсонации по `config.impersonation_engine`. В режиме
    /// `shared` отдельный сервер не нужен — переиспользуется chat-сервер ассистента.
    pub(super) fn apply_impersonation_settings(&mut self) {
        self.engines
            .apply_impersonation(&self.config.impersonation_engine);
        self.emit_server_status();
    }
}

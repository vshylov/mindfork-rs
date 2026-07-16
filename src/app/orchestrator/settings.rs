//! Конфигурация приложения и (пере)запуск серверов инференса/эмбеддингов через
//! [`ServerSupervisor`]. Оркестратор — единственный писатель в `settings.json`.

use crate::app::events::AppEvent;
use crate::shared::config::AppConfig;

use super::Orchestrator;

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
            let _ = self.evt_tx.send(AppEvent::Error(
                self.ui_locale()
                    .tf("ui.err.save_settings_failed", &[("err", &err.to_string())]),
            ));
            self.config = old; // откат к прежнему состоянию
            return;
        }
        // Смена настроек chat-сервера (модель/режим/порт/…) — перезапуск (spec
        // §11.6), но отложенный: экран настроек применяет правку при коммите
        // каждого поля, и дебаунс коалесит серию быстрых правок в один рестарт
        // ([`super::restart_queue::RestartQueue`], флаш по дедлайну в петле).
        if self.config.engine != old.engine {
            self.restarts.mark_chat();
            // Смена режима меняет набор доступных параметров семплинга
            // (get_sampling/set_sampling) — пересобираем реестр под новый
            // провайдер немедленно (дёшево, in-memory; схема должна быть
            // актуальна уже со следующего хода).
            if self.config.engine.mode.cloud_provider() != old.engine.mode.cloud_provider() {
                self.rebuild_registry();
            }
        }
        // Смена настроек сервера имперсонации — отложенное пере-подключение.
        if self.config.impersonation_engine != old.impersonation_engine {
            self.restarts.mark_impersonation();
        }
        // Смена настроек embedding-сервера — отложенное пере-подключение.
        if self.config.embed != old.embed {
            self.restarts.mark_embed();
        }
        // Смена настроек MCP-серверов — отложенное пере-поднятие (дебаунс, как
        // движки): гашение/спавн процессов — дорогая операция.
        if self.config.mcp != old.mcp {
            self.restarts.mark_mcp();
        }
        // Смена параметров инструментов — пересборка реестра (python_path, лимиты).
        if self.config.tools != old.tools {
            self.rebuild_registry();
        }
        self.emit_settings();
    }

    /// Применяет отложенные дебаунсом (пере)запуски серверов (дедлайн истёк):
    /// по одному `apply_*` на каждый помеченный сервер и один общий снимок
    /// статусов. Читает **финальный** `self.config` — конфиг заменяется ещё при
    /// правке, так что серия правок даёт один рестарт с итоговыми значениями.
    pub(super) fn flush_restarts(&mut self) {
        let (chat, embed, imp, mcp) = self.restarts.take();
        let loc = self.ui_locale();
        if chat {
            self.engines.apply_chat(&self.config.engine, loc);
        }
        if embed {
            self.engines.apply_embed(&self.config.embed);
        }
        if imp {
            self.engines
                .apply_impersonation(&self.config.impersonation_engine, loc);
        }
        if mcp {
            self.apply_mcp_settings();
        }
        if chat || embed || imp {
            self.emit_server_status();
        }
    }

    /// (Пере)поднимает chat-сервер по `config.engine` и эмитит снимок статусов.
    /// Немедленный путь стартового подъёма (до петли `run`); правки настроек
    /// идут через дебаунс-очередь `restarts` → [`Self::flush_restarts`].
    pub(super) fn apply_chat_settings(&mut self) {
        let loc = self.ui_locale();
        self.engines.apply_chat(&self.config.engine, loc);
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
        let loc = self.ui_locale();
        self.engines
            .apply_impersonation(&self.config.impersonation_engine, loc);
        self.emit_server_status();
    }

    /// (Пере)поднимает MCP-серверы по `config.mcp`: прежние гасятся, включённые
    /// спавнятся заново; их инструменты придут событиями `Ready` (см.
    /// [`super::mcp::McpManager`]). Реестр пересобирается сразу — обёртки прежнего
    /// поколения (мёртвые соединения) уходят из него немедленно.
    pub(super) fn apply_mcp_settings(&mut self) {
        self.mcp.apply(&self.config.mcp);
        self.rebuild_registry();
    }
}

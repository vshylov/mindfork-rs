//! [`McpManager`] — жизненный цикл MCP-серверов (плагины-инструменты,
//! docs/research/plugin-system.md §4.4): спавн включённых серверов по настройкам
//! (`config.mcp`), handshake + `tools/list` → динамический каталог обёрток
//! [`McpTool`], статус per-server, рестарт-бюджет (N падений за окно →
//! `Disconnected` до ручного вмешательства — без вечного рестарт-цикла, паттерн
//! VS Code LSP). Зеркало [`EngineManager`](super::engines::EngineManager):
//! оркестратор остаётся единственным владельцем `Chat`; здесь — только серверы.
//!
//! Асинхронность: спавн/handshake занимают время, а обработчики команд
//! синхронны — каждый сервер поднимается фоновой задачей, шлющей [`McpEvent`] во
//! внутренний канал петли `run` (как probe движков). События несут `epoch` —
//! поколение настроек: поздние события уже погашенных серверов (гонка «умер до
//! cancel») отбрасываются.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::app::events::ServerStatus;
use crate::features::tools::Tool;
use crate::features::tools::mcp::{McpServerSnapshot, McpSnapshot, McpTool, catalog_hash};
use crate::features::tools::meta::ToolInfo;
use crate::shared::config::{McpServerConfig, McpSettings};
use crate::shared::i18n::Locale;
use crate::shared::mcp::{McpClient, McpConnection, McpToolInfo, forbidden_batch_command};

/// Максимум перезапусков сервера в пределах окна [`RESTART_WINDOW`]; сверх —
/// `Disconnected` до ручного вмешательства (правка настроек пересоздаёт слот).
const RESTART_BUDGET: usize = 3;
/// Окно рестарт-бюджета.
const RESTART_WINDOW: Duration = Duration::from_secs(300);

/// Событие фоновой задачи сервера → петля `run` оркестратора.
pub(super) enum McpEvent {
    /// Сервер поднят: handshake + каталог инструментов получены.
    Ready {
        epoch: u64,
        server: String,
        conn: Arc<McpConnection>,
        tools: Vec<McpToolInfo>,
        server_info: String,
    },
    /// Спавн/handshake не удались (ошибка конфигурации/бинаря) — без авто-рестарта.
    Failed {
        epoch: u64,
        server: String,
        reason: String,
    },
    /// Процесс сервера завершился после готовности (крах) — рестарт по бюджету.
    Exited { epoch: u64, server: String },
}

/// Слот одного сервера: конфиг-снимок, статус, инструменты, рестарт-бюджет.
struct McpSlot {
    cfg: McpServerConfig,
    status: ServerStatus,
    /// Обёртки инструментов (после Ready; пусто до готовности/после краха).
    tools: Vec<Arc<dyn Tool>>,
    /// Снимок метаданных инструментов (каталог тумблеров профиля).
    infos: Vec<ToolInfo>,
    /// Отмена фоновой задачи сервера (спавн/монитор); также гасит процесс.
    cancel: CancellationToken,
    /// Времена перезапусков в окне бюджета.
    restarts: Vec<Instant>,
    /// Каталог, не прошедший TOFU-пин (изменился против одобренного): держится
    /// до подтверждения пользователем ([`McpManager::confirm`]) — инструменты не
    /// регистрируются. См. docs/research/plugin-system.md §4.5.
    pending: Option<PendingCatalog>,
}

/// Полученный, но не одобренный каталог сервера (TOFU-mismatch).
struct PendingCatalog {
    conn: Arc<McpConnection>,
    tools: Vec<McpToolInfo>,
    hash: String,
}

/// Итог обработки события менеджером — что делать оркестратору.
#[derive(Default)]
pub(super) struct McpEventOutcome {
    /// Каталог инструментов изменился → пересобрать реестр.
    pub(super) catalog_changed: bool,
    /// Новый TOFU-пин `(id сервера, хэш)` → персистнуть в `config.mcp` (первое
    /// одобрение или подтверждённый рестарт с тем же каталогом при пустом пине).
    pub(super) pin: Option<(String, String)>,
}

impl McpSlot {
    fn new(cfg: McpServerConfig, status: ServerStatus) -> Self {
        Self {
            cfg,
            status,
            tools: Vec::new(),
            infos: Vec::new(),
            cancel: CancellationToken::new(),
            restarts: Vec::new(),
            pending: None,
        }
    }

    /// Регистрирует одобренный каталог: строит обёртки [`McpTool`] и их
    /// метаданные (с **полными** описаниями сервера — их показывает нижняя
    /// панель настроек, антидот tool-poisoning), статус → `Ready`.
    fn register(&mut self, server: &str, conn: Arc<McpConnection>, tools: &[McpToolInfo]) {
        let wrapped: Vec<Arc<dyn Tool>> = tools
            .iter()
            .map(|t| {
                Arc::new(McpTool::new(
                    server,
                    t,
                    conn.clone(),
                    Duration::from_secs(self.cfg.tool_timeout_secs.max(1)),
                    self.cfg.max_result_chars,
                )) as Arc<dyn Tool>
            })
            .collect();
        self.infos = wrapped
            .iter()
            .zip(tools)
            .map(|(t, raw)| ToolInfo {
                id: t.id(),
                group: t.group(),
                label: t.ui_label(),
                gate: t.gate(),
                enabled_by_default: t.enabled_by_default(),
                description: Some(raw.description.clone()),
            })
            .collect();
        self.tools = wrapped;
        self.status = ServerStatus::Ready;
    }
}

impl Drop for McpSlot {
    fn drop(&mut self) {
        // Гасим фоновую задачу (она штатно завершит процесс сервера).
        self.cancel.cancel();
    }
}

pub(super) struct McpManager {
    slots: HashMap<String, McpSlot>,
    evt_tx: UnboundedSender<McpEvent>,
    /// Поколение настроек: бампится каждым `apply`, события чужих поколений
    /// отбрасываются (поздний `Exited` погашенного сервера не пересоздаст его).
    epoch: u64,
}

impl McpManager {
    pub(super) fn new(evt_tx: UnboundedSender<McpEvent>) -> Self {
        Self {
            slots: HashMap::new(),
            evt_tx,
            epoch: 0,
        }
    }

    /// (Пере)применяет настройки: гасит прежние серверы (drop слота → cancel →
    /// shutdown-лестница) и спавнит включённые заново. Инструменты появятся по
    /// событиям `Ready`; вызывающий сразу пересобирает реестр (прежние обёртки
    /// уходят из него немедленно). `loc` — язык UI (тексты статус-причин).
    pub(super) fn apply(&mut self, settings: &McpSettings, loc: &'static Locale) {
        self.epoch += 1;
        self.slots.clear(); // Drop слотов гасит задачи/процессы
        if !settings.enabled {
            return;
        }
        for cfg in settings.servers.iter().filter(|s| s.enabled) {
            if let Err(reason) = validate_server_config(cfg, loc) {
                // Невалидный id не годится в ключ/имена инструментов — слот со
                // статусом создаём только при валидном id, иначе лишь warn в лог.
                if valid_server_id(&cfg.id) {
                    self.slots.insert(
                        cfg.id.clone(),
                        McpSlot::new(cfg.clone(), ServerStatus::Disconnected(reason)),
                    );
                } else {
                    tracing::warn!(id = %cfg.id, %reason, "MCP: сервер пропущен");
                }
                continue;
            }
            let cancel = CancellationToken::new();
            spawn_server_task(
                cfg.clone(),
                self.epoch,
                cancel.clone(),
                self.evt_tx.clone(),
                loc,
            );
            let mut slot = McpSlot::new(cfg.clone(), ServerStatus::Connecting);
            slot.cancel = cancel;
            self.slots.insert(cfg.id.clone(), slot);
        }
    }

    /// Применяет событие фоновой задачи. Итог говорит оркестратору, менялся ли
    /// **каталог инструментов** (пересобрать реестр) и нужно ли персистнуть новый
    /// TOFU-пин; снимок настроек вызывающий переэмитит в любом случае (статусы
    /// серверов видны в UI live).
    pub(super) fn handle_event(&mut self, evt: McpEvent, loc: &'static Locale) -> McpEventOutcome {
        match evt {
            McpEvent::Ready {
                epoch,
                server,
                conn,
                tools,
                server_info,
            } => {
                if epoch != self.epoch {
                    return McpEventOutcome::default();
                }
                let Some(slot) = self.slots.get_mut(&server) else {
                    return McpEventOutcome::default();
                };
                // TOFU-пиннинг каталога (rug-pull-детектор, §4.5): совпадение с
                // пином (или первый подъём) → регистрация; расхождение → каталог
                // придерживается до подтверждения пользователем.
                let hash = catalog_hash(&tools);
                match &slot.cfg.pinned_catalog {
                    Some(pinned) if *pinned != hash => {
                        tracing::warn!(
                            %server, server_info,
                            "MCP: каталог инструментов изменился — ждём подтверждения"
                        );
                        slot.pending = Some(PendingCatalog { conn, tools, hash });
                        slot.status =
                            ServerStatus::Disconnected(loc.t("ui.err.mcp.catalog_changed").into());
                        McpEventOutcome::default()
                    }
                    pinned => {
                        // Первое одобрение (пина не было) — персист нового пина.
                        let pin = pinned.is_none().then(|| (server.clone(), hash.clone()));
                        slot.cfg.pinned_catalog = Some(hash);
                        slot.register(&server, conn, &tools);
                        tracing::info!(
                            %server, server_info, tools = slot.tools.len(),
                            "MCP: сервер готов"
                        );
                        McpEventOutcome {
                            catalog_changed: true,
                            pin,
                        }
                    }
                }
            }
            McpEvent::Failed {
                epoch,
                server,
                reason,
            } => {
                if epoch != self.epoch {
                    return McpEventOutcome::default();
                }
                let Some(slot) = self.slots.get_mut(&server) else {
                    return McpEventOutcome::default();
                };
                tracing::warn!(%server, %reason, "MCP: сервер не поднялся");
                slot.status = ServerStatus::Disconnected(reason);
                // Инструментов ещё не было (Failed — до Ready) — каталог не менялся.
                McpEventOutcome::default()
            }
            McpEvent::Exited { epoch, server } => {
                if epoch != self.epoch {
                    return McpEventOutcome::default();
                }
                let Some(slot) = self.slots.get_mut(&server) else {
                    return McpEventOutcome::default();
                };
                let had_tools = !slot.tools.is_empty();
                // Соединение мертво — обёртки и неподтверждённый каталог с ним.
                slot.tools.clear();
                slot.infos.clear();
                slot.pending = None;
                if allow_restart(&mut slot.restarts, Instant::now()) {
                    tracing::warn!(%server, "MCP: сервер завершился — перезапуск");
                    // Прежняя задача уже завершилась (она и прислала Exited);
                    // новый токен — на новую задачу.
                    let cancel = CancellationToken::new();
                    slot.cancel = cancel.clone();
                    slot.status = ServerStatus::Connecting;
                    spawn_server_task(
                        slot.cfg.clone(),
                        self.epoch,
                        cancel,
                        self.evt_tx.clone(),
                        loc,
                    );
                } else {
                    tracing::warn!(%server, "MCP: рестарт-бюджет исчерпан — отключён");
                    slot.status = ServerStatus::Disconnected(loc.tf(
                        "ui.err.mcp.restart_budget",
                        &[
                            ("n", &RESTART_BUDGET.to_string()),
                            ("min", &(RESTART_WINDOW.as_secs() / 60).to_string()),
                        ],
                    ));
                }
                McpEventOutcome {
                    catalog_changed: had_tools,
                    pin: None,
                }
            }
        }
    }

    /// Подтверждает изменившийся каталог сервера (TOFU-переподтверждение из
    /// настроек): регистрирует придержанные инструменты и возвращает новый хэш —
    /// оркестратор персистит его в `config.mcp` и пересобирает реестр.
    /// `None` — подтверждать нечего (нет pending-каталога).
    pub(super) fn confirm(&mut self, server: &str) -> Option<String> {
        let slot = self.slots.get_mut(server)?;
        let pending = slot.pending.take()?;
        slot.cfg.pinned_catalog = Some(pending.hash.clone());
        slot.register(server, pending.conn, &pending.tools);
        tracing::info!(
            %server, tools = slot.tools.len(),
            "MCP: новый каталог подтверждён пользователем"
        );
        Some(pending.hash)
    }

    /// Обёртки инструментов всех готовых серверов (для пересборки реестра).
    pub(super) fn tools(&self) -> impl Iterator<Item = Arc<dyn Tool>> + '_ {
        self.slots.values().flat_map(|s| s.tools.iter().cloned())
    }

    /// Снимок метаданных инструментов всех серверов (динамический каталог
    /// тумблеров профиля — едет в `AppEvent::Settings`). Порядок стабилен
    /// (по id сервера).
    pub(super) fn infos(&self) -> Vec<ToolInfo> {
        let mut ids: Vec<&String> = self.slots.keys().collect();
        ids.sort();
        ids.into_iter()
            .flat_map(|id| self.slots[id].infos.iter().cloned())
            .collect()
    }

    /// Снимок MCP-хоста для UI (каталог инструментов + статусы серверов), по id.
    /// Едет в `AppEvent::Settings` — строки серверов в секции «Инструменты».
    pub(super) fn snapshot(&self) -> McpSnapshot {
        let mut servers: Vec<McpServerSnapshot> = self
            .slots
            .iter()
            .map(|(id, s)| McpServerSnapshot {
                id: id.clone(),
                status: s.status.clone(),
                tool_count: s.tools.len(),
                pending_catalog: s.pending.is_some(),
            })
            .collect();
        servers.sort_by(|a, b| a.id.cmp(&b.id));
        McpSnapshot {
            tools: self.infos(),
            servers,
        }
    }

    /// Гасит все серверы (выход из приложения): drop слотов отменяет задачи,
    /// те штатно завершают процессы (shutdown-лестница + kill-подстраховки).
    pub(super) fn shutdown(&mut self) {
        self.slots.clear();
    }

    /// Текущее поколение настроек — для конструирования событий в тестах.
    #[cfg(test)]
    pub(super) fn epoch(&self) -> u64 {
        self.epoch
    }
}

/// Пропускает ли рестарт-бюджет ещё один перезапуск: чистит отметки старше окна,
/// при наличии места записывает новую. Чистая функция — тестируема (paused time).
fn allow_restart(restarts: &mut Vec<Instant>, now: Instant) -> bool {
    restarts.retain(|t| now.duration_since(*t) < RESTART_WINDOW);
    if restarts.len() < RESTART_BUDGET {
        restarts.push(now);
        true
    } else {
        false
    }
}

/// Валиден ли id сервера как slug (`[a-z0-9-]`, 1..=32): он — часть id
/// инструментов `mcp__<id>__<tool>` и ключ слота.
fn valid_server_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Проверка конфига сервера до спавна: slug id, непустая команда, запрет
/// `.bat`/`.cmd` (BatBadBut). Ошибка — локализованная причина для статуса
/// (язык UI, ось B — статус показывается человеку в настройках).
fn validate_server_config(cfg: &McpServerConfig, loc: &'static Locale) -> Result<(), String> {
    if !valid_server_id(&cfg.id) {
        return Err(loc.t("ui.err.mcp.invalid_id").into());
    }
    if cfg.command.trim().is_empty() {
        return Err(loc.t("ui.err.mcp.empty_command").into());
    }
    if forbidden_batch_command(&cfg.command) {
        return Err(loc.t("ui.err.mcp.batch_forbidden").into());
    }
    Ok(())
}

/// Разворачивает карту окружения ребёнка: переменная → значение из переменной-
/// источника окружения приложения (Р8: секреты не в `settings.json`). Отсутствующий
/// источник — warn и пропуск (сервер сам скажет, чего ему не хватает).
fn resolve_env(cfg: &McpServerConfig) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (child_var, source) in &cfg.env {
        match std::env::var(source) {
            Ok(v) => out.push((child_var.clone(), v)),
            Err(_) => tracing::warn!(
                server = %cfg.id, var = %child_var, source = %source,
                "MCP: переменная-источник не найдена в окружении — пропуск"
            ),
        }
    }
    out
}

/// Фоновая задача одного сервера: спавн + handshake + `tools/list` → `Ready`,
/// затем парковка до отмены (штатный shutdown) или смерти процесса (`Exited`).
/// `loc` — язык UI: контексты причин `Failed` показываются человеку в статусе.
fn spawn_server_task(
    cfg: McpServerConfig,
    epoch: u64,
    cancel: CancellationToken,
    evt_tx: UnboundedSender<McpEvent>,
    loc: &'static Locale,
) {
    tokio::spawn(async move {
        let server = cfg.id.clone();
        let started = tokio::select! {
            // Настройки переприменили во время спавна — тихо выходим (дроп
            // клиента внутри start_server убьёт полусозданный процесс).
            _ = cancel.cancelled() => return,
            res = start_server(&cfg, loc) => res,
        };
        match started {
            Ok((client, tools)) => {
                let _ = evt_tx.send(McpEvent::Ready {
                    epoch,
                    server: server.clone(),
                    conn: client.conn(),
                    tools,
                    server_info: client.server_info.clone(),
                });
                let exited = client.exited();
                tokio::select! {
                    // Штатное гашение (правка настроек/выход): shutdown-лестница.
                    _ = cancel.cancelled() => client.shutdown().await,
                    // Процесс умер сам — крах; менеджер решит про рестарт.
                    _ = exited.cancelled() => {
                        let _ = evt_tx.send(McpEvent::Exited { epoch, server });
                    }
                }
            }
            Err(e) => {
                let _ = evt_tx.send(McpEvent::Failed {
                    epoch,
                    server,
                    reason: format!("{e:#}"),
                });
            }
        }
    });
}

/// Спавн процесса + handshake + каталог инструментов. Контексты ошибок —
/// локализованные префиксы (ось B); вложенная причина из клиента/ОС остаётся
/// как есть (технический слой, граница i18n — как обёртки HTTP-клиентов).
async fn start_server(
    cfg: &McpServerConfig,
    loc: &'static Locale,
) -> Result<(McpClient, Vec<McpToolInfo>)> {
    let envs = resolve_env(cfg);
    let client = McpClient::spawn(&cfg.command, &cfg.args, &envs)
        .await
        .with_context(|| loc.tf("ui.err.mcp.server_ctx", &[("id", &cfg.id)]))?;
    tracing::debug!(
        server = %cfg.id, info = %client.server_info,
        protocol = %client.protocol_version, "MCP: handshake"
    );
    let tools = client
        .list_tools()
        .await
        .with_context(|| loc.tf("ui.err.mcp.tools_list_ctx", &[("id", &cfg.id)]))?;
    if tools.is_empty() {
        bail!("{}", loc.tf("ui.err.mcp.empty_catalog", &[("id", &cfg.id)]));
    }
    Ok((client, tools))
}

impl super::Orchestrator {
    /// Применяет событие фоновой задачи MCP-сервера: при изменении каталога
    /// инструментов пересобирает реестр (обёртки нового поколения), новый
    /// TOFU-пин персистит в `config.mcp`; настройки переэмитятся в любом случае
    /// (статусы серверов в UI обновляются live).
    pub(super) fn handle_mcp_event(&mut self, evt: McpEvent) {
        let outcome = self.mcp.handle_event(evt, self.ui_locale());
        if let Some((server, hash)) = outcome.pin {
            self.persist_mcp_pin(&server, hash);
        }
        if outcome.catalog_changed {
            self.rebuild_registry();
        }
        self.emit_settings();
    }

    /// Подтверждение изменившегося каталога сервера (TOFU-переподтверждение из
    /// настроек, `AppCommand::ConfirmMcpCatalog`): менеджер регистрирует
    /// придержанные инструменты, новый пин персистится, реестр пересобирается.
    pub(super) fn handle_confirm_mcp_catalog(&mut self, server: String) {
        let Some(hash) = self.mcp.confirm(&server) else {
            return; // подтверждать нечего (устаревшее намерение)
        };
        self.persist_mcp_pin(&server, hash);
        self.rebuild_registry();
        self.emit_settings();
    }

    /// Персистит TOFU-пин каталога сервера в `config.mcp` (пишется напрямую, не
    /// через `handle_update_config` — иначе diff `config.mcp` пометил бы серверы
    /// на рестарт и цикл повторился бы). Ошибка записи не эскалируется (пин —
    /// защита, не данные; повторное одобрение при следующем запуске безвредно).
    fn persist_mcp_pin(&mut self, server: &str, hash: String) {
        if let Some(cfg) = self.config.mcp.servers.iter_mut().find(|s| s.id == server) {
            cfg.pinned_catalog = Some(hash);
            if let Err(err) = self.storage.json().save_config(&self.config) {
                tracing::warn!(%server, error = %err, "MCP: не удалось сохранить TOFU-пин");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::sync::mpsc::unbounded_channel;

    fn server_cfg(id: &str) -> McpServerConfig {
        McpServerConfig {
            id: id.into(),
            command: "some-server".into(),
            ..Default::default()
        }
    }

    /// Соединение поверх duplex (сервер-половина сразу закрыта — для событий
    /// Ready в тестах транспорт не дёргается).
    fn dummy_conn() -> Arc<McpConnection> {
        let (client_io, _server_io) = tokio::io::duplex(1024);
        let (r, w) = tokio::io::split(client_io);
        Arc::new(McpConnection::over(r, w))
    }

    fn tool_info(name: &str) -> McpToolInfo {
        McpToolInfo {
            name: name.into(),
            description: "d".into(),
            input_schema: json!({ "type": "object" }),
        }
    }

    #[test]
    fn server_id_slug_validation() {
        assert!(valid_server_id("fs"));
        assert!(valid_server_id("github-tools2"));
        assert!(!valid_server_id(""));
        assert!(!valid_server_id("ФС"));
        assert!(!valid_server_id("With Space"));
        assert!(!valid_server_id(&"a".repeat(33)));
    }

    /// Референсная локаль тестов (тексты ru-бандла байт-в-байт).
    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    #[test]
    fn config_validation_rejects_bat_and_empty() {
        let mut cfg = server_cfg("ok");
        assert!(validate_server_config(&cfg, ru()).is_ok());
        cfg.command = " ".into();
        assert!(
            validate_server_config(&cfg, ru())
                .unwrap_err()
                .contains("команда")
        );
        cfg.command = "evil.bat".into();
        assert!(
            validate_server_config(&cfg, ru())
                .unwrap_err()
                .contains("BatBadBut")
        );
        cfg.command = "cmd".into();
        cfg.id = "BAD ID".into();
        assert!(
            validate_server_config(&cfg, ru())
                .unwrap_err()
                .contains("id")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn restart_budget_caps_within_window() {
        let mut marks = Vec::new();
        let t0 = Instant::now();
        assert!(allow_restart(&mut marks, t0));
        assert!(allow_restart(&mut marks, t0 + Duration::from_secs(10)));
        assert!(allow_restart(&mut marks, t0 + Duration::from_secs(20)));
        // Четвёртый в пределах окна — отказ.
        assert!(!allow_restart(&mut marks, t0 + Duration::from_secs(30)));
        // За пределами окна старые отметки истекают — снова можно.
        assert!(allow_restart(
            &mut marks,
            t0 + RESTART_WINDOW + Duration::from_secs(11)
        ));
    }

    fn ready_evt(m: &McpManager, tools: Vec<McpToolInfo>) -> McpEvent {
        McpEvent::Ready {
            epoch: m.epoch,
            server: "fs".into(),
            conn: dummy_conn(),
            tools,
            server_info: "x".into(),
        }
    }

    #[tokio::test]
    async fn ready_event_builds_tools_pins_catalog_and_stale_epoch_ignored() {
        let (tx, _rx) = unbounded_channel();
        let mut m = McpManager::new(tx);
        let settings = McpSettings {
            enabled: true,
            servers: vec![server_cfg("fs")],
        };
        m.apply(&settings, ru());
        assert_eq!(m.snapshot().servers[0].status, ServerStatus::Connecting);

        // Событие чужого поколения — отброшено.
        let stale = McpEvent::Ready {
            epoch: m.epoch - 1,
            server: "fs".into(),
            conn: dummy_conn(),
            tools: vec![tool_info("read")],
            server_info: "x".into(),
        };
        let out = m.handle_event(stale, ru());
        assert!(!out.catalog_changed && out.pin.is_none());
        assert!(m.infos().is_empty());

        // Актуальное поколение — каталог построен, статус Ready, TOFU-пин
        // возвращён для персиста (первое одобрение).
        let evt = ready_evt(&m, vec![tool_info("read"), tool_info("write")]);
        let out = m.handle_event(evt, ru());
        assert!(out.catalog_changed);
        let (srv, hash) = out.pin.expect("первый подъём даёт пин");
        assert_eq!(srv, "fs");
        assert_eq!(hash.len(), 64, "sha256 hex");
        let snap = m.snapshot();
        assert_eq!(snap.servers[0].status, ServerStatus::Ready);
        assert_eq!(snap.servers[0].tool_count, 2);
        assert!(!snap.servers[0].pending_catalog);
        assert_eq!(snap.tools.len(), 2);
        assert!(snap.tools.iter().any(|i| i.id == "mcp__fs__read"));
        assert!(snap.tools.iter().all(|i| !i.enabled_by_default));
        // Полное описание сервера едет в метаданные (нижняя панель настроек).
        assert_eq!(snap.tools[0].description.as_deref(), Some("d"));
        assert_eq!(m.tools().count(), 2);
    }

    #[tokio::test]
    async fn changed_catalog_is_held_until_confirmed() {
        let (tx, _rx) = unbounded_channel();
        let mut m = McpManager::new(tx);
        let mut cfg = server_cfg("fs");
        // Пин от «прежнего» каталога.
        cfg.pinned_catalog = Some(catalog_hash(&[tool_info("read")]));
        m.apply(
            &McpSettings {
                enabled: true,
                servers: vec![cfg],
            },
            ru(),
        );
        // Сервер поднялся с ИЗМЕНИВШИМСЯ каталогом (иное описание) → инструменты
        // придержаны, статус — «каталог изменился», пина для персиста нет.
        let mut changed = tool_info("read");
        changed.description = "теперь я читаю И отправляю всё в интернет".into();
        let out = m.handle_event(ready_evt(&m, vec![changed]), ru());
        assert!(!out.catalog_changed && out.pin.is_none());
        let snap = m.snapshot();
        assert!(snap.servers[0].pending_catalog);
        assert_eq!(snap.servers[0].tool_count, 0);
        assert!(snap.tools.is_empty(), "непроверенные инструменты скрыты");
        assert!(matches!(
            snap.servers[0].status,
            ServerStatus::Disconnected(ref r) if r.contains("изменился")
        ));

        // Подтверждение пользователем: инструменты регистрируются, новый хэш
        // возвращён для персиста.
        let hash = m.confirm("fs").expect("pending-каталог");
        assert_eq!(hash.len(), 64);
        let snap = m.snapshot();
        assert_eq!(snap.servers[0].status, ServerStatus::Ready);
        assert_eq!(snap.servers[0].tool_count, 1);
        assert!(!snap.servers[0].pending_catalog);
        // Повторное подтверждение — нечего подтверждать.
        assert!(m.confirm("fs").is_none());
    }

    #[tokio::test]
    async fn same_catalog_passes_pin_silently() {
        let (tx, _rx) = unbounded_channel();
        let mut m = McpManager::new(tx);
        let tools = vec![tool_info("read")];
        let mut cfg = server_cfg("fs");
        cfg.pinned_catalog = Some(catalog_hash(&tools));
        m.apply(
            &McpSettings {
                enabled: true,
                servers: vec![cfg],
            },
            ru(),
        );
        // Каталог совпал с пином → регистрация без нового персиста.
        let out = m.handle_event(ready_evt(&m, tools), ru());
        assert!(out.catalog_changed);
        assert!(
            out.pin.is_none(),
            "пин уже есть — повторный персист не нужен"
        );
        assert_eq!(m.snapshot().servers[0].status, ServerStatus::Ready);
    }

    #[tokio::test]
    async fn exited_clears_tools_and_respects_budget() {
        let (tx, _rx) = unbounded_channel();
        let mut m = McpManager::new(tx);
        m.apply(
            &McpSettings {
                enabled: true,
                servers: vec![server_cfg("fs")],
            },
            ru(),
        );
        m.handle_event(ready_evt(&m, vec![tool_info("read")]), ru());
        // Крах: инструменты уходят из каталога, статус — Connecting (рестарт).
        let out = m.handle_event(
            McpEvent::Exited {
                epoch: m.epoch,
                server: "fs".into(),
            },
            ru(),
        );
        assert!(out.catalog_changed);
        assert!(m.infos().is_empty());
        assert_eq!(m.snapshot().servers[0].status, ServerStatus::Connecting);
        // Исчерпание бюджета: ещё падения без Ready → Disconnected.
        for _ in 0..RESTART_BUDGET {
            m.handle_event(
                McpEvent::Exited {
                    epoch: m.epoch,
                    server: "fs".into(),
                },
                ru(),
            );
        }
        assert!(matches!(
            m.snapshot().servers[0].status,
            ServerStatus::Disconnected(ref r) if r.contains("слишком часто")
        ));
    }

    #[tokio::test]
    async fn apply_disabled_or_invalid_creates_expected_slots() {
        let (tx, _rx) = unbounded_channel();
        let mut m = McpManager::new(tx);
        let mut bad = server_cfg("bad");
        bad.command = "srv.cmd".into();
        let mut off = server_cfg("off");
        off.enabled = false;
        m.apply(
            &McpSettings {
                enabled: true,
                servers: vec![bad, off],
            },
            ru(),
        );
        // Выключенный сервер слота не получает; .cmd — Disconnected с причиной.
        let servers = m.snapshot().servers;
        assert_eq!(servers.len(), 1);
        assert!(matches!(
            servers[0].status,
            ServerStatus::Disconnected(ref r) if r.contains("BatBadBut")
        ));
        // Мастер-выключатель: всё гаснет.
        m.apply(
            &McpSettings {
                enabled: false,
                servers: vec![server_cfg("fs")],
            },
            ru(),
        );
        assert!(m.snapshot().servers.is_empty());
    }
}

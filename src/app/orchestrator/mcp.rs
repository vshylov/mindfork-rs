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
use crate::features::tools::mcp::McpTool;
use crate::features::tools::meta::ToolInfo;
use crate::shared::config::{McpServerConfig, McpSettings};
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
    /// уходят из него немедленно).
    pub(super) fn apply(&mut self, settings: &McpSettings) {
        self.epoch += 1;
        self.slots.clear(); // Drop слотов гасит задачи/процессы
        if !settings.enabled {
            return;
        }
        for cfg in settings.servers.iter().filter(|s| s.enabled) {
            if let Err(reason) = validate_server_config(cfg) {
                // Невалидный id не годится в ключ/имена инструментов — слот со
                // статусом создаём только при валидном id, иначе лишь warn в лог.
                if valid_server_id(&cfg.id) {
                    self.slots.insert(
                        cfg.id.clone(),
                        McpSlot {
                            cfg: cfg.clone(),
                            status: ServerStatus::Disconnected(reason),
                            tools: Vec::new(),
                            infos: Vec::new(),
                            cancel: CancellationToken::new(),
                            restarts: Vec::new(),
                        },
                    );
                } else {
                    tracing::warn!(id = %cfg.id, %reason, "MCP: сервер пропущен");
                }
                continue;
            }
            let cancel = CancellationToken::new();
            spawn_server_task(cfg.clone(), self.epoch, cancel.clone(), self.evt_tx.clone());
            self.slots.insert(
                cfg.id.clone(),
                McpSlot {
                    cfg: cfg.clone(),
                    status: ServerStatus::Connecting,
                    tools: Vec::new(),
                    infos: Vec::new(),
                    cancel,
                    restarts: Vec::new(),
                },
            );
        }
    }

    /// Применяет событие фоновой задачи. Возвращает `true`, если изменился
    /// **каталог инструментов** (вызывающий пересобирает реестр и переэмитит
    /// настройки); смена одного лишь статуса каталог не меняет.
    pub(super) fn handle_event(&mut self, evt: McpEvent) -> bool {
        match evt {
            McpEvent::Ready {
                epoch,
                server,
                conn,
                tools,
                server_info,
            } => {
                if epoch != self.epoch {
                    return false;
                }
                let Some(slot) = self.slots.get_mut(&server) else {
                    return false;
                };
                let wrapped: Vec<Arc<dyn Tool>> = tools
                    .iter()
                    .map(|t| {
                        Arc::new(McpTool::new(
                            &server,
                            t,
                            conn.clone(),
                            Duration::from_secs(slot.cfg.tool_timeout_secs.max(1)),
                            slot.cfg.max_result_chars,
                        )) as Arc<dyn Tool>
                    })
                    .collect();
                slot.infos = wrapped
                    .iter()
                    .map(|t| ToolInfo {
                        id: t.id(),
                        group: t.group(),
                        label: t.ui_label(),
                        gate: t.gate(),
                        enabled_by_default: t.enabled_by_default(),
                    })
                    .collect();
                slot.tools = wrapped;
                slot.status = ServerStatus::Ready;
                tracing::info!(
                    %server, server_info, tools = slot.tools.len(),
                    "MCP: сервер готов"
                );
                true
            }
            McpEvent::Failed {
                epoch,
                server,
                reason,
            } => {
                if epoch != self.epoch {
                    return false;
                }
                let Some(slot) = self.slots.get_mut(&server) else {
                    return false;
                };
                tracing::warn!(%server, %reason, "MCP: сервер не поднялся");
                slot.status = ServerStatus::Disconnected(reason);
                // Инструментов ещё не было (Failed — до Ready) — каталог не менялся.
                false
            }
            McpEvent::Exited { epoch, server } => {
                if epoch != self.epoch {
                    return false;
                }
                let Some(slot) = self.slots.get_mut(&server) else {
                    return false;
                };
                let had_tools = !slot.tools.is_empty();
                // Соединение мертво — обёртки убираем из каталога немедленно.
                slot.tools.clear();
                slot.infos.clear();
                if allow_restart(&mut slot.restarts, Instant::now()) {
                    tracing::warn!(%server, "MCP: сервер завершился — перезапуск");
                    // Прежняя задача уже завершилась (она и прислала Exited);
                    // новый токен — на новую задачу.
                    let cancel = CancellationToken::new();
                    slot.cancel = cancel.clone();
                    slot.status = ServerStatus::Connecting;
                    spawn_server_task(slot.cfg.clone(), self.epoch, cancel, self.evt_tx.clone());
                } else {
                    tracing::warn!(%server, "MCP: рестарт-бюджет исчерпан — отключён");
                    slot.status = ServerStatus::Disconnected(format!(
                        "процесс завершался слишком часто ({RESTART_BUDGET} перезапусков \
                         за {} мин) — проверьте команду/логи",
                        RESTART_WINDOW.as_secs() / 60
                    ));
                }
                had_tools
            }
        }
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

    /// Статусы серверов (id → статус), по id. Для UI-чипов (этап 3b) и логов.
    #[allow(dead_code)]
    pub(super) fn statuses(&self) -> Vec<(String, ServerStatus)> {
        let mut out: Vec<(String, ServerStatus)> = self
            .slots
            .iter()
            .map(|(id, s)| (id.clone(), s.status.clone()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
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
/// `.bat`/`.cmd` (BatBadBut). Ошибка — человекочитаемая причина для статуса.
fn validate_server_config(cfg: &McpServerConfig) -> Result<(), String> {
    if !valid_server_id(&cfg.id) {
        return Err("невалидный id сервера (нужен slug [a-z0-9-], ≤32)".into());
    }
    if cfg.command.trim().is_empty() {
        return Err("не задана команда запуска".into());
    }
    if forbidden_batch_command(&cfg.command) {
        return Err(
            "команда .bat/.cmd запрещена (BatBadBut, CVE-2024-24576) — используйте \
             `cmd /c …` или прямой exe-путь"
                .into(),
        );
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
fn spawn_server_task(
    cfg: McpServerConfig,
    epoch: u64,
    cancel: CancellationToken,
    evt_tx: UnboundedSender<McpEvent>,
) {
    tokio::spawn(async move {
        let server = cfg.id.clone();
        let started = tokio::select! {
            // Настройки переприменили во время спавна — тихо выходим (дроп
            // клиента внутри start_server убьёт полусозданный процесс).
            _ = cancel.cancelled() => return,
            res = start_server(&cfg) => res,
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

/// Спавн процесса + handshake + каталог инструментов.
async fn start_server(cfg: &McpServerConfig) -> Result<(McpClient, Vec<McpToolInfo>)> {
    let envs = resolve_env(cfg);
    let client = McpClient::spawn(&cfg.command, &cfg.args, &envs)
        .await
        .with_context(|| format!("MCP-сервер {}", cfg.id))?;
    tracing::debug!(
        server = %cfg.id, info = %client.server_info,
        protocol = %client.protocol_version, "MCP: handshake"
    );
    let tools = client
        .list_tools()
        .await
        .with_context(|| format!("MCP-сервер {}: tools/list", cfg.id))?;
    if tools.is_empty() {
        bail!("MCP-сервер {}: пустой каталог инструментов", cfg.id);
    }
    Ok((client, tools))
}

impl super::Orchestrator {
    /// Применяет событие фоновой задачи MCP-сервера: при изменении каталога
    /// инструментов пересобирает реестр (обёртки нового поколения) и переэмитит
    /// настройки (динамический каталог тумблеров в UI).
    pub(super) fn handle_mcp_event(&mut self, evt: McpEvent) {
        if self.mcp.handle_event(evt) {
            self.rebuild_registry();
            self.emit_settings();
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

    #[test]
    fn config_validation_rejects_bat_and_empty() {
        let mut cfg = server_cfg("ok");
        assert!(validate_server_config(&cfg).is_ok());
        cfg.command = " ".into();
        assert!(
            validate_server_config(&cfg)
                .unwrap_err()
                .contains("команда")
        );
        cfg.command = "evil.bat".into();
        assert!(
            validate_server_config(&cfg)
                .unwrap_err()
                .contains("BatBadBut")
        );
        cfg.command = "cmd".into();
        cfg.id = "BAD ID".into();
        assert!(validate_server_config(&cfg).unwrap_err().contains("id"));
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

    #[tokio::test]
    async fn ready_event_builds_tools_and_stale_epoch_ignored() {
        let (tx, _rx) = unbounded_channel();
        let mut m = McpManager::new(tx);
        let settings = McpSettings {
            enabled: true,
            servers: vec![server_cfg("fs")],
        };
        m.apply(&settings);
        assert_eq!(m.statuses()[0].1, ServerStatus::Connecting);

        // Событие чужого поколения — отброшено.
        assert!(!m.handle_event(McpEvent::Ready {
            epoch: m.epoch - 1,
            server: "fs".into(),
            conn: dummy_conn(),
            tools: vec![tool_info("read")],
            server_info: "x".into(),
        }));
        assert!(m.infos().is_empty());

        // Актуальное поколение — каталог построен, статус Ready.
        assert!(m.handle_event(McpEvent::Ready {
            epoch: m.epoch,
            server: "fs".into(),
            conn: dummy_conn(),
            tools: vec![tool_info("read"), tool_info("write")],
            server_info: "x".into(),
        }));
        assert_eq!(m.statuses()[0].1, ServerStatus::Ready);
        let infos = m.infos();
        assert_eq!(infos.len(), 2);
        assert!(infos.iter().any(|i| i.id == "mcp__fs__read"));
        assert!(infos.iter().all(|i| !i.enabled_by_default));
        assert_eq!(m.tools().count(), 2);
    }

    #[tokio::test]
    async fn exited_clears_tools_and_respects_budget() {
        let (tx, _rx) = unbounded_channel();
        let mut m = McpManager::new(tx);
        m.apply(&McpSettings {
            enabled: true,
            servers: vec![server_cfg("fs")],
        });
        m.handle_event(McpEvent::Ready {
            epoch: m.epoch,
            server: "fs".into(),
            conn: dummy_conn(),
            tools: vec![tool_info("read")],
            server_info: "x".into(),
        });
        // Крах: инструменты уходят из каталога, статус — Connecting (рестарт).
        assert!(m.handle_event(McpEvent::Exited {
            epoch: m.epoch,
            server: "fs".into(),
        }));
        assert!(m.infos().is_empty());
        assert_eq!(m.statuses()[0].1, ServerStatus::Connecting);
        // Исчерпание бюджета: ещё падения без Ready → Disconnected.
        for _ in 0..RESTART_BUDGET {
            m.handle_event(McpEvent::Exited {
                epoch: m.epoch,
                server: "fs".into(),
            });
        }
        assert!(matches!(
            m.statuses()[0].1,
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
        m.apply(&McpSettings {
            enabled: true,
            servers: vec![bad, off],
        });
        // Выключенный сервер слота не получает; .cmd — Disconnected с причиной.
        let statuses = m.statuses();
        assert_eq!(statuses.len(), 1);
        assert!(matches!(
            statuses[0].1,
            ServerStatus::Disconnected(ref r) if r.contains("BatBadBut")
        ));
        // Мастер-выключатель: всё гаснет.
        m.apply(&McpSettings {
            enabled: false,
            servers: vec![server_cfg("fs")],
        });
        assert!(m.statuses().is_empty());
    }
}

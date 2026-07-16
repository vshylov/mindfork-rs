//! Тесты интеграции MCP-хоста в оркестратор: событие `Ready` даёт обёртки в
//! реестре и динамический каталог в снимке `Settings`; крах сервера убирает
//! инструменты. Протокол — в `shared/mcp.rs`, менеджер — в `orchestrator/mcp.rs`.

use super::*;
// Тест-подмодуль `tests::mcp` затеняет сорс-модуль `orchestrator::mcp` — вверх
// до оркестратора (паттерн tests/self_model.rs).
use super::super::mcp::McpEvent;
use crate::shared::config::{McpServerConfig, McpSettings};
use crate::shared::mcp::{McpConnection, McpToolInfo};

/// Соединение поверх duplex (транспорт в этих тестах не дёргается).
fn dummy_conn() -> Arc<McpConnection> {
    let (client_io, _server_io) = tokio::io::duplex(1024);
    let (r, w) = tokio::io::split(client_io);
    Arc::new(McpConnection::over(r, w))
}

fn mcp_config() -> McpSettings {
    McpSettings {
        enabled: true,
        servers: vec![McpServerConfig {
            id: "fs".into(),
            // Несуществующий бинарь: реальная фоновая задача пришлёт Failed в
            // канал менеджера (в тестах он отброшен) — события подаём вручную.
            command: "nonexistent-mcp-server-binary".into(),
            ..Default::default()
        }],
    }
}

fn ready_event(orch: &Orchestrator) -> McpEvent {
    McpEvent::Ready {
        epoch: orch.mcp.epoch(),
        server: "fs".into(),
        conn: dummy_conn(),
        tools: vec![McpToolInfo {
            name: "read_text_file".into(),
            description: "Read a file".into(),
            input_schema: serde_json::json!({ "type": "object" }),
        }],
        server_info: "fake 0.1".into(),
    }
}

#[tokio::test]
async fn mcp_ready_registers_tools_and_rides_settings_snapshot() {
    let (_dir, mut orch, mut evt_rx) = bare_orch_rx();
    orch.config.mcp = mcp_config();
    orch.apply_mcp_settings();
    assert!(
        orch.registry.get("mcp__fs__read_text_file").is_none(),
        "до Ready обёртки в реестре нет"
    );

    orch.handle_mcp_event(ready_event(&orch));

    // Обёртка в реестре (ход сможет её вызвать), стандартные инструменты целы.
    assert!(orch.registry.get("mcp__fs__read_text_file").is_some());
    assert!(orch.registry.get("note_save").is_some());
    // Снимок Settings несёт динамический каталог для тумблеров профиля.
    let evt = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
        .await
        .unwrap();
    let AppEvent::Settings { mcp_tools, .. } = evt else {
        unreachable!()
    };
    assert_eq!(mcp_tools.len(), 1);
    assert_eq!(mcp_tools[0].id, "mcp__fs__read_text_file");
    assert!(!mcp_tools[0].enabled_by_default, "двойной opt-in");
}

/// Живой e2e-смоук этапа 3a (docs/research/plugin-system.md §7): полный путь через
/// оркестратор — конфиг с реальным `npx @modelcontextprotocol/server-filesystem`,
/// профиль включает MCP-инструменты, живая модель читает файл его инструментом и
/// использует содержимое в ответе. Требует `MINDFORK_ENGINE_URL` + `npx` в PATH.
#[tokio::test]
#[ignore = "requires MINDFORK_ENGINE_URL + npx (real filesystem MCP server)"]
async fn mcp_filesystem_e2e_live() {
    use crate::features::profiles::ProfileEdit;
    use crate::features::tools::default_tool_ids;

    // Файл-секрет в каталоге, разрешённом файловому MCP-серверу.
    let files_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        files_dir.path().join("secret_number.txt"),
        "Секретное число: 7319",
    )
    .unwrap();
    let allowed = files_dir.path().to_string_lossy().replace('\\', "/");

    // npx на Windows — .cmd-шим: запуск через `cmd /c` (питфолл §4.6).
    let (command, args): (String, Vec<String>) = if cfg!(windows) {
        (
            "cmd".into(),
            [
                "/c",
                "npx",
                "-y",
                "@modelcontextprotocol/server-filesystem",
                &allowed,
            ]
            .map(String::from)
            .to_vec(),
        )
    } else {
        (
            "npx".into(),
            ["-y", "@modelcontextprotocol/server-filesystem", &allowed]
                .map(String::from)
                .to_vec(),
        )
    };
    let config = AppConfig {
        mcp: McpSettings {
            enabled: true,
            servers: vec![McpServerConfig {
                id: "fs".into(),
                command,
                args,
                ..Default::default()
            }],
        },
        ..Default::default()
    };
    let Some((_dir, cmd_tx, mut evt_rx, _handle)) = spawn_orch_live_cfg(config) else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };

    // Ждём готовности сервера: снимок Settings с непустым MCP-каталогом
    // (npx качает пакет при первом запуске — таймаут щедрый).
    let settings = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::Settings { mcp_tools, .. } if !mcp_tools.is_empty()),
        ),
    )
    .await
    .expect("MCP-сервер не поднялся за 120с")
    .unwrap();
    let AppEvent::Settings { mcp_tools, .. } = settings else {
        unreachable!()
    };
    eprintln!("MCP-инструментов в каталоге: {}", mcp_tools.len());
    assert!(mcp_tools.iter().all(|t| t.id.starts_with("mcp__fs__")));

    // Профиль включает MCP-инструменты (двойной opt-in: мастер-гейт уже вкл).
    let pl = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ProfileList(_)))
        .await
        .unwrap();
    let pid = match pl {
        AppEvent::ProfileList(v) => v[0].id,
        _ => unreachable!(),
    };
    let mut enabled = default_tool_ids();
    enabled.extend(mcp_tools.iter().map(|t| t.id.clone()));
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: pid,
            edit: Box::new(ProfileEdit {
                enabled_tools: Some(enabled),
                ..Default::default()
            }),
        })
        .unwrap();

    // Ход: модель должна прочитать файл MCP-инструментом и назвать число.
    let (out, tools) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        &format!(
            "Прочитай файл {allowed}/secret_number.txt с помощью инструмента и скажи, \
             какое секретное число в нём записано."
        ),
    )
    .await;
    eprintln!("вызовы: {tools:?}\nответ: {out}");
    assert!(
        tools.iter().any(|t| t.starts_with("mcp__fs__")),
        "модель не вызвала MCP-инструмент: {tools:?}"
    );
    assert!(
        out.contains("7319"),
        "ответ не использует результат MCP-инструмента: {out:?}"
    );
    cmd_tx.send(AppCommand::Quit).unwrap();
}

#[tokio::test]
async fn mcp_exited_removes_tools_from_registry() {
    let (_dir, mut orch) = bare_orch();
    orch.config.mcp = mcp_config();
    orch.apply_mcp_settings();
    orch.handle_mcp_event(ready_event(&orch));
    assert!(orch.registry.get("mcp__fs__read_text_file").is_some());

    // Крах процесса: соединение мертво — обёртка уходит из реестра немедленно.
    let epoch = orch.mcp.epoch();
    orch.handle_mcp_event(McpEvent::Exited {
        epoch,
        server: "fs".into(),
    });
    assert!(orch.registry.get("mcp__fs__read_text_file").is_none());
    assert!(orch.registry.get("note_save").is_some());
}

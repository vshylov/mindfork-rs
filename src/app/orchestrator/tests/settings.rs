//! Тесты оркестратора — настройки: снимок, апдейты, дебаунс рестарта. Часть модуля [`super`]
//! (фикстуры в mod.rs). См. docs/history/refactoring-god-objects.md, этап 3.

use super::*;

#[tokio::test]
async fn bootstrap_emits_settings_snapshot() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let ev = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
        .await
        .unwrap();
    if let AppEvent::Settings {
        config, profiles, ..
    } = ev
    {
        assert_eq!(config.schema_version, AppConfig::default().schema_version);
        assert_eq!(profiles.len(), 1, "дефолтный профиль в снимке");
    }
    drop(cmd_tx);
    handle.await.unwrap();
}

#[tokio::test]
async fn update_config_persists_and_reemits_settings() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let root = _d.path().to_path_buf();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
        .await
        .unwrap();

    let config = AppConfig {
        max_tool_rounds: 3,
        ..Default::default()
    };
    cmd_tx
        .send(AppCommand::UpdateConfig(Box::new(config)))
        .unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::Settings { config, .. } if config.max_tool_rounds == 3),
    )
    .await
    .unwrap();

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // Конфиг сохранён на диск.
    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    assert_eq!(reopened.json().load_config().unwrap().max_tool_rounds, 3);
}

#[tokio::test]
async fn update_profile_persists_edit() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let ev = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
        .await
        .unwrap();
    let id = match ev {
        AppEvent::Settings { profiles, .. } => profiles[0].id,
        _ => unreachable!(),
    };

    cmd_tx
        .send(AppCommand::UpdateProfile {
            id,
            edit: Box::new(ProfileEdit {
                system_message: Some("новое sys".into()),
                ..Default::default()
            }),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::Settings { profiles, .. }
            if profiles.iter().any(|p| p.default_system_message == "новое sys"))
    })
    .await
    .unwrap();

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

/// Смена модели перезапускает chat-сервер (spec §11.6 DoD), но с дебаунсом:
/// серия быстрых правок полей движка коалесится в **один** рестарт после паузы
/// тишины (`RestartQueue`), а конфиг сохраняется/переэмитится сразу.
/// `start_paused` — виртуальное время tokio: дедлайн дебаунса доматывается
/// мгновенно и детерминированно, когда обе правки уже обработаны.
#[tokio::test(start_paused = true)]
async fn model_change_restarts_chat_server_debounced() {
    let backend = Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
        FinishReason::Stop,
    )])) as Arc<dyn EngineBackend>;
    let sup = Arc::new(MockSupervisor::with_backend(Some(backend)));
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, mut evt_rx) = unbounded_channel();
    let handle = tokio::spawn(run(OrchestratorDeps {
        cmd_rx,
        evt_tx,
        storage,
        config: AppConfig::default(),
        supervisor: sup.clone(),
        default_language: crate::shared::i18n::Lang::default(),
    }));
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
        .await
        .unwrap();
    // Бутстрап поднял сервер один раз (стартовый путь — немедленный, без дебаунса).
    assert_eq!(sup.chat_call_count(), 1);

    // Две правки движка подряд (смена модели, затем -ngl) — как серия коммитов
    // полей на экране настроек. Обе уходят до истечения дебаунса.
    let engine1 = crate::shared::config::EngineSettings {
        managed: crate::shared::config::ManagedSettings {
            model_path: Some("other.gguf".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut engine2 = engine1.clone();
    engine2.managed.gpu_layers = 10;
    cmd_tx
        .send(AppCommand::UpdateConfig(Box::new(AppConfig {
            engine: engine1,
            ..Default::default()
        })))
        .unwrap();
    cmd_tx
        .send(AppCommand::UpdateConfig(Box::new(AppConfig {
            engine: engine2,
            ..Default::default()
        })))
        .unwrap();
    // Конфиг переэмичен сразу (обе правки), рестарта ещё не было.
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::Settings { config, .. } if config.engine.managed.gpu_layers == 10),
    )
    .await
    .unwrap();
    assert_eq!(
        sup.chat_call_count(),
        1,
        "рестарт отложен дебаунсом, конфиг применён сразу"
    );

    // По истечении паузы тишины — ровно один рестарт с итоговыми значениями
    // (флаш эмитит снимок статусов — ждём его как маркер).
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ServerStatus(_)))
        .await
        .unwrap();
    assert_eq!(
        sup.chat_call_count(),
        2,
        "две правки движка → один отложенный перезапуск сервера"
    );

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

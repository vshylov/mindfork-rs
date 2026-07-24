//! Orchestrator tests — settings: the snapshot, updates, the restart debounce. Part of the [`super`]
//! module (fixtures in mod.rs). See docs/history/refactoring-god-objects.md, stage 3.

use super::*;

use crate::shared::config::CloudProvider;

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
        assert_eq!(profiles.len(), 1, "the default profile is in the snapshot");
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

    // The config is saved to disk.
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

/// A model change restarts the chat server (spec §11.6 DoD), but with a debounce:
/// a series of quick engine-field edits coalesces into **one** restart after a silence
/// pause (`RestartQueue`), while the config is saved/re-emitted right away.
/// `start_paused` — tokio's virtual time: the debounce deadline is fast-forwarded
/// instantly and deterministically once both edits have already been processed.
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
    // Bootstrap brought the server up once (the startup path is immediate, no debounce).
    assert_eq!(sup.chat_call_count(), 1);

    // Two engine edits in a row (a model change, then -ngl) — like a series of field
    // commits on the settings screen. Both go out before the debounce expires.
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
    // The config is re-emitted right away (both edits), no restart yet.
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::Settings { config, .. } if config.engine.managed.gpu_layers == 10),
    )
    .await
    .unwrap();
    assert_eq!(
        sup.chat_call_count(),
        1,
        "the restart is deferred by the debounce, the config is applied right away"
    );

    // After the silence pause expires — exactly one restart with the final values
    // (the flush emits a status snapshot — wait for it as a marker).
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ServerStatus(_)))
        .await
        .unwrap();
    assert_eq!(
        sup.chat_call_count(),
        2,
        "two engine edits → one deferred server restart"
    );

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

/// A key entered in settings is saved **encrypted**: `settings.json`
/// has no plaintext, but the application reads it back (this machine's entry).
/// The feature's main safety invariant — see docs/research/api-key-storage.md.
#[tokio::test]
async fn set_api_key_persists_encrypted_and_reads_back() {
    if !crate::shared::secrets::scheme_available() {
        return; // non-systemd Linux without machine-id: saving keys isn't supported
    }
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let root = _d.path().to_path_buf();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
        .await
        .unwrap();

    cmd_tx
        .send(AppCommand::SetApiKey {
            provider: CloudProvider::OpenAi,
            key: "sk-super-secret-42".into(),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::Settings { api_keys_present, .. } if !api_keys_present.is_empty())
    })
    .await
    .unwrap();

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // On disk — ciphertext, not the secret.
    let raw = std::fs::read_to_string(root.join("settings.json")).unwrap();
    assert!(
        !raw.contains("sk-super-secret-42"),
        "the key's plaintext leaked into settings.json"
    );
    assert!(raw.contains("api_keys"), "the key entry wasn't saved");
    // The application reads the key back (this same machine).
    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let cfg = reopened.json().load_config().unwrap();
    assert_eq!(
        crate::shared::secrets::stored_key(&cfg.api_keys, CloudProvider::OpenAi.key()).as_deref(),
        Some("sk-super-secret-42")
    );
}

/// Editing any setting doesn't erase saved keys: the config snapshot from the UI doesn't
/// carry them, and the orchestrator restores its own value (like `last_active_chat`).
#[tokio::test]
async fn update_config_preserves_stored_api_keys() {
    if !crate::shared::secrets::scheme_available() {
        return;
    }
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let root = _d.path().to_path_buf();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
        .await
        .unwrap();

    cmd_tx
        .send(AppCommand::SetApiKey {
            provider: CloudProvider::Claude,
            key: "sk-ant-keep-me".into(),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::Settings { api_keys_present, .. } if !api_keys_present.is_empty())
    })
    .await
    .unwrap();

    // A snapshot from the UI (carries no keys at all) — like a commit of any settings field.
    cmd_tx
        .send(AppCommand::UpdateConfig(Box::new(AppConfig {
            max_tool_rounds: 5,
            ..Default::default()
        })))
        .unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::Settings { config, .. } if config.max_tool_rounds == 5),
    )
    .await
    .unwrap();

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let cfg = reopened.json().load_config().unwrap();
    assert_eq!(
        crate::shared::secrets::stored_key(&cfg.api_keys, CloudProvider::Claude.key()).as_deref(),
        Some("sk-ant-keep-me"),
        "editing settings erased the saved key"
    );
}

/// An empty key removes the saved value (in the UI — clearing the field).
#[tokio::test]
async fn set_empty_api_key_removes_stored_entry() {
    if !crate::shared::secrets::scheme_available() {
        return;
    }
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
        .await
        .unwrap();
    cmd_tx
        .send(AppCommand::SetApiKey {
            provider: CloudProvider::Gemini,
            key: "sk-temp".into(),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::Settings { api_keys_present, .. } if !api_keys_present.is_empty())
    })
    .await
    .unwrap();
    // An empty key means removal: the "configured" flag goes dark.
    cmd_tx
        .send(AppCommand::SetApiKey {
            provider: CloudProvider::Gemini,
            key: String::new(),
        })
        .unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::Settings { api_keys_present, .. } if api_keys_present.is_empty()),
    )
    .await
    .unwrap();
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

/// The settings snapshot for the UI carries no keys (not even as ciphertext) — only flags
/// "configured on this machine". See docs/research/api-key-storage.md §5.
#[tokio::test]
async fn settings_snapshot_carries_flags_not_secrets() {
    if !crate::shared::secrets::scheme_available() {
        return;
    }
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
        .await
        .unwrap();

    cmd_tx
        .send(AppCommand::SetApiKey {
            provider: CloudProvider::OpenAi,
            key: "sk-in-snapshot-test".into(),
        })
        .unwrap();
    let ev = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::Settings { api_keys_present, .. } if !api_keys_present.is_empty())
    })
    .await
    .unwrap();

    if let AppEvent::Settings {
        config,
        api_keys_present,
        ..
    } = ev
    {
        assert_eq!(api_keys_present, vec![CloudProvider::OpenAi]);
        assert!(
            config.api_keys.is_empty(),
            "the UI snapshot must not carry key entries"
        );
    }
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

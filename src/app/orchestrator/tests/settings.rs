//! Orchestrator tests — settings: the snapshot, updates, the restart debounce. Part of the [`super`]
//! module (fixtures in mod.rs). See docs/history/refactoring-god-objects.md, stage 3.

use super::*;

use crate::app::orchestrator::engines::{RESTART_BUDGET, Server};
use crate::shared::config::{CloudProvider, ServerMode};
use crate::shared::secrets::{ExternalSlot, SecretKey};
use crate::shared::server::ServerStatus;

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

/// An edit and its undo cost **no** restart: the debounce flag only says "something
/// was edited", and the decision is taken against what the server is actually running.
/// Without this, `Ctrl+Z` on an engine field would kill and reload a GGUF to arrive at
/// the values already loaded. See docs/history/settings-undo.md §5.1.
///
/// Proving a restart *didn't* happen can't rely on waiting for an absent event, so the
/// test ends with a genuine change and checks the counter against **its** status
/// event: if the reverted pair had restarted anything, the final count would be one
/// higher.
#[tokio::test(start_paused = true)]
async fn an_edit_and_its_undo_cost_no_restart() {
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
    assert_eq!(sup.chat_call_count(), 1, "bootstrap raised it once");

    // Edit an engine field, then put it back — exactly what `Ctrl+Z` sends.
    let edited = crate::shared::config::EngineSettings {
        managed: crate::shared::config::ManagedSettings {
            model_path: Some("other.gguf".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    cmd_tx
        .send(AppCommand::UpdateConfig(Box::new(AppConfig {
            engine: edited,
            ..Default::default()
        })))
        .unwrap();
    cmd_tx
        .send(AppCommand::UpdateConfig(Box::default()))
        .unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::Settings { config, .. } if config.engine == AppConfig::default().engine),
    )
    .await
    .unwrap();

    // Let the debounce deadline pass. Under `start_paused` tokio advances virtual time
    // to the orchestrator's timer first, so its flush has run by the time this returns.
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    assert_eq!(
        sup.chat_call_count(),
        1,
        "the config came back to what the server is already running — nothing to do"
    );

    // A genuine change still restarts — and its status event is the deterministic
    // marker that the flush above really ran.
    let changed = crate::shared::config::EngineSettings {
        managed: crate::shared::config::ManagedSettings {
            gpu_layers: 10,
            ..Default::default()
        },
        ..Default::default()
    };
    cmd_tx
        .send(AppCommand::UpdateConfig(Box::new(AppConfig {
            engine: changed,
            ..Default::default()
        })))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ServerStatus(_)))
        .await
        .unwrap();
    assert_eq!(
        sup.chat_call_count(),
        2,
        "exactly one restart, for the change that actually differed"
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
        .send(AppCommand::SetSecret {
            key: SecretKey::Provider(CloudProvider::OpenAi),
            value: "sk-super-secret-42".into(),
        })
        .unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::Settings { secrets_present, .. } if !secrets_present.is_empty()),
    )
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

/// An external server's key takes the same path as a provider's, but is addressed by
/// **slot**: entering it re-raises that one server, and the key the orchestrator hands
/// the supervisor is the external one — not a provider's, and not nothing. Both halves
/// matter: before docs/history/external-api-key.md the external mode resolved no stored key at
/// all, so a mode reading the wrong slot would look exactly like the old behaviour.
#[tokio::test(start_paused = true)]
async fn set_external_key_reraises_that_server_with_the_key() {
    if !crate::shared::secrets::scheme_available() {
        return; // non-systemd Linux without machine-id: saving keys isn't supported
    }
    let config = AppConfig {
        engine: crate::shared::config::EngineSettings {
            mode: ServerMode::External,
            external: crate::shared::config::ExternalSettings {
                url: Some("http://127.0.0.1:9/v1".into()),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    let (_d, sup, cmd_tx, mut evt_rx, handle) = spawn_orch_sup(config);
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
        .await
        .unwrap();
    assert_eq!(sup.chat_call_count(), 1, "bootstrap raised it once");
    assert_eq!(
        sup.chat_keys(),
        vec![None],
        "nothing is stored yet, so the supervisor falls back to env"
    );

    cmd_tx
        .send(AppCommand::SetSecret {
            key: SecretKey::External(ExternalSlot::Chat),
            value: "sk-gateway-1".into(),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::Settings { secrets_present, .. }
            if secrets_present.contains(&SecretKey::External(ExternalSlot::Chat)))
    })
    .await
    .unwrap();
    // The restart is deferred like an engine edit — wait for the flush's status snapshot.
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ServerStatus(_)))
        .await
        .unwrap();
    assert_eq!(
        sup.chat_keys(),
        vec![None, Some("sk-gateway-1".into())],
        "the chat server came back up with the external slot's key"
    );

    // A *different* slot's key leaves this server alone: the mark is per slot, not
    // "some secret changed". Proving a restart *didn't* happen can't wait on an absent
    // event, so the speech key is followed by a slot that **does** restart something
    // (embeddings) and the count is read against *its* flush — if the speech key had
    // marked chat, that same flush would have raised the chat server too.
    for slot in [ExternalSlot::Tts, ExternalSlot::Embed] {
        cmd_tx
            .send(AppCommand::SetSecret {
                key: SecretKey::External(slot),
                value: format!("sk-{slot:?}-2"),
            })
            .unwrap();
        wait_for(&mut evt_rx, |e| {
            matches!(e, AppEvent::Settings { secrets_present, .. }
                if secrets_present.contains(&SecretKey::External(slot)))
        })
        .await
        .unwrap();
    }
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ServerStatus(_)))
        .await
        .unwrap();
    assert_eq!(
        sup.chat_call_count(),
        2,
        "neither the speech nor the embedding key may restart the chat server"
    );

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

/// The backup password takes the same path as an API key: ciphertext on disk, a
/// presence flag to the UI, and readable back on this machine — which is what
/// makes `mindfork backup` pick it up with no argument. Spec §12.3.
#[tokio::test]
async fn set_backup_password_persists_encrypted_and_reads_back() {
    if !crate::shared::secrets::scheme_available() {
        return; // non-systemd Linux without machine-id
    }
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let root = _d.path().to_path_buf();
    wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::Settings { secrets_present, .. } if !secrets_present.contains(&SecretKey::BackupPassword))
    })
    .await
    .unwrap();

    cmd_tx
        .send(AppCommand::SetSecret {
            key: SecretKey::BackupPassword,
            value: "open-sesame-42".into(),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::Settings { secrets_present, .. } if secrets_present.contains(&SecretKey::BackupPassword))
    })
    .await
    .unwrap();

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let raw = std::fs::read_to_string(root.join("settings.json")).unwrap();
    assert!(
        !raw.contains("open-sesame-42"),
        "the backup password leaked into settings.json in the clear"
    );
    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let cfg = reopened.json().load_config().unwrap();
    assert_eq!(
        crate::shared::secrets::stored_key(
            &cfg.api_keys,
            crate::shared::secrets::BACKUP_PASSWORD_KEY
        )
        .as_deref(),
        Some("open-sesame-42")
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
        .send(AppCommand::SetSecret {
            key: SecretKey::Provider(CloudProvider::Claude),
            value: "sk-ant-keep-me".into(),
        })
        .unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::Settings { secrets_present, .. } if !secrets_present.is_empty()),
    )
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
        .send(AppCommand::SetSecret {
            key: SecretKey::Provider(CloudProvider::Gemini),
            value: "sk-temp".into(),
        })
        .unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::Settings { secrets_present, .. } if !secrets_present.is_empty()),
    )
    .await
    .unwrap();
    // An empty key means removal: the "configured" flag goes dark.
    cmd_tx
        .send(AppCommand::SetSecret {
            key: SecretKey::Provider(CloudProvider::Gemini),
            value: String::new(),
        })
        .unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::Settings { secrets_present, .. } if secrets_present.is_empty()),
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
        .send(AppCommand::SetSecret {
            key: SecretKey::Provider(CloudProvider::OpenAi),
            value: "sk-in-snapshot-test".into(),
        })
        .unwrap();
    let ev = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::Settings { secrets_present, .. } if !secrets_present.is_empty()),
    )
    .await
    .unwrap();

    if let AppEvent::Settings {
        config,
        secrets_present,
        ..
    } = ev
    {
        assert_eq!(
            secrets_present,
            vec![SecretKey::Provider(CloudProvider::OpenAi)]
        );
        assert!(
            config.api_keys.is_empty(),
            "the UI snapshot must not carry key entries"
        );
    }
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

/// A managed server whose process is gone can only be revived by launching a new
/// one — so the orchestrator relaunches it, but a bounded number of times: a server
/// that dies *because* of its configuration must not be respawned forever.
#[tokio::test]
async fn dead_managed_server_is_relaunched_until_the_budget_runs_out() {
    let (_d, mut orch) = bare_orch();
    orch.config.engine.mode = ServerMode::Managed;

    for round in 0..RESTART_BUDGET {
        orch.engines
            .set_chat_status(ServerStatus::Disconnected("process gone".into()));
        orch.relaunch_dead_managed_servers();
        assert!(
            !matches!(
                orch.engines.status_of(Server::Chat),
                ServerStatus::Disconnected(_)
            ),
            "round {round}: a dead managed server should have been relaunched"
        );
    }

    // The budget is spent — the next death is left alone instead of crash-looping.
    orch.engines
        .set_chat_status(ServerStatus::Disconnected("process gone".into()));
    orch.relaunch_dead_managed_servers();
    assert!(
        matches!(
            orch.engines.status_of(Server::Chat),
            ServerStatus::Disconnected(_)
        ),
        "the crash-loop guard should stop relaunching"
    );
}

/// We don't own an external process, so there's nothing to relaunch — its monitor
/// keeps polling and picks the recovery up by itself.
#[tokio::test]
async fn external_server_is_never_relaunched() {
    let (_d, mut orch) = bare_orch();
    orch.config.engine.mode = ServerMode::External;
    orch.engines
        .set_chat_status(ServerStatus::Disconnected("host down".into()));
    orch.relaunch_dead_managed_servers();
    assert!(
        matches!(
            orch.engines.status_of(Server::Chat),
            ServerStatus::Disconnected(_)
        ),
        "an external server must not be relaunched by us"
    );
}

/// A key typed into settings **wins** over the environment variable the settings
/// name — the precedence `api_key_env` already has everywhere else. Without it a
/// stale shell would silently shadow the key someone just entered, and the
/// resulting 401 would look like the app's fault.
#[test]
fn a_stored_search_key_beats_the_named_environment_variable() {
    use crate::shared::secrets::SearchSlot;

    // A variable name this test owns, so it cannot collide with a real shell.
    const VAR: &str = "MINDFORK_TEST_TAVILY_KEY_ENV";
    // SAFETY: single-threaded test, and the variable is this test's own name.
    unsafe { std::env::set_var(VAR, "from-the-environment") };

    let mut cfg = crate::shared::config::AppConfig::default();
    cfg.tools.web_tavily_key_env = Some(VAR.into());
    cfg.tools.web_brave_key_env = None;

    // Only the environment is set: it is used.
    assert_eq!(
        super::super::web_search_keys(&cfg),
        vec![(SearchSlot::Tavily, "from-the-environment".to_string())]
    );

    // Now store one in settings — it must win.
    crate::shared::secrets::put_key(
        &mut cfg.api_keys,
        &crate::shared::secrets::SecretKey::Search(SearchSlot::Tavily).storage_name(),
        "from-settings",
        || "test".to_string(),
    )
    .expect("the machine key scheme must be available in tests");
    assert_eq!(
        super::super::web_search_keys(&cfg),
        vec![(SearchSlot::Tavily, "from-settings".to_string())]
    );

    // SAFETY: as above.
    unsafe { std::env::remove_var(VAR) };
}

/// A provider with neither a stored key nor a variable simply is not in the
/// list — `web_search` must never be handed an empty credential to fail on.
#[test]
fn an_unconfigured_search_provider_yields_no_key() {
    let mut cfg = crate::shared::config::AppConfig::default();
    cfg.tools.web_tavily_key_env = None;
    cfg.tools.web_brave_key_env = Some("   ".into());
    assert!(super::super::web_search_keys(&cfg).is_empty());
}

/// A stored search key must appear in the presence list, or the settings row
/// keeps reading "not set" over a key that is on disk and in use — and someone
/// enters it a second time, or concludes the provider is broken. Clippy found
/// this one as dead code (`SearchSlot::ALL` unused); nothing was pinning it.
#[tokio::test]
async fn a_stored_search_key_shows_as_present() {
    use crate::shared::secrets::SearchSlot;

    if !crate::shared::secrets::scheme_available() {
        return; // non-systemd Linux without machine-id: saving keys isn't supported
    }
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
        .await
        .unwrap();

    for slot in SearchSlot::ALL {
        cmd_tx
            .send(AppCommand::SetSecret {
                key: SecretKey::Search(slot),
                value: format!("key-for-{slot:?}"),
            })
            .unwrap();
        wait_for(&mut evt_rx, |e| {
            matches!(e, AppEvent::Settings { secrets_present, .. }
                if secrets_present.contains(&SecretKey::Search(slot)))
        })
        .await
        .unwrap_or_else(|| panic!("{slot:?} never showed as present"));
    }

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

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
        extra_tools: Vec::new(),
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
        extra_tools: Vec::new(),
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
    assert!(super::super::web_search_keys(&cfg).is_empty());
    // A whitespace-only variable name is not a name either.
    cfg.tools.web_tavily_key_env = Some("   ".into());
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

/// What one engine said about itself does not outlive it.
///
/// Until this was fixed the facts were asked again at start-up and on a status
/// flip only. A managed or external server announces itself through its probe,
/// so those switches were covered; a cloud is ready at once and says nothing —
/// and the previous engine's window went on deciding when automatic compaction
/// fired, against a model it had never been measured for
/// (docs/research/openrouter-mode.md §4.1, §9 A2). The gateway's own mode would
/// have met this on every change of model.
#[tokio::test]
async fn what_the_previous_engine_said_does_not_outlive_a_settings_edit() {
    use crate::app::orchestrator::compaction::EngineFacts;
    use crate::shared::api::contract::ModelCapabilities;

    let (_d, mut orch) = bare_orch();
    orch.config.engine.mode = ServerMode::External;
    // As a gateway's catalogue answered for the model `external` pointed at.
    let epoch = orch.context.epoch();
    orch.handle_budget_result(
        epoch,
        EngineFacts {
            budget: None,
            caps: Some(ModelCapabilities {
                context_length: Some(64_000),
                sampling_fields: Some(vec!["temperature".to_string()].into()),
            }),
        },
    );
    assert_eq!(orch.context_budget(), Some(64_000));
    assert!(orch.endpoint_catalogued());

    // The switch that flips no status.
    orch.config.engine.mode = ServerMode::Claude;
    orch.restarts.mark_chat();
    orch.flush_restarts();

    assert_eq!(
        orch.context_budget(),
        None,
        "the window belonged to the engine that is gone"
    );
    assert!(!orch.endpoint_catalogued());
    assert!(orch.endpoint_sampling_fields().is_none());
    assert!(
        orch.context.epoch() > epoch,
        "and an answer still in flight for it would be dropped"
    );
}

/// An embedder that keeps the texts it was asked to embed, as they arrived.
struct Recording {
    asked: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl Embedder for Recording {
    async fn embed(
        &self,
        texts: Vec<String>,
        _role: crate::shared::api::EmbedRole,
    ) -> anyhow::Result<Vec<Vec<f32>>> {
        let vectors = texts.iter().map(|t| vec![t.len() as f32, 1.0]).collect();
        self.asked.lock().unwrap().extend(texts);
        Ok(vectors)
    }
}

/// An orchestrator whose supervisor hands out `embedder`, bare, every time an
/// embedding engine is applied — what the production supervisor does.
fn orch_on(embedder: Arc<Recording>) -> (tempfile::TempDir, Orchestrator) {
    let (dir, mut orch) = bare_orch();
    orch.engines = EngineManager::new(
        Arc::new(MockSupervisor::with_backend_and_embedder(
            None,
            Some(embedder as Arc<dyn Embedder>),
        )),
        unbounded_channel().0,
        unbounded_channel().0,
        unbounded_channel().0,
    );
    orch.config.embed.convention = crate::shared::embed_prefix::EmbedConvention::E5;
    (dir, orch)
}

/// Embeds one passage through whatever the orchestrator would hand a tool now,
/// and says what reached the embedder for it: every text asked since the last
/// call, the guard's own among them.
async fn embedded(orch: &Orchestrator, seen: &Recording, text: &str) -> Vec<String> {
    orch.engines
        .embedder()
        .embed(
            vec![text.to_string()],
            crate::shared::api::EmbedRole::Passage,
        )
        .await
        .expect("an embedding");
    std::mem::take(&mut *seen.asked.lock().unwrap())
}

/// The canary, as the model-change guard asks for it under the e5 convention.
fn canary() -> String {
    format!("passage: {}", crate::shared::embed_identity::CANARY_TEXT)
}

/// An embedder is dressed by **every** road that installs one: its input
/// convention applied, and the model-change guard armed.
///
/// Start-up dressed it; a settings edit and a relaunch installed what the
/// supervisor built as it was — so from the first change of the embedding
/// settings to the restart there were no `query:`/`passage:` markers, and
/// nothing checked that the stored vectors belong to the model now answering.
/// A switch of the embedder is the moment that check exists for
/// (docs/research/openrouter-mode.md §9, A1).
#[tokio::test]
async fn an_embedder_is_dressed_by_every_road_that_installs_one() {
    let seen = Arc::new(Recording {
        asked: Default::default(),
    });
    let (_d, mut orch) = orch_on(seen.clone());

    // Start-up.
    orch.apply_embed_settings();
    let asked = embedded(&orch, &seen, "first").await;
    assert_eq!(asked.last().map(String::as_str), Some("passage: first"));
    assert!(asked.contains(&canary()), "the guard asked: {asked:?}");
    // Once per embedder: the guard does not ask again for the next text.
    assert_eq!(embedded(&orch, &seen, "second").await, ["passage: second"]);

    // A settings edit, as the settings screen makes it.
    let mut edited = orch.config.clone();
    edited.embed.external.model_name = Some("another-model".into());
    orch.handle_update_config(edited);
    orch.flush_restarts();
    let asked = embedded(&orch, &seen, "third").await;
    assert_eq!(
        asked.last().map(String::as_str),
        Some("passage: third"),
        "the convention survives the edit"
    );
    assert!(
        asked.contains(&canary()),
        "and the guard is armed for the embedder the edit installed: {asked:?}"
    );

    // A convention changed in the session is the one applied.
    let mut edited = orch.config.clone();
    edited.embed.convention = crate::shared::embed_prefix::EmbedConvention::None;
    orch.handle_update_config(edited);
    orch.flush_restarts();
    let asked = embedded(&orch, &seen, "fourth").await;
    assert_eq!(asked.last().map(String::as_str), Some("fourth"));
    assert!(
        asked.contains(&crate::shared::embed_identity::CANARY_TEXT.to_string()),
        "{asked:?}"
    );
}

/// The third road: a managed embedding server that died is relaunched, and
/// what the relaunch installs is dressed as well.
#[tokio::test]
async fn a_relaunched_embedder_is_dressed() {
    let seen = Arc::new(Recording {
        asked: Default::default(),
    });
    let (_d, mut orch) = orch_on(seen.clone());
    orch.config.embed.mode = ServerMode::Managed;
    orch.apply_embed_settings();
    embedded(&orch, &seen, "before").await;

    orch.engines
        .set_embed_status(ServerStatus::Disconnected("the process exited".into()));
    orch.relaunch_dead_managed_servers();
    let asked = embedded(&orch, &seen, "after").await;
    assert_eq!(asked.last().map(String::as_str), Some("passage: after"));
    assert!(asked.contains(&canary()), "{asked:?}");
}

/// The gateway's switch is the provider's: it reaches **every** slot that
/// speaks to the gateway — impersonation and the embedder as well as the chat —
/// and what each was applied with is compared, so the slot is raised again with
/// the switch as it now stands.
#[tokio::test]
async fn the_gateways_switch_reaches_impersonation_and_the_embedder() {
    use crate::shared::config::ImpersonationMode;

    let (_d, mut orch) = bare_orch();
    let mut config = orch.config.clone();
    config.impersonation_engine.mode = ImpersonationMode::OpenRouter;
    config.embed.mode = ServerMode::OpenRouter;
    orch.handle_update_config(config.clone());
    orch.flush_restarts();
    let current = |orch: &Orchestrator, config: &AppConfig| {
        (
            orch.engines.impersonation_is_current(
                &config.impersonation_engine,
                &config.api_keys,
                &config.openrouter,
            ),
            orch.engines
                .embed_is_current(&config.embed, &config.api_keys, &config.openrouter),
        )
    };
    assert_eq!(current(&orch, &config), (true, true));

    // The switch, and nothing else.
    config.openrouter.attribution = false;
    orch.handle_update_config(config.clone());
    assert_eq!(
        current(&orch, &config),
        (false, false),
        "what they were applied with is not what the settings say now"
    );
    assert_eq!(
        orch.restarts.take(),
        (false, true, true, false),
        "the embedder and impersonation, and not the chat: its mode is not the gateway's"
    );
    orch.restarts.mark_embed();
    orch.restarts.mark_impersonation();
    orch.flush_restarts();
    assert_eq!(current(&orch, &config), (true, true));

    // A slot that is not the gateway's is current whatever the switch says.
    let mut local = config.clone();
    local.impersonation_engine.mode = ImpersonationMode::Shared;
    local.embed.mode = ServerMode::Managed;
    orch.handle_update_config(local.clone());
    orch.flush_restarts();
    local.openrouter.attribution = true;
    orch.handle_update_config(local.clone());
    assert_eq!(orch.restarts.take(), (false, false, false, false));
    assert_eq!(current(&orch, &local), (true, true));
}

/// The model list's request follows the gateway's switch, on every tab that
/// speaks to the gateway — and no other provider's request is ever named,
/// whatever the switch says.
#[test]
fn the_model_list_names_the_app_where_the_gateways_switch_says_so() {
    use crate::shared::api::catalogue::ModelSlot;
    use crate::shared::config::{CloudSettings, EmbedSettings, ImpersonationMode};

    let (_d, mut orch) = bare_orch();
    orch.config.engine.mode = ServerMode::OpenRouter;
    orch.config.impersonation_engine.mode = ImpersonationMode::OpenRouter;
    orch.config.embed = EmbedSettings {
        mode: ServerMode::OpenRouter,
        ..Default::default()
    };
    let slots = [
        ModelSlot::Assistant,
        ModelSlot::Impersonation,
        ModelSlot::Embedder,
    ];
    let named = |orch: &Orchestrator| {
        slots.map(|slot| orch.catalogue_request(slot).expect("a request").attribution)
    };
    assert_eq!(named(&orch), [true; 3], "on until somebody turns it off");
    orch.config.openrouter.attribution = false;
    assert_eq!(named(&orch), [false; 3]);

    // Another cloud, the switch on: the headers are the gateway's.
    orch.config.openrouter.attribution = true;
    orch.config.engine.mode = ServerMode::Grok;
    orch.config.engine.grok = CloudSettings {
        api_key_env: Some("PATH".into()),
        ..Default::default()
    };
    let grok = orch
        .catalogue_request(ModelSlot::Assistant)
        .expect("a request");
    assert!(!grok.attribution);
}

/// The speech slot's list is asked of the gateway in the gateway's mode — at
/// the address and with the key variable of the slot's own section, named or
/// not as the provider's switch says — and of nobody in any other mode: no
/// other speech provider publishes a list of what speaks.
#[test]
fn the_speech_slots_list_is_the_gateways_and_follows_the_slots_own_section() {
    use crate::shared::api::catalogue::{CatalogueError, CatalogueShape, ModelSlot};
    use crate::shared::config::TtsMode;

    let (_d, mut orch) = bare_orch();
    // The chat slot is somewhere else entirely: speech has a mode of its own.
    orch.config.engine.mode = ServerMode::Managed;
    for mode in TtsMode::ALL {
        orch.config.tts.mode = mode;
        let asked = orch.catalogue_request(ModelSlot::Speech);
        if mode != TtsMode::OpenRouter {
            assert_eq!(asked, Err(CatalogueError::NotConfigured), "{mode:?}");
            continue;
        }
        let asked = asked.expect("a request");
        assert_eq!(asked.shape, CatalogueShape::OpenRouterSpeech);
        assert_eq!(asked.base, "https://openrouter.ai/api/v1");
        assert_eq!(asked.key, None, "the list is public: asked without a key");
        assert!(asked.attribution, "on until somebody turns it off");
    }

    orch.config.tts.mode = TtsMode::OpenRouter;
    orch.config.openrouter.attribution = false;
    orch.config.tts.openrouter.url = Some(" https://eu.openrouter.ai/api/v1 ".into());
    // A variable that is certainly set, read and never sent: nothing is asked
    // here, the request is only built.
    orch.config.tts.openrouter.api_key_env = Some("PATH".into());
    // The chat slot's section names another address; it is not the speech
    // slot's.
    orch.config.engine.openrouter.url = Some("https://chat.example/v1".into());
    let asked = orch
        .catalogue_request(ModelSlot::Speech)
        .expect("a request");
    assert_eq!(asked.base, "https://eu.openrouter.ai/api/v1");
    assert!(asked.key.is_some(), "the variable the speech section names");
    assert!(!asked.attribution);
}

/// An answer of the engine that is gone says nothing to the screens.
///
/// The memo dropped it — and the list of sampling fields it carried was sent to
/// the UI before the epoch was looked at. With the facts asked again on every
/// applied change, two answers race whenever a mode is changed twice in a row:
/// the slower engine answers last, and its list would stay on the settings
/// screen of the other.
#[tokio::test]
async fn an_answer_of_the_engine_that_is_gone_tells_the_screens_nothing() {
    use crate::app::orchestrator::compaction::EngineFacts;
    use crate::shared::api::contract::ModelCapabilities;

    let answer = || EngineFacts {
        budget: None,
        caps: Some(ModelCapabilities {
            context_length: Some(64_000),
            sampling_fields: Some(vec!["temperature".to_string()].into()),
        }),
    };
    let told = |rx: &mut UnboundedReceiver<AppEvent>| {
        let mut lists = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let AppEvent::EngineSamplingFields(fields) = event {
                lists.push(fields);
            }
        }
        lists
    };
    let (_d, mut orch, mut rx) = bare_orch_rx();
    // A mode whose window is the engine's to say, not the settings'.
    orch.config.engine.mode = ServerMode::External;
    let gone = orch.context.epoch();
    orch.refresh_engine_facts();
    told(&mut rx);

    orch.handle_budget_result(gone, answer());
    assert_eq!(told(&mut rx), [], "the engine it describes is gone");
    assert_eq!(orch.context_budget(), None);
    assert!(orch.endpoint_sampling_fields().is_none());

    // The control: the same answer for the engine that is there.
    orch.handle_budget_result(orch.context.epoch(), answer());
    assert_eq!(
        told(&mut rx),
        [Some(vec!["temperature".to_string()].into())]
    );
    assert_eq!(orch.context_budget(), Some(64_000));
}

/// The status event of a slot raised again — waited for under a bound, so that
/// a restart that never came is a failure with these words rather than a test
/// that never ends (the clock is paused: the bound costs nothing).
async fn raised_again(rx: &mut UnboundedReceiver<AppEvent>) {
    tokio::time::timeout(
        std::time::Duration::from_secs(60),
        wait_for(rx, |e| matches!(e, AppEvent::ServerStatus(_))),
    )
    .await
    .expect("the slot is raised again")
    .expect("the event stream");
}

/// The gateway's own switch reaches the slots that speak to the gateway — and
/// no other: a managed server is a GGUF killed and loaded again, and must not
/// pay that for a header it never sends.
///
/// Proving a restart *didn't* happen can't rely on waiting for an absent event,
/// so the second half ends with a genuine change and reads the counter against
/// **its** status event.
#[tokio::test(start_paused = true)]
async fn the_gateways_own_switch_restarts_only_the_slots_that_speak_to_it() {
    let backend = Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
        FinishReason::Stop,
    )])) as Arc<dyn EngineBackend>;
    let sup = Arc::new(MockSupervisor::with_backend(Some(backend)));
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, mut evt_rx) = unbounded_channel();
    let through_the_gateway = |attribution: bool| AppConfig {
        engine: crate::shared::config::EngineSettings {
            mode: ServerMode::OpenRouter,
            ..Default::default()
        },
        openrouter: crate::shared::config::OpenRouterSettings { attribution },
        ..Default::default()
    };
    let handle = tokio::spawn(run(OrchestratorDeps {
        cmd_rx,
        evt_tx,
        storage,
        config: through_the_gateway(true),
        supervisor: sup.clone(),
        default_language: crate::shared::i18n::Lang::default(),
        extra_tools: Vec::new(),
    }));
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
        .await
        .unwrap();
    assert_eq!(sup.chat_call_count(), 1, "bootstrap raised it once");

    // The switch alone: the chat slot speaks to the gateway, so it is re-raised.
    cmd_tx
        .send(AppCommand::UpdateConfig(Box::new(through_the_gateway(
            false,
        ))))
        .unwrap();
    raised_again(&mut evt_rx).await;
    assert_eq!(
        sup.chat_call_count(),
        2,
        "the client has to be rebuilt without the headers"
    );

    // Off the gateway (a restart of its own), then the switch alone again.
    let managed = |attribution: bool, gpu_layers: i32| AppConfig {
        engine: crate::shared::config::EngineSettings {
            managed: crate::shared::config::ManagedSettings {
                gpu_layers,
                ..Default::default()
            },
            ..Default::default()
        },
        openrouter: crate::shared::config::OpenRouterSettings { attribution },
        ..Default::default()
    };
    cmd_tx
        .send(AppCommand::UpdateConfig(Box::new(managed(false, 99))))
        .unwrap();
    raised_again(&mut evt_rx).await;
    assert_eq!(sup.chat_call_count(), 3);

    cmd_tx
        .send(AppCommand::UpdateConfig(Box::new(managed(true, 99))))
        .unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::Settings { config, .. } if config.openrouter.attribution),
    )
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    assert_eq!(
        sup.chat_call_count(),
        3,
        "a managed server has nothing to do with the gateway's switch"
    );

    // The marker that the flush above really ran: a genuine change restarts once.
    cmd_tx
        .send(AppCommand::UpdateConfig(Box::new(managed(true, 10))))
        .unwrap();
    raised_again(&mut evt_rx).await;
    assert_eq!(sup.chat_call_count(), 4);

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

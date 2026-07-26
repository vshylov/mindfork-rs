//! Orchestrator tests — profiles: create/delete/cascade. Part of the [`super`]
//! module (fixtures in mod.rs). See docs/history/refactoring-god-objects.md, stage 3.

use super::*;

#[tokio::test]
async fn create_profile_appears_in_profile_list() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    // Bootstrap creates one default profile.
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ProfileList(p) if p.len() == 1),
    )
    .await
    .unwrap();

    cmd_tx
        .send(AppCommand::CreateProfile {
            name: "  Второй  ".into(),
            system_message: "sys".into(),
        })
        .unwrap();
    let list = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ProfileList(p) if p.len() == 2),
    )
    .await
    .unwrap();
    if let AppEvent::ProfileList(profiles) = list {
        assert!(profiles.iter().any(|p| p.name == "Второй")); // the name is normalized
    }

    drop(cmd_tx);
    handle.await.unwrap();
}

#[tokio::test]
async fn delete_profile_cascades_to_its_chats() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    // Create a second profile and learn its id.
    cmd_tx
        .send(AppCommand::CreateProfile {
            name: "Второй".into(),
            system_message: "sys".into(),
        })
        .unwrap();
    let list = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ProfileList(p) if p.len() == 2),
    )
    .await
    .unwrap();
    let second_id = match list {
        AppEvent::ProfileList(profiles) => profiles.iter().find(|p| p.name == "Второй").unwrap().id,
        _ => unreachable!(),
    };

    // Create a chat from the second profile → two chats in total.
    cmd_tx
        .send(AppCommand::NewChat {
            profile_id: Some(second_id),
        })
        .unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ChatList(c) if c.len() == 2),
    )
    .await
    .unwrap();

    // Delete the second profile: its chat cascades to hidden → one remains.
    cmd_tx.send(AppCommand::DeleteProfile(second_id)).unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ProfileList(p) if p.len() == 1),
    )
    .await
    .unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ChatList(c) if c.len() == 1),
    )
    .await
    .unwrap();

    drop(cmd_tx);
    handle.await.unwrap();
}

/// The settings screen keeps its own copy of the profile list, so creating/deleting a
/// profile must re-emit `Settings` — otherwise the new profile is invisible there (not
/// even selectable) and a deleted one lingers until a restart.
#[tokio::test]
async fn create_and_delete_profile_reemit_settings() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    cmd_tx
        .send(AppCommand::CreateProfile {
            name: "Второй".into(),
            system_message: "sys".into(),
        })
        .unwrap();
    let settings = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::Settings { profiles, .. } if profiles.len() == 2),
    )
    .await
    .unwrap();
    let second_id = match settings {
        AppEvent::Settings { profiles, .. } => {
            profiles.iter().find(|p| p.name == "Второй").unwrap().id
        }
        _ => unreachable!(),
    };

    cmd_tx.send(AppCommand::DeleteProfile(second_id)).unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::Settings { profiles, .. } if profiles.len() == 1),
    )
    .await
    .unwrap();

    drop(cmd_tx);
    handle.await.unwrap();
}

/// The legacy per-profile impersonation message becomes a named impersonation profile
/// on startup, and the assistant profile is linked to it (spec §11.8). Idempotent: a
/// second launch on the same data changes nothing.
#[tokio::test]
async fn legacy_impersonation_message_migrates_to_a_profile() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::with_root(dir.path());
    let storage = Storage::open(paths.clone()).unwrap();
    let mut profile = Profile::new("Джойс", "Ты — Джойс.");
    profile.impersonation_system_message = "Ты — Владимир.".into();
    let profile_id = profile.id;
    storage.json().upsert_profile(&profile).unwrap();
    drop(storage);

    // First launch: the migration runs.
    let (cmd_tx, handle) = spawn_orch_at(dir.path());
    drop(cmd_tx);
    handle.await.unwrap();

    let storage = Storage::open(paths.clone()).unwrap();
    let config = storage.json().load_config().unwrap();
    assert_eq!(config.impersonation_profiles.len(), 1);
    let imp = &config.impersonation_profiles[0];
    assert_eq!(imp.system_message, "Ты — Владимир.");
    assert!(
        imp.name.contains("Джойс"),
        "named after the profile: {}",
        imp.name
    );
    let saved = storage.json().load_profiles().unwrap();
    let saved = saved.iter().find(|p| p.id == profile_id).unwrap();
    assert_eq!(saved.impersonation_profile_id, Some(imp.id));
    // The legacy field is kept on disk (nothing reads it for prompts any more).
    assert_eq!(saved.impersonation_system_message, "Ты — Владимир.");
    drop(storage);

    // Second launch: nothing is duplicated.
    let (cmd_tx, handle) = spawn_orch_at(dir.path());
    drop(cmd_tx);
    handle.await.unwrap();
    let storage = Storage::open(paths).unwrap();
    assert_eq!(
        storage
            .json()
            .load_config()
            .unwrap()
            .impersonation_profiles
            .len(),
        1
    );
}

/// Runs the orchestrator on an existing data root (used to check startup migrations
/// across two launches); the events are discarded.
fn spawn_orch_at(
    root: &std::path::Path,
) -> (UnboundedSender<AppCommand>, tokio::task::JoinHandle<()>) {
    let storage = Arc::new(Storage::open(Paths::with_root(root)).unwrap());
    let config = storage.json().load_config().unwrap();
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, mut evt_rx) = unbounded_channel();
    tokio::spawn(async move { while evt_rx.recv().await.is_some() {} });
    let deps = OrchestratorDeps {
        cmd_rx,
        evt_tx,
        storage,
        config,
        supervisor: Arc::new(MockSupervisor::with_backend(None)),
        default_language: crate::shared::i18n::Lang::default(),
    };
    (cmd_tx, tokio::spawn(run(deps)))
}

#[tokio::test]
async fn cannot_delete_last_profile() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let list = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ProfileList(_)))
        .await
        .unwrap();
    let only_id = match list {
        AppEvent::ProfileList(p) => p[0].id,
        _ => unreachable!(),
    };

    cmd_tx.send(AppCommand::DeleteProfile(only_id)).unwrap();
    let err = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Error(_)))
        .await
        .unwrap();
    assert!(matches!(err, AppEvent::Error(_)));

    drop(cmd_tx);
    handle.await.unwrap();
}

/// Role names shown in the feed are resolved from the **profile** (spec §5.1), so
/// they arrive on chat activation and are re-sent after a profile edit — a rename
/// in settings applies to the already-open chat.
#[test]
fn character_names_come_from_the_profile_and_refresh_on_edit() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let mut profile = Profile::new("P", "sys");
    profile.character_names.user = "Гайя".into();
    let pid = profile.id;
    let chat = Chat::from_profile(&profile, "Чат");
    let id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);

    orch.activate(id);
    let names = take_character_names(&mut rx).expect("names sent on activation");
    assert_eq!(names.user, "Гайя");
    assert_eq!(names.assistant, "", "an unset name stays unset");

    orch.handle_update_profile(
        pid,
        ProfileEdit {
            character_names: Some(crate::entities::profile::CharacterNames {
                user: "Гайя".into(),
                assistant: "Анна".into(),
                system: String::new(),
            }),
            ..Default::default()
        },
    );
    let names = take_character_names(&mut rx).expect("names re-sent after the edit");
    assert_eq!(names.assistant, "Анна");
}

/// The last `CharacterNames` event in the queue (the handlers also emit list/settings
/// events, so we filter rather than take the head).
fn take_character_names(
    rx: &mut UnboundedReceiver<AppEvent>,
) -> Option<crate::entities::profile::CharacterNames> {
    let mut found = None;
    while let Ok(ev) = rx.try_recv() {
        if let AppEvent::CharacterNames(names) = ev {
            found = Some(names);
        }
    }
    found
}

/// Bootstrap clears the legacy seed role names (never displayed before this
/// feature), so the feed/export fall back to the interface language's labels.
#[tokio::test]
async fn bootstrap_clears_legacy_seed_character_names() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let storage = Arc::new(Storage::open(Paths::with_root(&root)).unwrap());

    let mut seeded = Profile::new("Старый", "sys");
    seeded.character_names = crate::entities::profile::CharacterNames {
        user: "Вы".into(),
        assistant: "Assistant".into(),
        system: "Система".into(),
    };
    let mut custom = Profile::new("Свой", "sys");
    custom.character_names.assistant = "Анна".into();
    let (seeded_id, custom_id) = (seeded.id, custom.id);
    storage.json().upsert_profile(&seeded).unwrap();
    storage.json().upsert_profile(&custom).unwrap();

    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, mut evt_rx) = unbounded_channel();
    let handle = tokio::spawn(run(OrchestratorDeps {
        cmd_rx,
        evt_tx,
        storage: storage.clone(),
        config: AppConfig::default(),
        supervisor: Arc::new(MockSupervisor::with_backend(None)),
        default_language: crate::shared::i18n::Lang::default(),
    }));
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    drop(cmd_tx);
    handle.await.unwrap();

    let stored = storage.json().load_profiles().unwrap();
    let seeded = stored.iter().find(|p| p.id == seeded_id).unwrap();
    assert_eq!(
        seeded.character_names,
        crate::entities::profile::CharacterNames::default(),
        "seed names are cleared"
    );
    let custom = stored.iter().find(|p| p.id == custom_id).unwrap();
    assert_eq!(
        custom.character_names.assistant, "Анна",
        "a chosen name stays"
    );
}

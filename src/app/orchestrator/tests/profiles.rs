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
        extra_tools: Vec::new(),
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
        extra_tools: Vec::new(),
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

/// A fresh install: bootstrap makes one profile and one empty chat, and the
/// scaffold language stays **editable** — the pristine chat binds nothing
/// (spec §10). Switching it re-derives the untouched localized defaults: the
/// profile's name and system message, the chat's title and system-message
/// copy — in memory, in the emitted events, and on disk.
#[tokio::test]
async fn fresh_install_language_is_editable_and_rederives_defaults() {
    let ru = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
    let en = crate::shared::i18n::locale(crate::shared::i18n::Lang::En);
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch(None);

    // Bootstrap: one ru profile, one empty chat — and no locked language.
    let list = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ProfileList(p) if p.len() == 1),
    )
    .await
    .unwrap();
    let profile_id = match list {
        AppEvent::ProfileList(profiles) => {
            assert_eq!(profiles[0].name, ru.t("defaults.profile_name"));
            profiles[0].id
        }
        _ => unreachable!(),
    };
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::Settings { language_locked, .. } if language_locked.is_empty()),
    )
    .await
    .expect("the bootstrap chat is pristine — the language must not be locked");

    // The settings screen sends a full snapshot; the untouched defaults ride
    // along byte-equal to the ru bundle and must follow the language.
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: profile_id,
            edit: Box::new(ProfileEdit {
                name: Some(ru.t("defaults.profile_name").into()),
                system_message: Some(ru.t("defaults.system_message").into()),
                language: Some(crate::shared::i18n::Lang::En),
                ..Default::default()
            }),
        })
        .unwrap();

    // The pristine chat's default title follows — through the same event a
    // manual rename sends, so an open chat's header updates too.
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ChatRenamed { title, .. } if title == en.t("defaults.chat_title")),
    )
    .await
    .expect("the pristine chat is re-derived in the new language");
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ProfileList(p) if p[0].name == en.t("defaults.profile_name")),
    )
    .await
    .unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::Settings { language_locked, .. } if language_locked.is_empty()),
    )
    .await
    .expect("still no data — the language stays editable");

    drop(cmd_tx);
    handle.await.unwrap();

    // What reached disk (the deferred chat save flushes on exit).
    let storage = Storage::open(Paths::with_root(dir.path())).unwrap();
    let profiles = storage.json().load_profiles().unwrap();
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].language, crate::shared::i18n::Lang::En);
    assert_eq!(profiles[0].name, en.t("defaults.profile_name"));
    assert_eq!(
        profiles[0].default_system_message,
        en.t("defaults.system_message")
    );
    let chats = storage.json().load_chats().unwrap();
    assert_eq!(chats.len(), 1);
    assert_eq!(chats[0].title, en.t("defaults.chat_title"));
    assert_eq!(chats[0].system_message, en.t("defaults.system_message"));
}

/// What locks the scaffold language is conversation content, not the chat's
/// existence: messages (a greeting copy included), deleted-exchange tombstones
/// and a compaction summary each lock; a pristine chat doesn't (spec §10).
#[test]
fn conversation_content_locks_profile_language() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let profile = Profile::new("P", "sys");
    let pid = profile.id;
    orch.profiles.push(profile);
    orch.chats
        .push(Chat::from_profile(&orch.profiles[0], "Новый чат"));
    assert!(!orch.profile_has_data(pid), "a pristine chat binds nothing");

    // A greeting-only chat already holds an assistant message — it locks.
    orch.chats[0].push_message(Message::assistant("Здравствуйте!"));
    assert!(orch.profile_has_data(pid));

    // The authoritative gate: the switch is dropped and reported.
    orch.handle_update_profile(
        pid,
        ProfileEdit {
            language: Some(crate::shared::i18n::Lang::En),
            ..Default::default()
        },
    );
    assert_eq!(
        orch.profiles[0].language,
        crate::shared::i18n::Lang::default(),
        "the locked language must not change"
    );
    let mut saw_error = false;
    while let Ok(ev) = rx.try_recv() {
        saw_error |= matches!(ev, AppEvent::Error(_));
    }
    assert!(saw_error, "the rejected switch is reported");

    // Tombstones are restorable conversation content — they lock on their own.
    orch.chats[0].messages.clear();
    assert!(!orch.profile_has_data(pid));
    orch.chats[0].record_deleted(
        vec![Message::user("привет")],
        String::new(),
        crate::entities::chat::DeletedCause::DeleteExchange,
    );
    assert!(orch.profile_has_data(pid));

    // So is a compaction summary.
    orch.chats[0].deleted.clear();
    orch.chats[0].compaction = Some(crate::entities::chat::Compaction {
        summary: "сводка".into(),
        upto: 0,
        boundary_id: Uuid::new_v4(),
        compacted_at: chrono::Utc::now(),
        rolls: 1,
    });
    assert!(orch.profile_has_data(pid));
}

/// The re-derivation respects the user's text: a custom profile name/system
/// message and a manually chosen chat title survive the switch untouched; the
/// pristine chat's system-message copy tracks the profile (as if created now).
#[test]
fn language_switch_keeps_user_edited_texts() {
    let (_d, mut orch) = bare_orch();
    let mut profile = Profile::new("Гея", "Ты — Гея.");
    profile.language = crate::shared::i18n::Lang::Ru;
    let pid = profile.id;
    orch.profiles.push(profile);
    let mut chat = Chat::from_profile(&orch.profiles[0], "Мой чат");
    chat.renamed_manually = true;
    let cid = chat.id;
    orch.chats.push(chat);

    orch.handle_update_profile(
        pid,
        ProfileEdit {
            language: Some(crate::shared::i18n::Lang::En),
            ..Default::default()
        },
    );

    assert_eq!(orch.profiles[0].language, crate::shared::i18n::Lang::En);
    assert_eq!(orch.profiles[0].name, "Гея");
    assert_eq!(orch.profiles[0].default_system_message, "Ты — Гея.");
    let chat = orch.chats.iter().find(|c| c.id == cid).unwrap();
    assert_eq!(
        chat.title, "Мой чат",
        "a manual rename outranks re-derivation"
    );
    assert_eq!(chat.system_message, "Ты — Гея.");
}

/// `/profile fork` (spec §10): the active chat's profile, copied under the
/// default name with its memory and none of its chats; the store and
/// `profiles.json` both hold it, and the notice says what came along.
#[test]
fn fork_copies_the_active_profile_and_its_memory() {
    use crate::entities::self_model_history::VersionAuthor;
    let (_d, mut orch, mut rx) = bare_orch_rx();
    orch.config.interface.language = crate::shared::i18n::Lang::En;
    let mut profile = Profile::new("Sol", "You are Sol.");
    profile.companion.notes_in_prompt = true;
    let pid = profile.id;
    let chat = Chat::from_profile(&profile, "t");
    orch.active_id = Some(chat.id);
    orch.chats.push(chat);
    orch.profiles.push(profile.clone());
    orch.storage.json().upsert_profile(&profile).unwrap();
    let storage = orch.storage.clone();
    let db = storage.db();
    db.note_insert(&crate::entities::note::Note::new(
        pid,
        "Alex likes chess",
        vec![],
    ))
    .unwrap();
    db.self_model_update(pid, &VersionAuthor::Person, |m| {
        m.summary = "I am Sol.".into();
        true
    })
    .unwrap();

    orch.handle_fork_profile(None);

    assert_eq!(orch.profiles.len(), 2);
    let fork = orch.profiles[1].clone();
    assert_eq!(fork.name, "Sol — fork");
    assert!(fork.companion.notes_in_prompt, "settings come along");
    let origin = fork.forked_from.clone().unwrap();
    assert_eq!(
        (origin.profile_id, origin.self_model_version),
        (pid, Some(1))
    );
    let stored = orch.storage.json().load_profiles().unwrap();
    assert!(
        stored
            .iter()
            .any(|p| p.id == fork.id && p.forked_from.is_some())
    );
    assert_eq!(db.note_list(fork.id, None, &[], None).unwrap().len(), 1);
    assert_eq!(
        db.self_model_get(fork.id).unwrap().unwrap().summary,
        "I am Sol."
    );
    assert!(
        orch.chats.iter().all(|c| c.profile_id == pid),
        "the chats stay with the source"
    );
    let notice = std::iter::from_fn(|| rx.try_recv().ok())
        .find_map(|e| match e {
            AppEvent::Notice(text) => Some(text),
            _ => None,
        })
        .expect("a notice answers the command");
    assert!(notice.contains("notes: 1"), "{notice}");
    assert!(notice.contains("self-model versions: 1"), "{notice}");

    // A name given is used as given.
    orch.handle_fork_profile(Some("Ada".into()));
    assert_eq!(orch.profiles[2].name, "Ada");
}

/// With no conversation open there is no profile to fork, and it is said.
#[test]
fn fork_without_an_open_chat_says_so() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    orch.config.interface.language = crate::shared::i18n::Lang::En;
    orch.profiles.push(Profile::new("Sol", ""));
    orch.handle_fork_profile(None);
    assert_eq!(orch.profiles.len(), 1);
    let said = std::iter::from_fn(|| rx.try_recv().ok())
        .any(|e| matches!(e, AppEvent::Notice(text) if text.contains("no profile to fork")));
    assert!(said);
}

/// The reported defect (spec §10): a fresh install opens a pristine chat, the
/// person writes a persona into the profile's system message, goes back to
/// that chat — and its first reply follows the text the chat was created with.
/// A pristine chat still holding the profile's old text now takes the new one;
/// a begun chat keeps its copy, and so does a pristine one the person set with
/// `/system`; another profile's chat is not this profile's business.
#[test]
fn a_pristine_chat_follows_a_profile_system_message_edit() {
    let (_d, mut orch) = bare_orch();
    let profile = Profile::new("P", "You are a helpful assistant.");
    let other = Profile::new("Q", "You are a helpful assistant.");
    let pid = profile.id;
    let pristine = Chat::from_profile(&profile, "new");
    let mut begun = Chat::from_profile(&profile, "begun");
    begun.push_message(Message::user("hi"));
    let mut own = Chat::from_profile(&profile, "own");
    own.system_message = "Mine.".into();
    let foreign = Chat::from_profile(&other, "foreign");
    let ids = [pristine.id, begun.id, own.id, foreign.id];
    orch.profiles.extend([profile, other]);
    orch.chats.extend([pristine, begun, own, foreign]);

    orch.handle_update_profile(
        pid,
        ProfileEdit {
            system_message: Some("You are sarcastic.".into()),
            ..Default::default()
        },
    );

    let text = |orch: &Orchestrator, id: Uuid| {
        orch.chats
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.system_message.clone())
            .unwrap()
    };
    assert_eq!(
        text(&orch, ids[0]),
        "You are sarcastic.",
        "the pristine chat follows"
    );
    assert_eq!(
        text(&orch, ids[1]),
        "You are a helpful assistant.",
        "a begun conversation keeps its copy"
    );
    assert_eq!(
        text(&orch, ids[2]),
        "Mine.",
        "a text set with /system outranks the profile's"
    );
    assert_eq!(
        text(&orch, ids[3]),
        "You are a helpful assistant.",
        "another profile's chat is untouched"
    );

    // A second edit: the pristine chat holds the previous text, so it follows
    // again — it is "as if created now" every time, not once.
    orch.handle_update_profile(
        pid,
        ProfileEdit {
            system_message: Some("You are terse.".into()),
            ..Default::default()
        },
    );
    assert_eq!(text(&orch, ids[0]), "You are terse.");
    assert_eq!(text(&orch, ids[2]), "Mine.");
}

/// `/system [text|reset|clear]` (spec §10) on the active chat: bare hands the
/// text back as a command line, an edit replaces the chat's copy and says so,
/// `reset` takes the profile's again — or says it already has it — and the
/// profile itself is never touched.
#[test]
fn chat_system_command_edits_the_chat_not_the_profile() {
    use crate::features::profile_command::ChatSystemEdit;
    let (_d, mut orch, mut rx) = bare_orch_rx();
    orch.config.interface.language = crate::shared::i18n::Lang::En;
    let profile = Profile::new("P", "You are sarcastic.");
    let mut chat = Chat::from_profile(&profile, "chat");
    chat.system_message = "You are a helpful assistant.".into();
    chat.push_message(Message::user("hi"));
    let (pid, cid) = (profile.id, chat.id);
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.activate(cid);
    while rx.try_recv().is_ok() {}

    let mut answer = |orch: &mut Orchestrator, edit: ChatSystemEdit| {
        orch.handle_chat_system(edit);
        let mut last = None;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                AppEvent::Notice(text) => last = Some(format!("notice: {text}")),
                AppEvent::RestoreInput(text) => last = Some(format!("input: {text}")),
                _ => {}
            }
        }
        last.expect("every form answers")
    };
    let text = |orch: &Orchestrator| orch.chats[0].system_message.clone();

    assert_eq!(
        answer(&mut orch, ChatSystemEdit::Show),
        "input: /system You are a helpful assistant."
    );
    let reset = answer(&mut orch, ChatSystemEdit::Reset);
    assert!(reset.starts_with("notice: "), "{reset}");
    assert_eq!(text(&orch), "You are sarcastic.");
    let again = answer(&mut orch, ChatSystemEdit::Reset);
    assert!(again.contains("already"), "{again}");

    let set = answer(
        &mut orch,
        ChatSystemEdit::Set("Line one.\nLine two.".into()),
    );
    assert!(
        set.contains("/profile system"),
        "names the profile's route: {set}"
    );
    assert_eq!(text(&orch), "Line one.\nLine two.");

    let cleared = answer(&mut orch, ChatSystemEdit::Clear);
    assert!(
        cleared.contains("/system reset"),
        "names the way back: {cleared}"
    );
    assert_eq!(text(&orch), "");
    let empty = answer(&mut orch, ChatSystemEdit::Show);
    assert!(
        empty.starts_with("notice: ") && empty.contains("/system"),
        "{empty}"
    );

    assert_eq!(
        orch.profiles
            .iter()
            .find(|p| p.id == pid)
            .unwrap()
            .default_system_message,
        "You are sarcastic.",
        "the profile is never touched"
    );
}

/// The reported flow end to end, on what reaches the engine: a fresh install's
/// first chat, the persona written into the profile, the first message — the
/// request carries the persona, not the text the chat was created with. Then
/// the conversation is under way: a second profile edit leaves it alone until
/// `/system reset` takes the profile's text into it.
#[tokio::test]
async fn the_first_request_after_a_persona_edit_carries_the_persona() {
    use crate::features::profile_command::ChatSystemEdit;
    let backend = CapturingBackend::new();
    let (_dir, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend.clone()), no_auto_cfg());
    let profile_id = match wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ProfileList(p) if p.len() == 1),
    )
    .await
    .unwrap()
    {
        AppEvent::ProfileList(profiles) => profiles[0].id,
        _ => unreachable!(),
    };
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let edit = |text: &str| AppCommand::UpdateProfile {
        id: profile_id,
        edit: Box::new(ProfileEdit {
            system_message: Some(text.into()),
            ..Default::default()
        }),
    };
    let turn = |text: &str| AppCommand::SendMessage(text.into());

    cmd_tx.send(edit("You are sarcastic and brief.")).unwrap();
    cmd_tx.send(turn("Hi! My name is Alex.")).unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    let system = backend.last_request().system.unwrap_or_default();
    assert!(
        system.starts_with("You are sarcastic and brief."),
        "the first request must carry the persona: {system}"
    );

    // Begun: the profile's next edit is for new conversations…
    cmd_tx.send(edit("You are terse.")).unwrap();
    cmd_tx.send(turn("And now?")).unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    let system = backend.last_request().system.unwrap_or_default();
    assert!(
        system.starts_with("You are sarcastic and brief."),
        "{system}"
    );

    // …until `/system reset` takes it into this one.
    cmd_tx
        .send(AppCommand::ChatSystem(ChatSystemEdit::Reset))
        .unwrap();
    cmd_tx.send(turn("And now?")).unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    let system = backend.last_request().system.unwrap_or_default();
    assert!(system.starts_with("You are terse."), "{system}");

    drop(cmd_tx);
    handle.await.unwrap();
}

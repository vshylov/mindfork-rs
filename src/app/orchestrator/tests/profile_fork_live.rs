//! A live `#[ignore]` smoke for forking a companion (spec §10, `/profile fork`;
//! docs/research/companion-platform.md C8 (a)): a profile whose self-model
//! knows the person is forked, and a new conversation with the fork — none of
//! the source's chats came along — must still know them, because the memory
//! did. The copy itself is pinned by the storage tests; what a mock cannot
//! answer is whether a real model, given the fork's prompt, uses it.
//!
//! `MINDFORK_ENGINE_URL=…/v1 cargo test profile_fork_live -- --ignored --nocapture --test-threads=1`

use super::*;
use crate::entities::self_model_history::VersionAuthor;

const PERSONA: &str = "You are Sol, Alex's companion. You talk with Alex the way a close \
friend would: warm, honest, curious. Keep replies conversational and short.";

#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn a_fork_remembers_what_its_source_knew_live() {
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let json = crate::shared::storage::JsonStore::new(Paths::with_root(&root));
    let mut profile = Profile::new("Sol", PERSONA);
    profile.language = crate::shared::i18n::Lang::En;
    profile.enabled_tools = crate::features::tools::all_tool_ids();
    profile.known_tools = profile.enabled_tools.clone();
    json.upsert_profile(&profile).unwrap();
    let mut chat = Chat::from_profile(&profile, "Alex");
    chat.push_message(Message::user("Hi Sol!"));
    chat.push_message(Message::assistant("Hi Alex!"));
    json.save_chat(&chat).unwrap();
    {
        let storage = Storage::open(Paths::with_root(&root)).unwrap();
        storage
            .db()
            .self_model_update(profile.id, &VersionAuthor::Person, |m| {
                m.summary = "I am Sol, Alex's companion.".into();
                m.user_model.perceived_traits = vec!["patient".into(), "competitive".into()];
                m.user_model.current_interests =
                    vec!["chess — plays the Sicilian Defence every Sunday".into()];
                m.user_model.relationship_dynamic = "Old friends.".into();
                true
            })
            .unwrap();
    }

    let mut config = AppConfig::default();
    config.engine.mode = crate::shared::config::ServerMode::External;
    let storage = Arc::new(Storage::open(Paths::with_root(&root)).unwrap());
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, mut evt_rx) = unbounded_channel();
    let handle = tokio::spawn(run(OrchestratorDeps {
        cmd_rx,
        evt_tx,
        storage,
        config,
        supervisor: Arc::new(MockSupervisor::with_backend(Some(backend))),
        default_language: crate::shared::i18n::Lang::En,
        extra_tools: Vec::new(),
    }));
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    cmd_tx
        .send(AppCommand::ForkProfile {
            name: Some("Sol b".into()),
        })
        .unwrap();
    let Some(AppEvent::ProfileList(list)) = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ProfileList(list) if list.len() == 2),
    )
    .await
    else {
        unreachable!()
    };
    let fork = list.iter().find(|p| p.name == "Sol b").unwrap().id;
    cmd_tx
        .send(AppCommand::NewChat {
            profile_id: Some(fork),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let (reply, tools) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "Quick one: what do you know about my hobbies?",
    )
    .await;
    eprintln!("fork's reply: tools={tools:?}\n{reply}\n");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let storage = Storage::open(Paths::with_root(&root)).unwrap();
    let chats = storage.json().load_chats().unwrap();
    assert_eq!(
        chats.iter().filter(|c| c.profile_id == fork).count(),
        1,
        "the fork has only the conversation it was given"
    );
    let lower = reply.to_lowercase();
    assert!(
        lower.contains("chess") || lower.contains("sicilian"),
        "the fork does not know what its source knew: {reply:?}"
    );
    // The source's self-model is untouched by whatever the fork's turn wrote.
    let source = storage.db().self_model_get(profile.id).unwrap().unwrap();
    assert_eq!(source.summary, "I am Sol, Alex's companion.");
}

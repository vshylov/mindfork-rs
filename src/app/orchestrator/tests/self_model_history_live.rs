//! A live `#[ignore]` smoke for the self-model's history (spec §17.9;
//! docs/research/companion-platform.md C9): a real model writes the self-model
//! through its tools in a turn, a background reflection may write more, and the
//! person edits it on `F3` — each write must be a version of its own, recorded
//! as the right author, the model by the name the turn knew it by.
//!
//! What a mock cannot answer is only the first part — that a real model's tool
//! call reaches the store with the turn's name and chat; the rules themselves
//! are pinned by the storage and orchestrator tests.
//!
//! `MINDFORK_ENGINE_URL=…/v1 cargo test self_model_history_live -- --ignored --nocapture --test-threads=1`

use super::*;
use crate::entities::self_model::SelfModelEdit;
use crate::entities::self_model_history::VersionAuthor;

/// How long the reflection the turn sets off may take before the smoke stops
/// waiting for it — its writes are reported, not required.
const REFLECTION_WAIT: std::time::Duration = std::time::Duration::from_secs(300);

#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn self_model_history_records_who_wrote_each_version_live() {
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let mut config = AppConfig::default();
    // A server by URL: the app learns the model's name by asking it.
    config.engine.mode = crate::shared::config::ServerMode::External;
    // A reflection after every reply: its versions are the background kind.
    config.self_model.auto_reflect_every = 1;
    let (dir, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend), config);
    let root = dir.path().to_path_buf();
    // The name is asked for when the engine comes up; a turn sent before the
    // answer knows none, and records none — which is the truth, and not what
    // this smoke is about. The profile list and the name arrive in either
    // order, so both are waited for in one loop.
    let (mut pid, mut named) = (None, None);
    while pid.is_none() || named.is_none() {
        match evt_rx.recv().await.expect("the app ended") {
            AppEvent::ProfileList(list) => pid = Some(list[0].id),
            AppEvent::EngineModel(Some(name)) => named = Some(name),
            _ => {}
        }
    }
    let pid = pid.unwrap();
    eprintln!("engine model: {named:?}");
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: pid,
            edit: Box::new(ProfileEdit {
                enabled_tools: Some(crate::features::tools::all_tool_ids()),
                ..Default::default()
            }),
        })
        .unwrap();

    let (text, tools) = run_turn_live(
        &cmd_tx,
        &mut evt_rx,
        "My name is Alex, I write Rust and I dislike wordiness. Remember this: call \
         update_user_model (traits, interests) and update_self_model (a short description \
         of yourself, and the goal of helping me briefly and to the point).",
    )
    .await;
    eprintln!("turn: tools={tools:?}\ntext={text:?}\n");

    // The reflection the reply set off, if it starts: wait for it to land.
    let landed = tokio::time::timeout(REFLECTION_WAIT, async {
        let mut started = false;
        while let Some(ev) = evt_rx.recv().await {
            if let AppEvent::BackgroundTask { kind, active } = ev
                && kind == crate::entities::task::BackgroundKind::Reflection
            {
                if active {
                    started = true;
                } else if started {
                    return true;
                }
            }
        }
        false
    })
    .await;
    eprintln!("reflection landed: {landed:?}");

    // The person's edit on `F3`.
    cmd_tx
        .send(AppCommand::UpdateSelfModel(SelfModelEdit::SetSummary(
            "An assistant who keeps it short.".into(),
        )))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::SelfModelView { .. }))
        .await
        .unwrap();
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let storage = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = storage.json().load_chats().unwrap()[0].id;
    let history = storage.db().self_model_history(pid).unwrap();
    for v in &history {
        eprintln!(
            "v{} {} {:?} summary={:?}",
            v.version,
            v.written_at.to_rfc3339(),
            v.author,
            v.model.summary.chars().take(60).collect::<String>()
        );
    }

    assert!(
        tools
            .iter()
            .any(|t| t == "update_self_model" || t == "update_user_model"),
        "the model made no self-model call — nothing for the history to record"
    );
    let turn_versions: Vec<_> = history
        .iter()
        .filter(|v| {
            matches!(
                &v.author,
                VersionAuthor::Model {
                    background: false,
                    ..
                }
            )
        })
        .collect();
    assert!(
        !turn_versions.is_empty(),
        "no version recorded as the turn's: {history:#?}"
    );
    for v in &turn_versions {
        let VersionAuthor::Model { name, chat_id, .. } = &v.author else {
            unreachable!()
        };
        assert!(
            name.as_deref().is_some_and(|n| !n.is_empty()),
            "the turn's model has no name: {:?}",
            v.author
        );
        assert_eq!(*chat_id, Some(chat), "recorded against another chat");
    }
    // Every write is its own version, numbered without a gap, ending in the
    // person's edit.
    let numbers: Vec<u64> = history.iter().map(|v| v.version).collect();
    assert_eq!(numbers, (1..=history.len() as u64).collect::<Vec<_>>());
    let last = history.last().unwrap();
    assert_eq!(last.author, VersionAuthor::Person);
    assert_eq!(last.model.summary, "An assistant who keeps it short.");
    // A background version, when the reflection wrote one, names the model too.
    for v in &history {
        if let VersionAuthor::Model {
            background: true,
            name,
            ..
        } = &v.author
        {
            assert!(name.is_some(), "a reflection's version without its model");
        }
    }
}

//! A live `#[ignore]` smoke for carrying a companion (spec §12.5,
//! `mindfork companion export|import`; docs/research/companion-platform.md
//! C8 (b)): a companion whose self-model knows the person is exported from one
//! data root, encrypted, and imported into another that has never seen it; the
//! first conversation there must know the person, because the memory crossed.
//! The archive's shape is pinned by the feature's tests; what a mock cannot
//! answer is whether a real model, given the imported prompt, uses it.
//!
//! `MINDFORK_ENGINE_URL=…/v1 cargo test companion_carry_live -- --ignored --nocapture --test-threads=1`

use super::*;
use crate::entities::self_model_history::VersionAuthor;
use crate::features::companion_carry;

const PERSONA: &str = "You are Sol, Alex's companion. You talk with Alex the way a close \
friend would: warm, honest, curious. Keep replies conversational and short.";

#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn a_carried_companion_remembers_on_the_other_machine_live() {
    let Some(backend) = live_backend() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::En);

    // Here: the companion, its self-model knowing Alex.
    let here = Paths::with_root(dir.path().join("here"));
    let mut profile = Profile::new("Sol", PERSONA);
    profile.language = crate::shared::i18n::Lang::En;
    profile.enabled_tools = crate::features::tools::all_tool_ids();
    profile.known_tools = profile.enabled_tools.clone();
    {
        let storage = Storage::open(here.clone()).unwrap();
        storage.json().upsert_profile(&profile).unwrap();
        storage
            .db()
            .self_model_update(profile.id, &VersionAuthor::Person, |m| {
                m.summary = "I am Sol, Alex's companion.".into();
                m.user_model.perceived_traits = vec!["patient".into()];
                m.user_model.current_interests =
                    vec!["pottery — throws bowls on a wheel every Saturday".into()];
                true
            })
            .unwrap();
    }
    let archive = dir.path().join("sol.zip");
    let out =
        companion_carry::export(&here, "Sol", Some(archive.clone()), Some("t3st"), loc).unwrap();
    eprintln!("exported: {out:?}");

    // There: a root that has never seen it.
    let there = Paths::with_root(dir.path().join("there"));
    let storage = Arc::new(Storage::open(there.clone()).unwrap());
    let landed = companion_carry::import(&there, &storage, &archive, Some("t3st"), loc).unwrap();
    eprintln!("imported: {landed:?}");

    let mut config = AppConfig::default();
    config.engine.mode = crate::shared::config::ServerMode::External;
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
        .send(AppCommand::NewChat {
            profile_id: Some(landed.profile_id),
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
    eprintln!("the carried companion's reply: tools={tools:?}\n{reply}\n");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let storage = Storage::open(there).unwrap();
    let profiles = storage.json().load_profiles().unwrap();
    assert!(
        profiles
            .iter()
            .any(|p| p.id == landed.profile_id && p.name == "Sol")
    );
    let lower = reply.to_lowercase();
    assert!(
        lower.contains("pottery") || lower.contains("bowl"),
        "the carried companion does not know what it knew: {reply:?}"
    );
}

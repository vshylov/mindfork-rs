//! The language-model history recorder (spec §9.14): `handle_done` appends a
//! record to the chat's profile after an exchange whose model name is known —
//! the dedup itself is `Db::llm_history_note`'s and is covered in the storage
//! tests; here — what the orchestrator feeds it and when it stays silent.

use super::*;
use crate::entities::message::{MessageFinish, MessageMetadata};
use crate::entities::profile::LlmChange;
use crate::shared::config::ServerMode;

/// An assistant reply carrying the metadata `finalize_message` writes.
fn reply(model: Option<&str>, mode: ServerMode, finish: MessageFinish) -> Message {
    let mut msg = Message::assistant("ответ");
    msg.metadata = Some(MessageMetadata {
        sampling: SamplingConfig::default(),
        mode,
        model: model.map(Into::into),
        finish: Some(finish),
    });
    msg
}

/// Lands `messages` as one finished turn, the way the generation task does.
fn land_turn(orch: &mut Orchestrator, chat_id: Uuid, messages: Vec<Message>) {
    let gen_id = Uuid::new_v4();
    orch.gen_state
        .begin(gen_id, tokio_util::sync::CancellationToken::new());
    orch.handle_done(super::super::generation::GenResult {
        id: gen_id,
        chat_id,
        messages,
        effects: vec![],
        deleted: vec![],
        usage: None,
        continuation: None,
    });
}

/// A bare orchestrator with one profile and one chat holding a user message.
fn orch_with_chat() -> (tempfile::TempDir, Orchestrator, Uuid, Uuid) {
    let (dir, mut orch, _rx) = bare_orch_rx();
    let profile = Profile::new("P", "sys");
    let pid = profile.id;
    let mut chat = Chat::from_profile(&profile, "t");
    chat.push_message(Message::user("привет"));
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);
    (dir, orch, pid, chat_id)
}

#[tokio::test]
async fn a_completed_exchange_writes_the_baseline_and_then_only_changes() {
    let (_d, mut orch, pid, chat_id) = orch_with_chat();

    // First exchange — the baseline record (fork F6): the original model
    // would otherwise be unrecoverable.
    land_turn(
        &mut orch,
        chat_id,
        vec![reply(
            Some("gemma-4"),
            ServerMode::Managed,
            MessageFinish::Stop,
        )],
    );
    // Same model again — no duplicate (fork F4: the pair name+mode decides).
    land_turn(
        &mut orch,
        chat_id,
        vec![reply(
            Some("gemma-4"),
            ServerMode::Managed,
            MessageFinish::Stop,
        )],
    );
    // A different name — a record.
    land_turn(
        &mut orch,
        chat_id,
        vec![reply(
            Some("qwen-3.6"),
            ServerMode::Managed,
            MessageFinish::Stop,
        )],
    );
    // The same name through a different mode — a record too: the stored mode
    // must not go stale.
    land_turn(
        &mut orch,
        chat_id,
        vec![reply(
            Some("qwen-3.6"),
            ServerMode::External,
            MessageFinish::Stop,
        )],
    );

    let history = orch.storage.db().llm_history(pid).unwrap();
    assert_eq!(
        history
            .iter()
            .map(|r| (r.model.as_str(), r.mode))
            .collect::<Vec<_>>(),
        [
            ("gemma-4", ServerMode::Managed),
            ("qwen-3.6", ServerMode::Managed),
            ("qwen-3.6", ServerMode::External),
        ]
    );
}

#[tokio::test]
async fn an_engine_that_did_not_say_a_name_records_nothing() {
    let (_d, mut orch, pid, chat_id) = orch_with_chat();
    // External mode with no typed name and no discovery answer yet — the
    // requirement's "the API does not expose a name" case.
    land_turn(
        &mut orch,
        chat_id,
        vec![reply(None, ServerMode::External, MessageFinish::Stop)],
    );
    assert!(orch.storage.db().llm_history(pid).unwrap().is_empty());
}

#[tokio::test]
async fn a_cancelled_partial_reply_still_counts_as_the_models_work() {
    // Fork F5(a): the model demonstrably answered in this profile — a partial
    // reply the user kept is an exchange; the empty turn never gets here (the
    // `handle_done` early return drops it before the recorder runs).
    let (_d, mut orch, pid, chat_id) = orch_with_chat();
    land_turn(
        &mut orch,
        chat_id,
        vec![reply(
            Some("gemma-4"),
            ServerMode::Managed,
            MessageFinish::Cancelled,
        )],
    );
    assert_eq!(orch.storage.db().llm_history(pid).unwrap().len(), 1);
}

#[tokio::test]
async fn the_newest_metadata_of_the_turn_names_the_record() {
    // A tool round stores no metadata (`finalize_message` returns `None` for
    // it); the extraction walks from the newest message back to the first one
    // that carries a name.
    let (_d, mut orch, pid, chat_id) = orch_with_chat();
    let mut bare = Message::assistant("");
    bare.metadata = None;
    land_turn(
        &mut orch,
        chat_id,
        vec![
            reply(Some("gemma-4"), ServerMode::Managed, MessageFinish::Stop),
            bare,
        ],
    );
    let history = orch.storage.db().llm_history(pid).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].model, "gemma-4");
}

#[tokio::test]
async fn the_record_lands_under_the_chats_own_profile() {
    // Two profiles, two chats: each exchange writes its own profile's history
    // (the §9.5 isolation invariant, here on the recorder's side).
    let (_d, mut orch, pid_a, chat_a) = orch_with_chat();
    let profile_b = Profile::new("B", "sys");
    let pid_b = profile_b.id;
    let mut chat = Chat::from_profile(&profile_b, "t2");
    chat.push_message(Message::user("привет"));
    let chat_b = chat.id;
    orch.profiles.push(profile_b);
    orch.chats.push(chat);

    land_turn(
        &mut orch,
        chat_a,
        vec![reply(
            Some("gemma-4"),
            ServerMode::Managed,
            MessageFinish::Stop,
        )],
    );
    orch.active_id = Some(chat_b);
    land_turn(
        &mut orch,
        chat_b,
        vec![reply(
            Some("gpt-5.2"),
            ServerMode::OpenAi,
            MessageFinish::Stop,
        )],
    );

    let a = orch.storage.db().llm_history(pid_a).unwrap();
    let b = orch.storage.db().llm_history(pid_b).unwrap();
    assert_eq!(
        a.iter().map(|r| r.model.as_str()).collect::<Vec<_>>(),
        ["gemma-4"]
    );
    assert_eq!(
        b.iter().map(|r| r.model.as_str()).collect::<Vec<_>>(),
        ["gpt-5.2"]
    );
}

#[tokio::test]
async fn a_turn_for_a_vanished_chat_records_nothing_and_does_not_panic() {
    // Best-effort (docs/lessons.md §8): the chat was deleted while the turn
    // ran — `record_llm_history` cannot resolve a profile and returns instead
    // of failing the turn's landing.
    let (_d, mut orch, pid, _chat_id) = orch_with_chat();
    land_turn(
        &mut orch,
        Uuid::new_v4(),
        vec![reply(
            Some("gemma-4"),
            ServerMode::Managed,
            MessageFinish::Stop,
        )],
    );
    assert!(orch.storage.db().llm_history(pid).unwrap().is_empty());
}

#[test]
fn llm_change_compares_by_name_and_mode() {
    // The dedup contract in one place: equal pair — equal records (timestamps
    // aside), and either half differing breaks the equality the store checks.
    let at = chrono::Utc::now();
    let base = LlmChange {
        changed_at: at,
        model: "m".into(),
        mode: ServerMode::Managed,
    };
    assert_eq!(base, base.clone());
    assert_ne!(
        base,
        LlmChange {
            model: "other".into(),
            ..base.clone()
        }
    );
    assert_ne!(
        base,
        LlmChange {
            mode: ServerMode::External,
            ..base.clone()
        }
    );
}

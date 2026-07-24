//! Orchestrator tests — impersonation (request building + streaming). Part of the [`super`]
//! module (fixtures in mod.rs). See docs/history/refactoring-god-objects.md, stage 3.

use super::*;

#[test]
fn impersonation_request_swaps_roles_and_sets_system() {
    let profile = Profile::new("P", "sys ассистента");
    let mut chat = Chat::from_profile(&profile, "c");
    chat.push_message(Message::assistant("Привет! Чем помочь?"));
    chat.push_message(Message::user("Расскажи о Rust"));
    chat.push_message(Message::assistant("Rust — системный язык…"));

    let req = build_impersonation_request(
        &chat,
        "Ты — пользователь".into(),
        "",
        SamplingConfig::default(),
        None,
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
    );

    assert_eq!(req.system.as_deref(), Some("Ты — пользователь"));
    assert!(req.tools.is_empty());
    // Roles are swapped: assistant↔user.
    assert_eq!(req.messages.len(), 3);
    assert_eq!(
        req.messages[0].role,
        crate::shared::api::contract::ApiRole::User
    );
    assert_eq!(req.messages[0].content, "Привет! Чем помочь?");
    assert_eq!(
        req.messages[1].role,
        crate::shared::api::contract::ApiRole::Assistant
    );
    assert_eq!(req.messages[1].content, "Расскажи о Rust");
    assert_eq!(
        req.messages[2].role,
        crate::shared::api::contract::ApiRole::User
    );
}

#[test]
fn impersonation_request_disables_reasoning() {
    // Impersonation discards "thoughts", so the request must suppress reasoning —
    // otherwise models with thinking "baked into" the template (Gemma/Qwen) spend their whole
    // budget on reasoning_content, and the reply text comes back empty (an empty preview).
    let profile = Profile::new("P", "sys");
    let chat = Chat::from_profile(&profile, "c");
    let req = build_impersonation_request(
        &chat,
        "Ты — пользователь".into(),
        "",
        // The user left "thoughts" enabled in the impersonation sampling —
        // the request must still turn them off.
        SamplingConfig {
            thinking: Some(true),
            ..Default::default()
        },
        None,
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
    );
    assert_eq!(req.sampling.thinking, Some(false));
    assert_eq!(req.sampling.reasoning_budget, Some(0));
    assert_eq!(
        req.sampling.reasoning_effort,
        Some(crate::entities::sampling::ReasoningEffort::None)
    );
}

#[test]
fn impersonation_request_with_seed_adds_continuation_hint() {
    let profile = Profile::new("P", "sys");
    let chat = Chat::from_profile(&profile, "c");
    let req = build_impersonation_request(
        &chat,
        "Ты — пользователь".into(),
        "Мне нужно ",
        SamplingConfig::default(),
        None,
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
    );
    let system = req.system.unwrap();
    assert!(system.contains("Ты — пользователь"));
    assert!(
        system.contains("Мне нужно"),
        "the seed made it into the instruction"
    );
}

#[test]
fn impersonation_request_includes_user_hint() {
    let profile = Profile::new("P", "sys");
    let chat = Chat::from_profile(&profile, "c");
    let req = build_impersonation_request(
        &chat,
        "Ты — пользователь".into(),
        "",
        SamplingConfig::default(),
        Some("Известное о человеке: черты — скептик"),
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
    );
    let system = req.system.unwrap();
    assert!(system.contains("Ты — пользователь"));
    // The interlocutor model is mixed into the impersonation system prompt.
    assert!(system.contains("черты — скептик"));
}

#[test]
fn swap_role_skips_system_tool_and_empty() {
    assert!(swap_role_message(&Message::new(MessageRole::System, "x")).is_none());
    assert!(swap_role_message(&Message::new(MessageRole::Tool, "x")).is_none());
    assert!(swap_role_message(&Message::user("   ")).is_none());
}

#[tokio::test]
async fn impersonate_streams_into_preview_and_finishes() {
    // The first request (send) → a reply; the second (impersonation) → a message.
    let backend = Arc::new(MockBackend::sequence(vec![
        vec![
            ChatChunk::Text("ответ".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
        vec![
            ChatChunk::Text("моя реплика".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
    ])) as Arc<dyn EngineBackend>;
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    // Need at least one message in the history.
    cmd_tx
        .send(AppCommand::SendMessage("привет".into()))
        .unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();

    cmd_tx
        .send(AppCommand::Impersonate {
            seed: String::new(),
        })
        .unwrap();
    // Impersonation starts.
    wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::ImpersonationStarted { .. })
    })
    .await
    .unwrap();
    // The message text arrives in deltas.
    let chunk = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::ImpersonationChunk { .. })
    })
    .await
    .unwrap();
    assert!(matches!(chunk, AppEvent::ImpersonationChunk { text, .. } if text == "моя реплика"));
    // Finishes with Stop.
    let fin = wait_for(&mut evt_rx, |e| {
        matches!(e, AppEvent::ImpersonationFinished { .. })
    })
    .await
    .unwrap();
    assert!(matches!(
        fin,
        AppEvent::ImpersonationFinished {
            reason: FinishReason::Stop,
            ..
        }
    ));

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

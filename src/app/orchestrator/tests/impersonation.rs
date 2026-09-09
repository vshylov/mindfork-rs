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
        None,
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
        None,
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
        None,
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
        None,
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

/// A chat whose older half is folded away, plus the view over it. Shaped like a
/// real one: the greeting, two exchanges folded, one exchange verbatim — and the
/// boundary on a `User` message, which is what `plan_cut` guarantees.
fn compacted_chat() -> Chat {
    let profile = Profile::new("P", "sys");
    let mut chat = Chat::from_profile(&profile, "c");
    chat.push_message(Message::assistant("Привет! Чем помочь?"));
    chat.push_message(Message::user("Первый вопрос"));
    chat.push_message(Message::assistant("Первый ответ"));
    chat.push_message(Message::user("Второй вопрос"));
    chat.push_message(Message::assistant("Второй ответ"));
    let boundary = 3; // The second user message — a cut always lands on a User one.
    chat.compaction = Some(crate::entities::chat::Compaction {
        summary: "Ранее: обсудили первый вопрос.".into(),
        upto: boundary,
        boundary_id: chat.messages[boundary].id,
        compacted_at: chrono::Utc::now(),
        rolls: 1,
    });
    chat
}

#[test]
fn impersonation_folds_the_compacted_prefix_and_carries_the_summary() {
    let chat = compacted_chat();
    let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
    let req = build_impersonation_request(
        &chat,
        chat.compaction_view(true),
        "Ты — пользователь".into(),
        "",
        SamplingConfig::default(),
        None,
        loc,
    );

    // Only the verbatim tail is sent, and the folded part is not in it.
    assert_eq!(req.messages.len(), 2, "the tail is messages[3..]");
    let sent: Vec<&str> = req.messages.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(sent, vec!["Второй вопрос", "Второй ответ"]);
    assert!(
        !sent.iter().any(|t| t.contains("Первый")),
        "the folded exchange must not be sent verbatim: {sent:?}"
    );
    // …and what replaced it is in the system prompt, after the persona.
    let system = req.system.unwrap();
    assert!(system.starts_with("Ты — пользователь"));
    assert!(system.contains("Ранее: обсудили первый вопрос."));
}

/// Impersonation has **no** tools, so the block must tell the model to work from
/// the summary rather than point it at `history_read`/`history_search` it cannot
/// call — the dead-end wording the whole S12 gate exists to prevent.
#[test]
fn impersonation_summary_block_does_not_offer_the_read_back_tools() {
    let chat = compacted_chat();
    let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
    let req = build_impersonation_request(
        &chat,
        chat.compaction_view(true),
        "Ты — пользователь".into(),
        "",
        SamplingConfig::default(),
        None,
        loc,
    );
    let system = req.system.unwrap();
    assert!(req.tools.is_empty(), "impersonation never offers tools");
    assert!(system.contains(loc.t("compaction.block.no_tools")));
    assert!(
        !system.contains("history_read") && !system.contains("history_search"),
        "must not name tools this request does not carry: {system}"
    );
}

/// The master switch off (or nothing folded) → byte-for-byte the request that
/// was built before compression existed.
#[test]
fn impersonation_is_unchanged_when_compaction_is_off() {
    let chat = compacted_chat();
    let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
    let build = |view| {
        build_impersonation_request(
            &chat,
            view,
            "Ты — пользователь".into(),
            "",
            SamplingConfig::default(),
            None,
            loc,
        )
    };
    let off = build(chat.compaction_view(false));
    assert_eq!(off.system.as_deref(), Some("Ты — пользователь"));
    assert_eq!(off.messages.len(), 5, "the whole history is sent");
    // The stored summary is dormant, not discarded: switching back brings it in.
    assert_eq!(build(chat.compaction_view(true)).messages.len(), 2);
}

/// A cut always lands on a `User` message, and impersonation **swaps** roles —
/// so the request starts with an assistant turn, which it never did before.
/// Verified live against Anthropic, native Gemini and OpenAI Responses (all
/// accept it), but the tail matters more than the head: Anthropic treats a
/// **trailing** assistant turn as a prefill and answers with nothing. That is
/// exactly what a future change to the cut could reintroduce silently — hence
/// this test rather than a comment.
#[test]
fn compacted_impersonation_starts_with_assistant_and_ends_with_user() {
    use crate::shared::api::contract::ApiRole;
    let chat = compacted_chat();
    let req = build_impersonation_request(
        &chat,
        chat.compaction_view(true),
        "Ты — пользователь".into(),
        "",
        SamplingConfig::default(),
        None,
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
    );
    assert_eq!(req.messages.first().unwrap().role, ApiRole::Assistant);
    assert_eq!(
        req.messages.last().unwrap().role,
        ApiRole::User,
        "a trailing assistant turn reads as a prefill — the model would continue it instead of \
         writing the next message"
    );
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

/// The user persona is resolved through the assistant profile's reference into
/// `config.impersonation_profiles`; every miss falls back to the default text.
#[test]
fn impersonation_system_resolves_through_the_profile_reference() {
    let (_d, mut orch) = bare_orch();
    let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
    let default_text = loc.t("prompt.impersonation.default");

    let imp = crate::shared::config::ImpersonationProfile::new("Юзер", "Ты — Владимир.");
    let imp_id = imp.id;
    orch.config.impersonation_profiles = vec![imp];

    let mut profile = Profile::new("P", "sys");
    // No reference — the default.
    assert_eq!(orch.impersonation_system(Some(&profile), loc), default_text);
    // A reference — the persona's message.
    profile.impersonation_profile_id = Some(imp_id);
    assert_eq!(
        orch.impersonation_system(Some(&profile), loc),
        "Ты — Владимир."
    );
    // A dangling reference (the persona was deleted) — the default, not an empty prompt.
    profile.impersonation_profile_id = Some(uuid::Uuid::new_v4());
    assert_eq!(orch.impersonation_system(Some(&profile), loc), default_text);
    // A blank persona message — also the default.
    profile.impersonation_profile_id = Some(imp_id);
    orch.config.impersonation_profiles[0].system_message = "   ".into();
    assert_eq!(orch.impersonation_system(Some(&profile), loc), default_text);
}

/// Impersonation on the shared engine records its exact usage beside the
/// estimate its reservation was priced from, under its own kind
/// (docs/research/title-impersonation-usage.md §3.2): the budget's
/// `Impersonation` ratio moves, the turn's does not.
#[tokio::test]
async fn impersonation_records_its_usage_under_its_own_kind() {
    use crate::shared::api::contract::TokenUsage;
    use crate::shared::session_budget::Shape;
    let (_d, mut orch, _rx, chat_id) = bare_with_chat(vec![
        Message::user("Привет!"),
        Message::assistant("Здравствуйте. Чем помочь?"),
    ]);
    orch.active_id = Some(chat_id);
    orch.engines.backend = Some(Arc::new(MockBackend::scripted(vec![
        ChatChunk::Text("Расскажи о себе.".into()),
        ChatChunk::Usage(TokenUsage {
            prompt_tokens: 50_000,
            completion_tokens: 4,
            reasoning_tokens: 0,
            prefill: None,
        }),
        ChatChunk::Finished(FinishReason::Stop),
    ])) as Arc<dyn EngineBackend>);
    let budget = orch.session_budget();
    assert_eq!(budget.density(Shape::Impersonation), 1.0);

    orch.handle_impersonate(String::new());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while budget.density(Shape::Impersonation) == 1.0 && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        budget.density(Shape::Impersonation) > 1.0,
        "50 000 exact over a small estimate: {}",
        budget.density(Shape::Impersonation)
    );
    assert_eq!(
        budget.density(Shape::Turn),
        1.0,
        "impersonation's record is impersonation's"
    );
}

/// Impersonation's prompt is the session's largest — the whole conversation,
/// roles swapped, under its own system, processed cold — and on the shared
/// engine its timing rides the task's landing to the slow-prefill rule
/// (docs/research/oneshot-samples.md §3.1): `ImpersonationFinished` first,
/// the note after it.
#[tokio::test]
async fn impersonations_landing_offers_its_sample_on_the_shared_engine() {
    use crate::shared::api::contract::{Prefill, TokenUsage};
    let (_d, mut orch, mut rx, chat_id) = bare_with_chat(vec![
        Message::user("Привет!"),
        Message::assistant("Здравствуйте. Чем помочь?"),
    ]);
    orch.active_id = Some(chat_id);
    orch.config.engine.mode = crate::shared::config::ServerMode::External;
    orch.engines.backend = Some(Arc::new(MockBackend::scripted(vec![
        ChatChunk::Text("Расскажи о себе.".into()),
        ChatChunk::Usage(TokenUsage {
            prompt_tokens: 10_642,
            completion_tokens: 4,
            reasoning_tokens: 0,
            prefill: Some(Prefill {
                tokens: 10_642,
                ms: 280_000,
            }),
        }),
        ChatChunk::Finished(FinishReason::Stop),
    ])) as Arc<dyn EngineBackend>);
    let (done_tx, mut done_rx) = tokio::sync::mpsc::unbounded_channel();
    orch.imp_done_tx = done_tx;
    while rx.try_recv().is_ok() {}

    orch.handle_impersonate(String::new());
    let done = tokio::time::timeout(std::time::Duration::from_secs(5), done_rx.recv())
        .await
        .expect("the task landed")
        .unwrap();
    assert_eq!(
        done.prefill.map(|p| p.tokens),
        Some(10_642),
        "the shared engine's sample rides the landing"
    );
    orch.handle_imp_done(done);
    let events: Vec<AppEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    let finished = events
        .iter()
        .position(|e| matches!(e, AppEvent::ImpersonationFinished { .. }))
        .expect("the landing");
    let note = events
        .iter()
        .position(|e| matches!(e, AppEvent::Notice(t) if t.contains("-b 256 -ub 256")))
        .expect("the note");
    assert!(finished < note, "the landing first, the note after it");
}

/// The sample is kept under the record's own condition — a budget, which is
/// the shared engine (fork F3): a separate impersonation server is another
/// server, with its own batch and session, and the task keeps nothing there;
/// and a stream that ended before its usage chunk carries nothing anywhere.
#[tokio::test]
async fn a_separate_server_or_a_cut_stream_lands_no_sample() {
    use crate::shared::api::contract::{Prefill, TokenUsage};
    let (_d, mut orch, _rx, _chat_id) = bare_with_chat(vec![]);
    let request = || crate::shared::api::ChatRequest {
        continue_final: false,
        system: Some("persona".into()),
        messages: vec![crate::shared::api::ApiMessage::user("hello")],
        sampling: crate::entities::sampling::SamplingConfig::default(),
        tools: Vec::new(),
    };
    let timed = || {
        Arc::new(MockBackend::scripted(vec![
            ChatChunk::Text("reply".into()),
            ChatChunk::Usage(TokenUsage {
                prompt_tokens: 400,
                completion_tokens: 1,
                reasoning_tokens: 0,
                prefill: Some(Prefill {
                    tokens: 400,
                    ms: 10_000,
                }),
            }),
            ChatChunk::Finished(FinishReason::Stop),
        ])) as Arc<dyn EngineBackend>
    };
    let (evt_tx, _evt_rx) = tokio::sync::mpsc::unbounded_channel();

    // A separate server: no budget, no sample.
    let (done_tx, mut done_rx) = tokio::sync::mpsc::unbounded_channel();
    super::super::impersonation::spawn_impersonation(
        timed(),
        request(),
        Uuid::new_v4(),
        tokio_util::sync::CancellationToken::new(),
        orch.ui_locale(),
        evt_tx.clone(),
        done_tx,
        None,
    );
    let done = tokio::time::timeout(std::time::Duration::from_secs(5), done_rx.recv())
        .await
        .expect("landed")
        .unwrap();
    assert_eq!(done.reason, FinishReason::Stop);
    assert!(
        done.prefill.is_none(),
        "another server: no sample for this one's rule"
    );

    // The shared engine, a stream cut before its usage: nothing to carry.
    let cut = Arc::new(MockBackend::scripted(vec![
        ChatChunk::Text("reply".into()),
        ChatChunk::Finished(FinishReason::Stop),
    ])) as Arc<dyn EngineBackend>;
    let (done_tx, mut done_rx) = tokio::sync::mpsc::unbounded_channel();
    super::super::impersonation::spawn_impersonation(
        cut,
        request(),
        Uuid::new_v4(),
        tokio_util::sync::CancellationToken::new(),
        orch.ui_locale(),
        evt_tx.clone(),
        done_tx,
        Some(orch.session_budget()),
    );
    let done = tokio::time::timeout(std::time::Duration::from_secs(5), done_rx.recv())
        .await
        .expect("landed")
        .unwrap();
    assert!(done.prefill.is_none(), "no usage chunk, no sample");

    // The shared engine with the chunk: the sample.
    let (done_tx, mut done_rx) = tokio::sync::mpsc::unbounded_channel();
    super::super::impersonation::spawn_impersonation(
        timed(),
        request(),
        Uuid::new_v4(),
        tokio_util::sync::CancellationToken::new(),
        orch.ui_locale(),
        evt_tx,
        done_tx,
        Some(orch.session_budget()),
    );
    let done = tokio::time::timeout(std::time::Duration::from_secs(5), done_rx.recv())
        .await
        .expect("landed")
        .unwrap();
    assert_eq!(done.prefill.map(|p| p.tokens), Some(400));
}

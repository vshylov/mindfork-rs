//! Orchestrator live smokes of the OpenRouter mode — the real supervisor, the
//! real gateway and a real local server, because what is under test is the
//! **switch** between them (docs/research/openrouter-mode.md §7, the go/no-go
//! of stage 1). Part of the [`super`] module.

use super::*;
use crate::app::supervisor::LlamaSupervisor;
use crate::shared::config::{CloudSettings, ExternalSettings, ServerMode};

/// The orchestrator on the **production** supervisor, so that the engine each
/// mode gets is the one a real run gets.
fn spawn_on_the_real_supervisor(config: AppConfig) -> OrchHandle {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, evt_rx) = unbounded_channel();
    let handle = tokio::spawn(run(OrchestratorDeps {
        cmd_rx,
        evt_tx,
        storage,
        config,
        supervisor: Arc::new(LlamaSupervisor::default()),
        default_language: crate::shared::i18n::Lang::default(),
        extra_tools: Vec::new(),
    }));
    (dir, cmd_tx, evt_rx, handle)
}

/// What the engine said about itself: the sampling fields its catalogue
/// published (`None` — it published none, which is every llama.cpp) and the
/// slots it reported (`None` — it has none to report, which is every gateway).
type Facts = (Option<Arc<[String]>>, Option<u32>);

/// Waits until the chat engine is **ready** and its facts are the ones
/// `settled` describes, and returns them — or fails with the last that were
/// said.
///
/// The facts arrive as events, and more than once: the slot count is first
/// announced as gone and then as answered, and the engine is asked again on
/// every flip of its status. So the first event of a kind is not the answer,
/// and a fact said before the status changed belongs to a question that has
/// been asked again since — it is forgotten here as the orchestrator forgets
/// it. What is returned was said **inside this wait, after the engine became
/// ready**: the new engine's word, not a leftover.
///
/// Readiness is part of it for a reason of its own: the gateway's mode is
/// `Connecting` until its key is judged, and a message sent before that is
/// refused.
async fn engine_facts(
    rx: &mut UnboundedReceiver<AppEvent>,
    settled: impl Fn(&Facts) -> bool,
) -> Facts {
    let (mut ready, mut fields, mut slots) = (false, None, None);
    let wait = async {
        loop {
            match rx.recv().await.expect("the event stream") {
                AppEvent::ServerStatus(statuses) => {
                    ready = statuses.chat == ServerStatus::Ready;
                    (fields, slots) = (None, None);
                }
                AppEvent::EngineSamplingFields(f) => fields = Some(f),
                AppEvent::EngineSlots(s) => slots = Some(s),
                _ => continue,
            }
            if let (true, Some(f), Some(s)) = (ready, &fields, &slots) {
                let facts = (f.clone(), *s);
                if settled(&facts) {
                    return facts;
                }
            }
        }
    };
    let facts = tokio::time::timeout(std::time::Duration::from_secs(90), wait).await;
    facts.unwrap_or_else(|_| {
        panic!("the engine never settled: ready={ready} fields={fields:?} slots={slots:?}")
    })
}

/// One turn, to its end — or the failure it ended with, in the app's words: a
/// turn the engine refuses has no `Finished` to wait for.
async fn turn(
    cmd_tx: &UnboundedSender<AppCommand>,
    rx: &mut UnboundedReceiver<AppEvent>,
    text: &str,
) -> String {
    cmd_tx.send(AppCommand::SendMessage(text.into())).unwrap();
    let mut out = String::new();
    let reply = async {
        loop {
            match rx.recv().await.expect("the event stream") {
                AppEvent::Chunk { text, .. } => out.push_str(&text),
                AppEvent::Error(said) => panic!("the turn failed: {said}"),
                AppEvent::Finished { .. } => return,
                _ => {}
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(300), reply)
        .await
        .expect("the turn ends");
    out
}

/// A llama.cpp's facts: no list of its own, and a slot count.
fn of_a_local_server(facts: &Facts) -> bool {
    facts.0.is_none() && facts.1.is_some()
}

/// A gateway's facts: a list, and no slots.
fn of_the_gateway(facts: &Facts) -> bool {
    facts.0.is_some() && facts.1.is_none()
}

/// The scenario the mode exists for: a chat on a **local server**, a switch to
/// the **gateway**, and back — inside one session, with nothing typed twice.
///
/// Three things are asserted at each switch, and each is a claim of
/// docs/research/openrouter-mode.md:
///
/// - the engine's facts are the new engine's (§4.1): a llama.cpp publishes no
///   sampling list and reports its slots, the gateway publishes a list — in its
///   own words, none of llama.cpp's — and has no slots. Before the facts were
///   asked again on a settings edit, the list of the previous engine survived;
/// - the turn is answered by the engine the mode names, which the message
///   records: only a reply through the gateway names who served it and what it
///   cost;
/// - both sections are still in the saved settings afterwards (§2.1): the
///   switch took a mode row, not three fields.
#[tokio::test]
#[ignore = "requires MINDFORK_ENGINE_URL (a llama.cpp, no key) and MINDFORK_OPENROUTER_KEY; MINDFORK_OPENROUTER_MODEL names the gateway's model"]
async fn a_chat_moves_to_the_gateway_and_back_in_one_session_live() {
    let Ok(local) = std::env::var("MINDFORK_ENGINE_URL") else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    if std::env::var("MINDFORK_OPENROUTER_KEY").map_or(true, |k| k.trim().is_empty()) {
        eprintln!("skip: MINDFORK_OPENROUTER_KEY not set");
        return;
    }
    let slug = std::env::var("MINDFORK_OPENROUTER_MODEL")
        .unwrap_or_else(|_| "anthropic/claude-haiku-4.5".into());

    let mut on_the_local_server = no_auto_cfg();
    on_the_local_server.engine.mode = ServerMode::External;
    on_the_local_server.engine.external = ExternalSettings {
        url: Some(local.clone()),
        ..Default::default()
    };
    on_the_local_server.engine.openrouter = CloudSettings {
        model_name: Some(slug.clone()),
        // The key is read from the environment by name, as a user without a
        // stored key would have it.
        api_key_env: Some("MINDFORK_OPENROUTER_KEY".into()),
        ..Default::default()
    };
    // A small local model thinks for as long as it is allowed to.
    on_the_local_server.default_sampling.thinking = Some(false);
    let mut through_the_gateway = on_the_local_server.clone();
    through_the_gateway.engine.mode = ServerMode::OpenRouter;

    let (dir, cmd_tx, mut rx, handle) = spawn_on_the_real_supervisor(on_the_local_server.clone());
    let root = dir.path().to_path_buf();
    let chat_id = match wait_for(&mut rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .expect("a chat")
    {
        AppEvent::ChatActivated { id, .. } => id,
        _ => unreachable!(),
    };
    let question = "Answer with one word: what is the capital of France?";

    // 1. On the local server.
    let (fields, slots) = engine_facts(&mut rx, of_a_local_server).await;
    eprintln!("local: fields={fields:?} slots={slots:?}");
    let first = turn(&cmd_tx, &mut rx, question).await;
    eprintln!("local: {first:?}");
    assert!(first.to_lowercase().contains("paris"), "{first:?}");

    // 2. To the gateway: one field of the config changes.
    cmd_tx
        .send(AppCommand::UpdateConfig(Box::new(
            through_the_gateway.clone(),
        )))
        .unwrap();
    let (fields, slots) = engine_facts(&mut rx, of_the_gateway).await;
    eprintln!("gateway: fields={fields:?} slots={slots:?}");
    let fields = fields.expect("the gateway publishes what the model takes");
    assert!(fields.iter().any(|f| f == "temperature"), "{fields:?}");
    assert!(
        !fields
            .iter()
            .any(|f| f == "repeat_penalty" || f.starts_with("mirostat")),
        "the list is in the gateway's words: {fields:?}"
    );
    let second = turn(&cmd_tx, &mut rx, question).await;
    eprintln!("gateway: {second:?}");
    assert!(second.to_lowercase().contains("paris"), "{second:?}");

    // 3. And back.
    cmd_tx
        .send(AppCommand::UpdateConfig(Box::new(on_the_local_server)))
        .unwrap();
    // The gateway's list does not survive the switch: the wait ends on an
    // event that says there is none.
    let (fields, slots) = engine_facts(&mut rx, of_a_local_server).await;
    eprintln!("local again: fields={fields:?} slots={slots:?}");
    let third = turn(&cmd_tx, &mut rx, question).await;
    eprintln!("local again: {third:?}");
    assert!(third.to_lowercase().contains("paris"), "{third:?}");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // What each reply says about where it came from.
    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let chat = reopened.json().load_chat(chat_id).unwrap().unwrap();
    let replies: Vec<_> = chat
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::Assistant)
        .filter_map(|m| m.metadata.clone())
        .collect();
    for meta in &replies {
        eprintln!(
            "reply: mode={:?} model={:?} provider={:?} cost_nanos={:?}",
            meta.mode, meta.model, meta.provider, meta.cost_nanos
        );
    }
    assert_eq!(
        replies.iter().map(|m| m.mode).collect::<Vec<_>>(),
        [
            ServerMode::External,
            ServerMode::OpenRouter,
            ServerMode::External
        ]
    );
    assert_eq!(replies[1].model.as_deref(), Some(slug.as_str()));
    assert!(replies[1].provider.is_some(), "the gateway says who served");
    assert!(replies[1].cost_nanos.is_some_and(|c| c > 0));
    assert_eq!(
        (&replies[0].provider, replies[0].cost_nanos),
        (&None, None),
        "a local server says neither"
    );
    assert_eq!((&replies[2].provider, replies[2].cost_nanos), (&None, None));

    // Both sections are where they were: nothing was typed twice.
    let saved = reopened.json().load_config().unwrap();
    assert_eq!(saved.engine.mode, ServerMode::External);
    assert_eq!(saved.engine.external.url.as_deref(), Some(local.as_str()));
    assert_eq!(
        saved.engine.openrouter.model_name.as_deref(),
        Some(slug.as_str())
    );
}

/// `/continue` in the mode follows the route table of spec §6.4 with no
/// catalogue to wait for: the mode **is** the statement that this is a gateway.
/// Both arms, because a gate that always refuses and one that always allows each
/// pass one of them: a model the table lets continue resumes its reply without
/// restarting it, and one it does not is refused with the gateway's note.
#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY; MINDFORK_OPENROUTER_CONTINUES_MODEL and MINDFORK_OPENROUTER_RESTARTS_MODEL name the two arms"]
async fn continue_in_the_mode_follows_the_route_table_live() {
    if std::env::var("MINDFORK_OPENROUTER_KEY").map_or(true, |k| k.trim().is_empty()) {
        eprintln!("skip: MINDFORK_OPENROUTER_KEY not set");
        return;
    }
    let arm = |var: &str, measured: &str| {
        std::env::var(var)
            .ok()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| measured.to_string())
    };
    for (slug, continues) in [
        (
            arm(
                "MINDFORK_OPENROUTER_CONTINUES_MODEL",
                "anthropic/claude-haiku-4.5",
            ),
            true,
        ),
        (
            arm(
                "MINDFORK_OPENROUTER_RESTARTS_MODEL",
                "google/gemma-4-31b-it",
            ),
            false,
        ),
    ] {
        let mut cfg = no_auto_cfg();
        cfg.engine.mode = ServerMode::OpenRouter;
        cfg.engine.openrouter = CloudSettings {
            model_name: Some(slug.clone()),
            api_key_env: Some("MINDFORK_OPENROUTER_KEY".into()),
            ..Default::default()
        };
        // The fixture of the smoke this one shares its body with.
        cfg.default_sampling.max_tokens = Some(4);
        cfg.default_sampling.reasoning_budget = Some(0);
        let (_dir, cmd_tx, mut rx, handle) = spawn_on_the_real_supervisor(cfg);
        engine_facts(&mut rx, of_the_gateway).await;
        super::live::cut_then_continue(&cmd_tx, &mut rx, &slug, continues).await;
        cmd_tx.send(AppCommand::Quit).unwrap();
        handle.await.unwrap();
    }
}

/// A key the gateway refuses is what the **status** says, before any message is
/// sent — and a message sent anyway is refused with those words rather than
/// with a `401` from the first request.
#[tokio::test]
#[ignore = "requires network access to openrouter.ai; no key is needed — the key under test is a wrong one"]
async fn a_refused_key_is_the_chats_status_live() {
    // A variable name this smoke owns, holding a key of the gateway's shape
    // that is nobody's. Not a variable that merely exists: whatever it held
    // would be sent to the gateway as a key.
    const VAR: &str = "MINDFORK_TEST_OPENROUTER_WRONG_KEY";
    // SAFETY: the live smokes run on one thread (`--test-threads=1`), and the
    // variable is this smoke's own name.
    unsafe { std::env::set_var(VAR, format!("sk-or-v1-{}", "0".repeat(64))) };
    let mut cfg = no_auto_cfg();
    cfg.engine.mode = ServerMode::OpenRouter;
    cfg.engine.openrouter = CloudSettings {
        model_name: Some("anthropic/claude-haiku-4.5".into()),
        api_key_env: Some(VAR.into()),
        ..Default::default()
    };
    let (_dir, cmd_tx, mut rx, handle) = spawn_on_the_real_supervisor(cfg);
    let refused = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        wait_for(&mut rx, |e| {
            matches!(e, AppEvent::ServerStatus(s)
                if matches!(s.chat, ServerStatus::Disconnected(_)))
        }),
    )
    .await
    .expect("the key check answers")
    .expect("the event stream");
    let AppEvent::ServerStatus(statuses) = refused else {
        unreachable!()
    };
    let ServerStatus::Disconnected(said) = statuses.chat else {
        unreachable!()
    };
    eprintln!("status: {said}");
    assert!(said.contains("OpenRouter"), "{said}");

    cmd_tx.send(AppCommand::SendMessage("hi".into())).unwrap();
    let note = wait_for(&mut rx, |e| matches!(e, AppEvent::Error(_)))
        .await
        .expect("the refusal");
    let AppEvent::Error(text) = note else {
        unreachable!()
    };
    eprintln!("a message sent anyway: {text}");
    assert!(text.contains("OpenRouter"), "{text}");

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

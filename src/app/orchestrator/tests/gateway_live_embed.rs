//! Live smokes of the embedder through the OpenRouter mode — stage 2 of
//! docs/research/openrouter-mode.md (§7): the index a local model built answers
//! a query the gateway embedded, a long text reaches a cloud in parts, and the
//! model-change guard is armed by a switch made in the session. Part of the
//! [`super`] module.
//!
//! On the production supervisor and through the tools the model calls, so that
//! what is embedded goes the whole road: the convention, the guard, the batch
//! cap, the retry, the client. No chat engine is needed — the tools are called
//! as the agentic loop calls them, without a model to decide that they are.

use super::*;
use crate::app::orchestrator::engines::Server;
use crate::app::supervisor::LlamaSupervisor;
use crate::shared::api::EmbedRole;
use crate::shared::config::{CloudSettings, EmbedSettings, ExternalSettings, ServerMode};
use crate::shared::embed_identity::CANARY_TEXT;

const PARIS: &str = "The capital of France is Paris, the country's largest city.";
const CORPUS: [&str; 5] = [
    PARIS,
    "The cat sat on the windowsill and watched the rain.",
    "Rust's borrow checker rejects a second mutable reference to the same value.",
    "Sourdough needs a starter that has been fed for at least a week.",
    "The moon's gravity is about a sixth of the earth's.",
];

fn variable(name: &str) -> Option<String> {
    let value = std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    if value.is_none() {
        eprintln!("skip: {name} not set");
    }
    value
}

/// The embedder's section in the gateway's mode, its key read by name.
fn through_the_gateway(model: &str) -> CloudSettings {
    CloudSettings {
        model_name: Some(model.to_string()),
        api_key_env: Some("MINDFORK_OPENROUTER_KEY".into()),
        ..Default::default()
    }
}

/// A bare orchestrator on the **production** supervisor, with the channel the
/// embedder's status arrives on: no loop runs here, so the smoke reads it.
fn on_the_real_supervisor(
    embed: EmbedSettings,
) -> (
    tempfile::TempDir,
    Orchestrator,
    UnboundedReceiver<AppEvent>,
    UnboundedReceiver<ServerStatus>,
) {
    let (dir, mut orch, events) = bare_orch_rx();
    let (embed_tx, embed_rx) = unbounded_channel();
    orch.engines = EngineManager::new(
        Arc::new(LlamaSupervisor::default()),
        unbounded_channel().0,
        unbounded_channel().0,
        embed_tx,
    );
    orch.config.embed = embed;
    (dir, orch, events, embed_rx)
}

/// Waits for the embedder to be ready, as the loop would learn it.
async fn ready(orch: &mut Orchestrator, statuses: &mut UnboundedReceiver<ServerStatus>) {
    let wait = async {
        while orch.engines.status_of(Server::Embed) != &ServerStatus::Ready {
            let status = statuses.recv().await.expect("the embedder's status");
            orch.engines.set_embed_status(status);
        }
    };
    if tokio::time::timeout(std::time::Duration::from_secs(60), wait)
        .await
        .is_err()
    {
        panic!(
            "the embedder never became ready: {:?}",
            orch.engines.status_of(Server::Embed)
        );
    }
}

/// Applies an edit of the embedding settings the way the settings screen's
/// edit is applied — the road that used to install the embedder bare.
async fn switch(
    orch: &mut Orchestrator,
    statuses: &mut UnboundedReceiver<ServerStatus>,
    embed: EmbedSettings,
) {
    let mut edited = orch.config.clone();
    edited.embed = embed;
    orch.handle_update_config(edited);
    orch.flush_restarts();
    ready(orch, statuses).await;
}

/// Calls a tool as the agentic loop does, with the embedder the orchestrator
/// would hand it now.
async fn call(orch: &Orchestrator, profile: Uuid, tool: &str, args: serde_json::Value) -> String {
    let ctx = super::attachments::turn_ctx(orch, profile, Uuid::new_v4(), Vec::new());
    let tool = orch.registry.get(tool).expect("the tool is registered");
    match tool.invoke(&ctx, args).await {
        Ok(outcome) => outcome.result,
        Err(refused) => format!("refused: {refused}"),
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (norm(a) * norm(b))
}

/// What the guard and the notice have said since the last look.
fn notices(events: &mut UnboundedReceiver<AppEvent>) -> Vec<String> {
    let mut said = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let AppEvent::Error(text) | AppEvent::Notice(text) = event {
            said.push(text);
        }
    }
    said
}

/// The go/no-go of the stage, and the switching the whole track is about.
///
/// An index is built on a **local** `bge-m3`; the embedder is moved to the
/// gateway's `baai/bge-m3` by an edit made in the session; a query embedded by
/// the gateway finds the passage the local model indexed — and **no reindex is
/// offered**: no notice, the same generation, the knowledge base not stale.
/// Then the text added through the gateway is found as well, by either.
///
/// The control arm is the guard itself. The same switch to a model of the same
/// width that is *not* the same model has to be caught — which it can only be
/// if the embedder the edit installed is guarded (research §9, A1): the claim
/// "no reindex was offered" is worth nothing from a guard that was not there.
#[tokio::test]
#[ignore = "requires MINDFORK_EMBED_URL (a local bge-m3) and MINDFORK_OPENROUTER_KEY"]
async fn an_index_built_locally_answers_a_query_embedded_by_the_gateway_live() {
    let (Some(local), Some(_)) = (
        variable("MINDFORK_EMBED_URL"),
        variable("MINDFORK_OPENROUTER_KEY"),
    ) else {
        return;
    };
    let same = std::env::var("MINDFORK_OPENROUTER_EMBED_MODEL")
        .ok()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| "baai/bge-m3".into());
    let other = "intfloat/multilingual-e5-large";
    let on = |mode: ServerMode, model: &str| EmbedSettings {
        mode,
        external: ExternalSettings {
            url: Some(local.clone()),
            ..Default::default()
        },
        openrouter: through_the_gateway(model),
        ..Default::default()
    };

    let (_dir, mut orch, mut events, mut statuses) =
        on_the_real_supervisor(on(ServerMode::External, &same));
    orch.apply_embed_settings();
    ready(&mut orch, &mut statuses).await;
    let profile = Uuid::new_v4();
    let db = orch.storage.clone();

    // 1. The index, on the local model.
    for (n, text) in CORPUS.iter().enumerate() {
        let added = call(
            &orch,
            profile,
            "rag_add",
            serde_json::json!({ "text": text, "source": format!("fact-{n}") }),
        )
        .await;
        assert!(!added.starts_with("refused"), "{added}");
    }
    assert_eq!(db.db().rag_count(profile).unwrap(), CORPUS.len());
    let generation = db.db().embed_generation().unwrap();
    let locally = orch
        .engines
        .embedder()
        .embed(vec![CANARY_TEXT.into()], EmbedRole::Passage)
        .await
        .unwrap()
        .remove(0);
    notices(&mut events);

    // 2. The embedder moves to the gateway, by an edit.
    switch(&mut orch, &mut statuses, on(ServerMode::OpenRouter, &same)).await;
    let question = "Which city is the capital of France?";
    let found = call(
        &orch,
        profile,
        "rag_search",
        serde_json::json!({ "query": question }),
    )
    .await;
    eprintln!("[{same}] rag_search: {found}");
    assert!(found.contains("Paris"), "{found}");
    let asked = orch
        .engines
        .embedder()
        .embed(vec![question.into()], EmbedRole::Query)
        .await
        .unwrap()
        .remove(0);
    let hits = db.db().rag_search(profile, &asked, 3).unwrap();
    eprintln!(
        "hits: {:?}",
        hits.iter()
            .map(|h| (h.distance, &h.chunk_text[..24]))
            .collect::<Vec<_>>()
    );
    assert_eq!(hits[0].chunk_text, PARIS);

    let through = orch
        .engines
        .embedder()
        .embed(vec![CANARY_TEXT.into()], EmbedRole::Passage)
        .await
        .unwrap()
        .remove(0);
    let said = notices(&mut events);
    eprintln!(
        "the canary, local against the gateway: {:.6}; said: {said:?}",
        cosine(&locally, &through)
    );
    assert_eq!(said, Vec::<String>::new(), "no reindex is offered");
    assert_eq!(db.db().embed_generation().unwrap(), generation);
    assert!(!db.db().rag_is_stale(profile).unwrap());

    // 3. What the gateway embeds is found too — by the gateway, and, back on
    //    the local model, by that.
    let lisbon = "The capital of Portugal is Lisbon, on the mouth of the Tagus.";
    let added = call(
        &orch,
        profile,
        "rag_add",
        serde_json::json!({ "text": lisbon, "source": "fact-gateway" }),
    )
    .await;
    assert!(!added.starts_with("refused"), "{added}");
    let ask = serde_json::json!({ "query": "Which city is the capital of Portugal?" });
    let found = call(&orch, profile, "rag_search", ask.clone()).await;
    assert!(found.contains("Lisbon"), "{found}");
    switch(&mut orch, &mut statuses, on(ServerMode::External, &same)).await;
    let found = call(&orch, profile, "rag_search", ask).await;
    assert!(found.contains("Lisbon"), "back on the local model: {found}");
    assert_eq!(notices(&mut events), Vec::<String>::new());
    assert_eq!(db.db().embed_generation().unwrap(), generation);

    // 4. The control: a model of the same width that is another model.
    switch(&mut orch, &mut statuses, on(ServerMode::OpenRouter, other)).await;
    let refused = call(
        &orch,
        profile,
        "rag_search",
        serde_json::json!({ "query": question }),
    )
    .await;
    let said = notices(&mut events);
    eprintln!("[{other}] rag_search: {refused}\nsaid: {said:?}");
    assert_eq!(said.len(), 1, "the change is said, once: {said:?}");
    assert!(db.db().embed_generation().unwrap() > generation);
    assert!(db.db().rag_is_stale(profile).unwrap());
    assert!(
        !refused.contains("Paris"),
        "and the stale index is not searched: {refused}"
    );
}

/// A text of more chunks than a cloud takes in one request is added whole.
///
/// `rag_add` embeds every chunk of its text at once. Measured, Gemini's
/// embedding models refuse the hundred-and-first input — through the gateway
/// and on Google's own endpoint alike — so any text of more than a hundred
/// chunks was refused outright. The control arm is that refusal, asked of the
/// bare client with the very chunks the tool was given.
async fn a_long_text_is_added(embed: EmbedSettings, bare: crate::shared::api::OpenAiClient) {
    let label = embed.active_model_name().unwrap_or_default();
    let (_dir, mut orch, _events, mut statuses) = on_the_real_supervisor(embed);
    // Small chunks, so that a text a person could read is a hundred of them.
    orch.config.rag.chunk_target_chars = 120;
    orch.apply_embed_settings();
    ready(&mut orch, &mut statuses).await;
    let profile = Uuid::new_v4();

    let needle = "The access code of the east gate is 48213, changed every Monday.";
    let mut paragraphs: Vec<String> = (0..130)
        .map(|n| {
            format!(
                "Entry {n} of the station log records a routine reading of instrument \
                 number {n}, taken at dawn and found within its usual range."
            )
        })
        .collect();
    paragraphs.insert(117, needle.to_string());
    let text = paragraphs.join("\n\n");
    let chunks = crate::features::tools::rag::chunk_text(
        &text,
        crate::features::tools::ToolParams::from_config(&orch.config).chunk_params,
    );
    eprintln!("[{label}] {} chunks", chunks.len());
    assert!(chunks.len() > 100, "the fixture is {} chunks", chunks.len());

    let refused = crate::shared::api::Embedder::embed(&bare, chunks.clone(), EmbedRole::Passage)
        .await
        .expect_err("the control: one request of them all is refused");
    eprintln!("[{label}] the bare client: {refused}");
    assert!(refused.to_string().contains("at most 100"), "{refused}");

    let added = call(
        &orch,
        profile,
        "rag_add",
        serde_json::json!({ "text": text, "source": "station-log" }),
    )
    .await;
    eprintln!("[{label}] rag_add: {added}");
    assert!(!added.starts_with("refused"), "{added}");
    assert_eq!(orch.storage.db().rag_count(profile).unwrap(), chunks.len());

    let found = call(
        &orch,
        profile,
        "rag_search",
        serde_json::json!({ "query": "What is the access code of the east gate?" }),
    )
    .await;
    assert!(found.contains("48213"), "{found}");
}

#[tokio::test]
#[ignore = "requires MINDFORK_OPENROUTER_KEY; MINDFORK_OPENROUTER_CAPPED_EMBED_MODEL names an embedding model that takes a hundred inputs"]
async fn a_long_text_is_added_through_the_gateway_live() {
    let Some(key) = variable("MINDFORK_OPENROUTER_KEY") else {
        return;
    };
    let model = std::env::var("MINDFORK_OPENROUTER_CAPPED_EMBED_MODEL")
        .ok()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| "google/gemini-embedding-2".into());
    let provider = crate::shared::config::CloudProvider::OpenRouter;
    let bare = crate::shared::api::OpenAiClient::new(provider.base_url())
        .with_api_key(Some(key))
        .with_model(Some(model.clone()))
        .for_openrouter(true);
    a_long_text_is_added(
        EmbedSettings {
            mode: ServerMode::OpenRouter,
            openrouter: through_the_gateway(&model),
            ..Default::default()
        },
        bare,
    )
    .await;
}

/// The same on a cloud of its own: the cap is every embedder's, and Google's
/// endpoint is where the number was measured.
#[tokio::test]
#[ignore = "requires MINDFORK_GEMINI_KEY"]
async fn a_long_text_is_added_through_gemini_live() {
    let Some(key) = variable("MINDFORK_GEMINI_KEY") else {
        return;
    };
    let model = "gemini-embedding-001";
    let provider = crate::shared::config::CloudProvider::Gemini;
    let bare = crate::shared::api::OpenAiClient::new(provider.base_url())
        .with_api_key(Some(key))
        .with_model(Some(model.to_string()));
    a_long_text_is_added(
        EmbedSettings {
            mode: ServerMode::Gemini,
            gemini: CloudSettings {
                model_name: Some(model.to_string()),
                api_key_env: Some("MINDFORK_GEMINI_KEY".into()),
                ..Default::default()
            },
            ..Default::default()
        },
        bare,
    )
    .await;
}

/// A key the gateway refuses is the embedder's status, on the road the
/// settings screen's edit takes.
#[tokio::test]
#[ignore = "requires network access to openrouter.ai; the key under test is a wrong one"]
async fn a_refused_key_is_the_embedders_status_live() {
    const VAR: &str = "MINDFORK_TEST_OPENROUTER_WRONG_EMBED_KEY";
    // SAFETY: the live smokes run on one thread (`--test-threads=1`), and the
    // variable is this smoke's own name.
    unsafe { std::env::set_var(VAR, format!("sk-or-v1-{}", "0".repeat(64))) };
    let (_dir, mut orch, _events, mut statuses) = on_the_real_supervisor(EmbedSettings {
        mode: ServerMode::OpenRouter,
        openrouter: CloudSettings {
            model_name: Some("baai/bge-m3".into()),
            api_key_env: Some(VAR.into()),
            ..Default::default()
        },
        ..Default::default()
    });
    orch.apply_embed_settings();
    assert_eq!(
        orch.engines.status_of(Server::Embed),
        &ServerStatus::Connecting
    );
    let said = tokio::time::timeout(std::time::Duration::from_secs(60), statuses.recv())
        .await
        .expect("the key check answers")
        .expect("the embedder's status");
    eprintln!("status: {said:?}");
    let ServerStatus::Disconnected(said) = said else {
        panic!("a refused key disconnects the slot: {said:?}")
    };
    assert!(said.contains("OpenRouter"), "{said}");
    assert!(said.contains("User not found"), "{said}");
}

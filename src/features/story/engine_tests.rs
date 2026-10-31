use std::sync::Arc;

use anyhow::Result;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::EngineAdapter;
use crate::entities::sampling::SamplingConfig;
use crate::features::story::port::tests::{prompt, schema};
use crate::features::story::port::{CallAddress, LlmPort, PortError, Role, prompt_sha256};
use crate::features::story::scene::tests::fresh_story;
use crate::shared::api::contract::{
    ChatChunk, ChatRequest, ChatStream, EngineBackend, FinishReason, Served, TokenUsage,
};
use crate::shared::api::mock::MockBackend;
use crate::shared::storage::story::{CallStatus, Mechanism, StoryDbError};

fn sampling() -> SamplingConfig {
    SamplingConfig {
        temperature: Some(0.8),
        thinking: Some(false),
        ..SamplingConfig::default()
    }
}

fn reply(text: &str) -> Vec<ChatChunk> {
    vec![
        ChatChunk::Thoughts("Mara would not budge.".to_owned()),
        ChatChunk::Text(text.to_owned()),
        ChatChunk::Usage(TokenUsage {
            prompt_tokens: 120,
            completion_tokens: 30,
            cached_tokens: Some(100),
            ..TokenUsage::default()
        }),
        ChatChunk::Served(Served {
            provider: Some("DeepInfra".to_owned()),
            cost_nanos: Some(1_500_000),
        }),
        ChatChunk::Finished(FinishReason::Stop),
    ]
}

fn adapter(
    backend: impl EngineBackend + 'static,
    log: crate::shared::storage::story::RunLogWriter,
) -> EngineAdapter {
    EngineAdapter::new(
        Arc::new(backend),
        log,
        "mock-model",
        sampling(),
        CancellationToken::new(),
    )
}

#[tokio::test]
async fn a_json_call_is_recorded_with_its_request_reply_and_cost() {
    let (story, log) = fresh_story().await;
    let port = adapter(
        MockBackend::sequence(vec![reply("{\"mood\": \"grim\"}")]),
        log,
    );
    let call = CallAddress {
        run: story.run.clone(),
        address: "ch1/sc1/r0/k0/round1/mara/propose".to_owned(),
    };
    let value = port
        .json(&call, Role::Character, &schema(), &prompt())
        .await
        .unwrap();
    assert_eq!(value, json!({"mood": "grim"}));

    let stored = story
        .db
        .call(&story.run, &call.address, 1)
        .await
        .unwrap()
        .unwrap();
    let recorded = &stored.call;
    assert_eq!(recorded.role, "character");
    assert_eq!(recorded.model, "mock-model");
    assert_eq!(recorded.mechanism, Mechanism::Fallback);
    assert_eq!(recorded.status, CallStatus::Ok);
    assert_eq!(
        recorded.prompt_sha256,
        prompt_sha256(&prompt(), Some(&schema()))
    );
    // The sampling as sent: an `f32` widens on its way into JSON.
    let temperature = recorded.params["temperature"].as_f64().unwrap();
    assert!((temperature - 0.8).abs() < 1e-6, "{temperature}");
    assert_eq!(recorded.params["thinking"], json!(false));
    let system = recorded.request["system"].as_str().unwrap();
    assert!(system.ends_with(&schema().to_json()), "{system}");
    assert_eq!(
        recorded.request["messages"],
        json!([{"role": "user", "content": "The gate closes."}])
    );
    let response = recorded.response.as_ref().unwrap();
    assert_eq!(response["text"], "{\"mood\": \"grim\"}");
    assert_eq!(response["thought_chars"], 21);
    assert_eq!(response["served_by"], "DeepInfra");
    assert_eq!(recorded.usage.input_tokens, Some(120));
    assert_eq!(recorded.usage.output_tokens, Some(30));
    assert_eq!(recorded.usage.cached_tokens, Some(100));
    assert_eq!(recorded.usage.cost_usd, Some(0.0015));
    assert!(recorded.latency_ms.is_some());

    // One live call per address: a second is refused by the call log.
    let again = port
        .json(&call, Role::Character, &schema(), &prompt())
        .await;
    assert!(
        matches!(
            again,
            Err(PortError::Store(StoryDbError::CallExists { .. }))
        ),
        "{again:?}"
    );
}

#[tokio::test]
async fn every_attempt_is_recorded_in_order() {
    let (story, log) = fresh_story().await;
    let cut = vec![
        ChatChunk::Text("{\"mo".to_owned()),
        ChatChunk::Finished(FinishReason::Length),
    ];
    let port = adapter(
        MockBackend::sequence(vec![
            cut,
            reply("{\"mood\": \"sly\"}"),
            reply("{\"mood\": \"calm\"}"),
        ]),
        log,
    );
    let call = CallAddress {
        run: story.run.clone(),
        address: "a".to_owned(),
    };
    assert!(
        port.json(&call, Role::Character, &schema(), &prompt())
            .await
            .is_ok()
    );
    let calls = story.db.calls(&story.run).await.unwrap();
    let seen: Vec<(u32, CallStatus, usize)> = calls
        .iter()
        .map(|c| {
            let turns = c.call.request["messages"].as_array().unwrap().len();
            (c.call.attempt, c.call.status, turns)
        })
        .collect();
    assert_eq!(
        seen,
        [
            (1, CallStatus::Cut, 1),
            (2, CallStatus::Ok, 1),
            (3, CallStatus::Ok, 3),
        ]
    );
    // A cut reports no usage of its own; nothing is invented for it.
    assert_eq!(calls[0].call.usage.input_tokens, None);
    assert_eq!(calls[0].call.usage.cost_usd, None);
}

#[tokio::test]
async fn how_a_stream_ends_decides_the_status() {
    let cases: Vec<(Vec<ChatChunk>, CallStatus, bool)> = vec![
        (
            vec![ChatChunk::Finished(FinishReason::Filtered)],
            CallStatus::Refused,
            false,
        ),
        (
            ChatChunk::failure("529 overloaded".to_owned(), true).to_vec(),
            CallStatus::Error,
            true,
        ),
        (
            vec![ChatChunk::Finished(FinishReason::Error)],
            CallStatus::Error,
            true,
        ),
        (
            vec![ChatChunk::Text("{".to_owned())],
            CallStatus::Error,
            true,
        ),
    ];
    for (chunks, status, transient) in cases {
        let (story, log) = fresh_story().await;
        let port = adapter(MockBackend::scripted(chunks.clone()), log);
        let call = CallAddress {
            run: story.run.clone(),
            address: "a".to_owned(),
        };
        let result = port
            .json(&call, Role::Character, &schema(), &prompt())
            .await;
        assert!(result.is_err(), "{chunks:?}");
        let first = story.db.call(&story.run, "a", 1).await.unwrap().unwrap();
        assert_eq!(first.call.status, status, "{chunks:?}");
        let response = first.call.response.unwrap();
        assert_eq!(response["transient"], transient, "{chunks:?}");
        let attempts = story.db.calls(&story.run).await.unwrap().len();
        assert_eq!(attempts, if transient { 2 } else { 1 }, "{chunks:?}");
    }
}

/// A backend whose requests cannot even be opened.
struct Unreachable;

#[async_trait::async_trait]
impl EngineBackend for Unreachable {
    async fn chat_stream(&self, _: ChatRequest, _: CancellationToken) -> Result<ChatStream> {
        anyhow::bail!("connection refused")
    }
}

#[tokio::test]
async fn a_request_that_cannot_be_opened_fails_the_call_in_its_own_words() {
    let (story, log) = fresh_story().await;
    let port = adapter(Unreachable, log);
    let call = CallAddress {
        run: story.run.clone(),
        address: "a".to_owned(),
    };
    match port.text(&call, Role::Author, &prompt()).await {
        Err(PortError::Failed { message, .. }) => assert_eq!(message, "connection refused"),
        other => panic!("{other:?}"),
    }
    let first = story.db.call(&story.run, "a", 1).await.unwrap().unwrap();
    assert_eq!(first.call.mechanism, Mechanism::Text);
    assert_eq!(first.call.role, "author");
    assert_eq!(first.call.response.unwrap()["error"], "connection refused");
}

#[tokio::test]
async fn a_cancelled_run_stops_at_its_call() {
    let (story, log) = fresh_story().await;
    let cancel = CancellationToken::new();
    cancel.cancel();
    let port = EngineAdapter::new(
        Arc::new(MockBackend::scripted(reply("{\"mood\": \"calm\"}"))),
        log,
        "mock-model",
        sampling(),
        cancel,
    );
    let call = CallAddress {
        run: story.run.clone(),
        address: "a".to_owned(),
    };
    let result = port
        .json(&call, Role::Character, &schema(), &prompt())
        .await;
    assert!(
        matches!(result, Err(PortError::Cancelled { .. })),
        "{result:?}"
    );
    let first = story.db.call(&story.run, "a", 1).await.unwrap().unwrap();
    assert_eq!(first.call.response.unwrap()["cancelled"], true);
    assert_eq!(story.db.calls(&story.run).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_text_call_sends_the_prompt_as_it_is_and_gives_the_log_back() {
    let (story, log) = fresh_story().await;
    let port = adapter(MockBackend::scripted(reply("The storm came in.")), log);
    let call = CallAddress {
        run: story.run.clone(),
        address: "ch1/draft".to_owned(),
    };
    let text = port.text(&call, Role::Author, &prompt()).await.unwrap();
    assert_eq!(text, "The storm came in.");
    let first = story
        .db
        .call(&story.run, "ch1/draft", 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.call.request["system"], "You are Mara.");
    assert_eq!(first.call.prompt_sha256, prompt_sha256(&prompt(), None));
    // The capability comes back whole: the run log can be written again.
    let mut log = port.into_log();
    log.finish_step(
        &story.run,
        "ch1/draft",
        crate::shared::storage::story::StepStatus::Done,
    )
    .await
    .unwrap();
}

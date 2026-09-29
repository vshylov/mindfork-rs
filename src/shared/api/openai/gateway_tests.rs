//! The OpenRouter gateway's dialect, end to end over a real socket
//! (docs/research/openrouter-mode.md §4.1): what a client built with
//! [`OpenAiClient::for_openrouter`] asks, in which words, and what it never asks.
//!
//! A file of its own rather than more of `client.rs`' test module: the stub here
//! answers by **path** and hands back the whole request — headers included —
//! which is what these tests are about and none of the others needed.

use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use super::client::{ATTRIBUTION_REFERER, ATTRIBUTION_TITLE, KeyVerdict};
use super::{OpenAiClient, wire};
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::shared::api::contract::{
    ApiImage, ApiMessage, ApiToolCall, ChatChunk, ChatRequest, EngineBackend, Served, VisionSupport,
};

/// One request as the stub received it.
#[derive(Debug, Clone)]
struct Seen {
    /// `GET /v1/model/x/y`.
    line: String,
    /// The header block as sent, one `name: value` per line.
    headers: String,
    body: String,
}

impl Seen {
    fn path(&self) -> &str {
        self.line.split_whitespace().nth(1).unwrap_or_default()
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
        })
    }

    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {:?}", self.body))
    }
}

/// `(path, status line, content type, body)`.
type Route = (&'static str, &'static str, &'static str, &'static str);

const JSON: &str = "application/json";
const SSE: &str = "text/event-stream";

/// Serves `connections` requests, each answered by the route whose path it
/// asked for (`404` otherwise), and hands back what was asked, in order.
///
/// The deadline lives in the thread: a client that never makes the request a
/// test expects must leave a stub that ends by itself, so the test fails on
/// what it saw instead of hanging (docs/lessons.md §2).
fn stub(
    connections: usize,
    routes: &'static [Route],
) -> (String, std::thread::JoinHandle<Vec<Seen>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let handle = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut seen = Vec::new();
        while seen.len() < connections {
            let mut sock = match listener.accept() {
                Ok((sock, _)) => sock,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                }
                Err(_) => return seen,
            };
            sock.set_nonblocking(false).unwrap();
            let mut raw = Vec::new();
            let mut chunk = [0u8; 8192];
            let request = loop {
                let n = match sock.read(&mut chunk) {
                    Ok(0) | Err(_) => break None,
                    Ok(n) => n,
                };
                raw.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&raw).to_string();
                let Some((head, body)) = text.split_once("\r\n\r\n") else {
                    continue;
                };
                let want: usize = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse().ok())?
                    })
                    .unwrap_or(0);
                if body.len() >= want {
                    let (line, headers) = head.split_once("\r\n").unwrap_or((head, ""));
                    break Some(Seen {
                        line: line.to_string(),
                        headers: headers.replace("\r\n", "\n"),
                        body: body.to_string(),
                    });
                }
            };
            let Some(request) = request else {
                return seen;
            };
            let (status, kind, body) = routes
                .iter()
                .find(|(path, ..)| *path == request.path())
                .map(|(_, status, kind, body)| (*status, *kind, *body))
                .unwrap_or((
                    "404 Not Found",
                    JSON,
                    r#"{"error":{"message":"Not Found","code":404}}"#,
                ));
            // `Connection: close`, or the client pools the socket and sends the
            // next request down one this stub has already dropped.
            let _ = sock.write_all(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
            seen.push(request);
        }
        seen
    });
    (format!("http://{addr}/v1"), handle)
}

/// A reply of one chunk, then the terminator.
const ONE_CHUNK: &str = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";

/// The entries below are the gateway's own, measured 2026-09-29 and cut to the
/// keys this client reads.
const HAIKU: &str = r#"{"data":{"id":"anthropic/claude-haiku-4.5","context_length":200000,
    "architecture":{"input_modalities":["text","image","file"],"output_modalities":["text"]},
    "supported_parameters":["include_reasoning","max_tokens","reasoning","temperature","tools","top_k","top_p"],
    "reasoning":{"mandatory":false}}}"#;
const GEMINI: &str = r#"{"data":{"id":"google/gemini-3.5-flash","context_length":1048576,
    "architecture":{"input_modalities":["text","image","video","file","audio"],"output_modalities":["text"]},
    "supported_parameters":["reasoning","include_reasoning","max_tokens","temperature","top_p","seed","tools","reasoning_effort"],
    "reasoning":{"mandatory":true,"default_enabled":true,"supported_efforts":["high","medium","low","minimal"],"default_effort":"medium"}}}"#;
const R1: &str = r#"{"data":{"id":"deepseek/deepseek-r1","context_length":64000,
    "architecture":{"input_modalities":["text"],"output_modalities":["text"]},
    "supported_parameters":["max_tokens","reasoning","repetition_penalty","temperature","top_k","top_p"],
    "reasoning":{"mandatory":true}}}"#;

fn gateway(url: String, model: &str) -> OpenAiClient {
    OpenAiClient::new(url)
        .with_api_key(Some("k".into()))
        .with_model(Some(model.into()))
        .for_openrouter(true)
}

fn turn(sampling: SamplingConfig) -> ChatRequest {
    ChatRequest {
        continue_final: false,
        system: None,
        messages: vec![ApiMessage::user("hi")],
        sampling,
        tools: Vec::new(),
    }
}

/// A turn that asks for reasoning to be off, the way the title, the compaction
/// roll and impersonation do.
fn muted() -> ChatRequest {
    turn(SamplingConfig {
        reasoning_effort: Some(ReasoningEffort::None),
        reasoning_budget: Some(0),
        thinking: Some(false),
        ..Default::default()
    })
}

async fn run(client: &OpenAiClient, req: ChatRequest) -> Vec<ChatChunk> {
    let mut stream = client
        .chat_stream(req, CancellationToken::new())
        .await
        .expect("a stream");
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        out.push(chunk);
    }
    out
}

/// The one body a test is about: the chat request's.
fn chat_body(seen: &[Seen]) -> serde_json::Value {
    seen.iter()
        .find(|s| s.path() == "/v1/chat/completions")
        .unwrap_or_else(|| panic!("no chat request among {seen:?}"))
        .json()
}

/// Fork F5: with the switch on, **every** request to the gateway names the
/// application — the catalogue's and the key's as well as the chat's — and with
/// it off none does. A client that is not the gateway's never does, whatever it
/// is pointed at.
#[tokio::test]
async fn a_gateway_request_names_the_application_and_no_other_request_does() {
    const ROUTES: &[Route] = &[
        (
            "/v1/model/anthropic/claude-haiku-4.5",
            "200 OK",
            JSON,
            HAIKU,
        ),
        ("/v1/chat/completions", "200 OK", SSE, ONE_CHUNK),
        ("/v1/key", "200 OK", JSON, r#"{"data":{"limit":null}}"#),
    ];
    let named = |seen: &[Seen]| -> Vec<(Option<String>, Option<String>)> {
        seen.iter()
            .map(|s| {
                (
                    s.header("http-referer").map(str::to_string),
                    s.header("x-openrouter-title").map(str::to_string),
                )
            })
            .collect()
    };

    let (url, server) = stub(3, ROUTES);
    let client = gateway(url, "anthropic/claude-haiku-4.5");
    assert_eq!(client.check_key().await, KeyVerdict::Accepted);
    // A thinking turn, so the catalogue is asked too.
    run(
        &client,
        turn(SamplingConfig {
            thinking: Some(true),
            ..Default::default()
        }),
    )
    .await;
    let seen = server.join().unwrap();
    assert_eq!(seen.len(), 3, "{seen:?}");
    for pair in named(&seen) {
        assert_eq!(
            pair,
            (
                Some(ATTRIBUTION_REFERER.to_string()),
                Some(ATTRIBUTION_TITLE.to_string())
            )
        );
    }
    assert!(
        seen.iter()
            .all(|s| s.header("authorization") == Some("Bearer k")),
        "the key travels with all three: {seen:?}"
    );

    // Switched off: the same three requests, and not a word about the app.
    let (url, server) = stub(3, ROUTES);
    let silent = OpenAiClient::new(url)
        .with_api_key(Some("k".into()))
        .with_model(Some("anthropic/claude-haiku-4.5".into()))
        .for_openrouter(false);
    silent.check_key().await;
    run(
        &silent,
        turn(SamplingConfig {
            thinking: Some(true),
            ..Default::default()
        }),
    )
    .await;
    let seen = server.join().unwrap();
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert!(named(&seen).iter().all(|p| *p == (None, None)), "{seen:?}");

    // Not the gateway's client: `external` pointed at anything at all.
    let (url, server) = stub(1, ROUTES);
    run(
        &OpenAiClient::new(url).with_api_key(Some("k".into())),
        turn(Default::default()),
    )
    .await;
    assert!(
        named(&server.join().unwrap())
            .iter()
            .all(|p| *p == (None, None))
    );
}

/// D1 and the rest of §3.5: the knob the gateway reads under another name
/// travels under that name, what is llama.cpp's own does not travel at all, and
/// what the gateway reads stays. The same sampling through a plain client is
/// the control: it still says everything, in llama.cpp's words.
#[tokio::test]
async fn the_body_is_written_in_the_gateways_words() {
    let sampling = SamplingConfig {
        temperature: Some(0.7),
        top_k: Some(40),
        top_p: Some(0.9),
        min_p: Some(0.05),
        max_tokens: Some(512),
        seed: Some(7),
        frequency_penalty: Some(0.1),
        presence_penalty: Some(0.2),
        repeat_penalty: Some(1.15),
        repeat_last_n: Some(64),
        dynatemp_range: Some(0.5),
        dynatemp_exponent: Some(1.0),
        typical_p: Some(0.9),
        top_n_sigma: Some(1.0),
        adaptive_target: Some(0.5),
        adaptive_decay: Some(0.9),
        mirostat: Some(2),
        mirostat_tau: Some(5.0),
        mirostat_eta: Some(0.1),
        dry_multiplier: Some(0.8),
        dry_base: Some(1.75),
        dry_allowed_length: Some(2),
        dry_penalty_last_n: Some(64),
        dry_sequence_breakers: Some(vec!["\n".into()]),
        xtc_probability: Some(0.5),
        xtc_threshold: Some(0.1),
        samplers: Some(vec!["top_k".into()]),
        ..Default::default()
    };
    const ROUTES: &[Route] = &[("/v1/chat/completions", "200 OK", SSE, ONE_CHUNK)];

    let (url, server) = stub(1, ROUTES);
    run(
        &gateway(url, "meta-llama/llama-3.3-70b-instruct"),
        turn(sampling.clone()),
    )
    .await;
    let body = chat_body(&server.join().unwrap());
    let mut keys: Vec<&str> = body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "frequency_penalty",
            "max_tokens",
            "messages",
            "min_p",
            "model",
            "presence_penalty",
            "repetition_penalty",
            "seed",
            "stream",
            "stream_options",
            "temperature",
            "top_k",
            "top_p",
        ],
        "{body}"
    );
    // An `f32` is written as the shortest text that reads back to it, so the
    // comparison is made in hundredths.
    let hundredths = |v: &serde_json::Value| (v.as_f64().unwrap() * 100.0).round() as i64;
    assert_eq!(hundredths(&body["repetition_penalty"]), 115, "{body}");

    let (url, server) = stub(1, ROUTES);
    run(&OpenAiClient::new(url), turn(sampling)).await;
    let plain = chat_body(&server.join().unwrap());
    assert_eq!(hundredths(&plain["repeat_penalty"]), 115, "{plain}");
    assert!(plain.get("repetition_penalty").is_none(), "{plain}");
    for llama in ["mirostat", "dry_multiplier", "samplers", "typical_p"] {
        assert!(
            plain.get(llama).is_some(),
            "{llama} still reaches llama.cpp"
        );
    }
}

/// §4.1, measured: a model that must reason answers a request to stop with a
/// `400`, and the lowest effort it lists with a `200` — so that is what a muted
/// turn asks for, in **one** request. The stub would answer the refusal with a
/// `404`, which no test here could mistake for a reply.
#[tokio::test]
async fn a_muted_turn_asks_a_model_that_must_reason_for_its_lowest_effort() {
    const ROUTES: &[Route] = &[
        ("/v1/model/google/gemini-3.5-flash", "200 OK", JSON, GEMINI),
        ("/v1/chat/completions", "200 OK", SSE, ONE_CHUNK),
    ];
    let (url, server) = stub(2, ROUTES);
    let client = gateway(url, "google/gemini-3.5-flash");
    let chunks = run(&client, muted()).await;
    assert!(
        chunks.contains(&ChatChunk::Text("hi".into())),
        "the turn answered: {chunks:?}"
    );
    let seen = server.join().unwrap();
    assert_eq!(
        seen.iter().map(Seen::path).collect::<Vec<_>>(),
        ["/v1/model/google/gemini-3.5-flash", "/v1/chat/completions"],
        "one question, one request"
    );
    let body = chat_body(&seen);
    assert_eq!(body["reasoning_effort"], "minimal", "{body}");
    assert!(body.get("reasoning").is_none(), "{body}");
}

/// The two neighbours of the case above. A model that must reason and lists no
/// efforts has nothing lower to be asked for, and is asked nothing — not the
/// `"none"` it would refuse. A model that may stop is asked to, as it always was.
#[tokio::test]
async fn the_request_to_stop_reasoning_follows_what_the_catalogue_says() {
    const ROUTES: &[Route] = &[
        ("/v1/model/deepseek/deepseek-r1", "200 OK", JSON, R1),
        (
            "/v1/model/anthropic/claude-haiku-4.5",
            "200 OK",
            JSON,
            HAIKU,
        ),
        ("/v1/chat/completions", "200 OK", SSE, ONE_CHUNK),
    ];
    let (url, server) = stub(2, ROUTES);
    run(&gateway(url, "deepseek/deepseek-r1"), muted()).await;
    let r1 = chat_body(&server.join().unwrap());
    assert!(r1.get("reasoning_effort").is_none(), "{r1}");
    assert!(r1.get("reasoning").is_none(), "{r1}");

    let (url, server) = stub(2, ROUTES);
    run(&gateway(url, "anthropic/claude-haiku-4.5"), muted()).await;
    let haiku = chat_body(&server.join().unwrap());
    assert_eq!(haiku["reasoning_effort"], "none", "{haiku}");

    // An effort somebody chose is nobody's to replace, on any model.
    let (url, server) = stub(1, ROUTES);
    run(
        &gateway(url, "deepseek/deepseek-r1"),
        turn(SamplingConfig {
            reasoning_effort: Some(ReasoningEffort::High),
            ..Default::default()
        }),
    )
    .await;
    let chosen = chat_body(&server.join().unwrap());
    assert_eq!(chosen["reasoning_effort"], "high", "{chosen}");
}

/// What the gateway is asked, and what it is never asked: one model's entry, by
/// its slug as written — a `:variant` included, since the gateway resolves it —
/// and neither `/props` nor the whole catalogue, which `external` has to try.
#[tokio::test]
async fn the_gateway_is_asked_about_one_model_and_never_for_props() {
    const ROUTES: &[Route] = &[(
        "/v1/model/anthropic/claude-haiku-4.5:nitro",
        "200 OK",
        JSON,
        HAIKU,
    )];
    let (url, server) = stub(1, ROUTES);
    let client = gateway(url, "anthropic/claude-haiku-4.5:nitro");

    assert_eq!(client.vision().await, VisionSupport::Supported);
    let caps = client.model_capabilities().await.expect("an entry");
    assert_eq!(caps.context_length, Some(200_000));
    assert!(
        caps.sampling_fields
            .as_deref()
            .is_some_and(|f| f.iter().any(|p| p == "top_k"))
    );
    // Questions only a llama.cpp can answer are not put to the network at all.
    assert_eq!(client.context_budget().await, None);
    assert_eq!(client.parallel_slots().await, None);
    assert_eq!(client.model_id().await, None);

    let seen = server.join().unwrap();
    assert_eq!(
        seen.iter().map(Seen::path).collect::<Vec<_>>(),
        ["/v1/model/anthropic/claude-haiku-4.5:nitro"],
        "one request answers every question asked above"
    );
}

/// A model the gateway does not know is its answer, and is kept: the entry is
/// not asked for again, and nothing is claimed about the model.
#[tokio::test]
async fn a_model_the_gateway_does_not_know_is_asked_about_once() {
    let (url, server) = stub(1, &[]);
    let client = gateway(url, "nope/not-a-model");
    assert_eq!(client.vision().await, VisionSupport::Unknown);
    assert!(client.model_capabilities().await.is_none());
    assert_eq!(server.join().unwrap().len(), 1);
}

/// `external` re-homes a tool's images once the catalogue has answered — that
/// is how it knows it is speaking to a gateway. This client was told, so it does
/// not wait: here the catalogue is down, and the images move all the same.
#[tokio::test]
async fn a_tools_image_is_re_homed_without_waiting_for_the_catalogue() {
    const ROUTES: &[Route] = &[
        (
            "/v1/model/anthropic/claude-haiku-4.5",
            "503 Service Unavailable",
            JSON,
            "{}",
        ),
        ("/v1/chat/completions", "200 OK", SSE, ONE_CHUNK),
    ];
    let round = ChatRequest {
        continue_final: false,
        system: None,
        messages: vec![
            ApiMessage::user("what is on the screen?"),
            ApiMessage::assistant_tool_calls(
                "",
                vec![ApiToolCall {
                    id: "call-1".into(),
                    name: "take_screenshot".into(),
                    arguments: "{}".into(),
                    thought_signature: None,
                }],
            ),
            ApiMessage::tool("call-1", "Screenshot taken.").with_images(vec![ApiImage {
                mime: "image/png".into(),
                data: std::sync::Arc::from("QUJD"),
                label: Some("screen.png".into()),
            }]),
        ],
        sampling: Default::default(),
        tools: Vec::new(),
    };
    assert!(wire::carries_tool_images(&round.messages));

    let (url, server) = stub(1, ROUTES);
    run(&gateway(url, "anthropic/claude-haiku-4.5"), round).await;
    let seen = server.join().unwrap();
    let body = chat_body(&seen);
    let messages = body["messages"].as_array().unwrap();
    let tool = messages.iter().find(|m| m["role"] == "tool").unwrap();
    assert_eq!(tool["content"], "Screenshot taken.", "text only: {tool}");
    let last = messages.last().unwrap();
    assert_eq!(last["role"], "user");
    assert!(
        last["content"]
            .as_array()
            .is_some_and(|parts| parts.iter().any(|p| p["type"] == "image_url")),
        "the image follows in a user message: {last}"
    );
}

/// Fork F11: the routed provider and the gateway's own meter arrive as one
/// chunk beside the usage — and a server that reports neither sends none.
#[tokio::test]
async fn who_served_and_for_how_much_rides_beside_the_usage() {
    const REPLY: &str = concat!(
        "data: {\"provider\":\"Amazon Bedrock\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Paris\"},\"finish_reason\":null}]}\n\n",
        "data: {\"provider\":\"Amazon Bedrock\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"provider\":\"Amazon Bedrock\",\"choices\":[],\"usage\":{\"prompt_tokens\":17,\"completion_tokens\":4,\"cost\":0.000037}}\n\n",
        "data: [DONE]\n\n",
    );
    const ROUTES: &[Route] = &[("/v1/chat/completions", "200 OK", SSE, REPLY)];
    let (url, server) = stub(1, ROUTES);
    let chunks = run(
        &gateway(url, "anthropic/claude-haiku-4.5"),
        turn(Default::default()),
    )
    .await;
    let _ = server.join();
    let served: Vec<&Served> = chunks
        .iter()
        .filter_map(|c| match c {
            ChatChunk::Served(s) => Some(s),
            _ => None,
        })
        .collect();
    assert_eq!(
        served,
        [&Served {
            provider: Some("Amazon Bedrock".into()),
            cost_nanos: Some(37_000),
        }],
        "{chunks:?}"
    );

    const PLAIN: &[Route] = &[(
        "/v1/chat/completions",
        "200 OK",
        SSE,
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":17,\"completion_tokens\":4}}\n\ndata: [DONE]\n\n",
    )];
    let (url, server) = stub(1, PLAIN);
    let chunks = run(&OpenAiClient::new(url), turn(Default::default())).await;
    let _ = server.join();
    assert!(
        !chunks.iter().any(|c| matches!(c, ChatChunk::Served(_))),
        "a llama.cpp says neither: {chunks:?}"
    );
}

/// Fork F6, the four verdicts. The distinction that matters is the third one:
/// an outage is not a judgment of the key, and must not read as one.
#[tokio::test]
async fn a_key_is_accepted_refused_or_not_judged_at_all() {
    const OK: &[Route] = &[("/v1/key", "200 OK", JSON, r#"{"data":{"limit":null}}"#)];
    const REFUSED: &[Route] = &[(
        "/v1/key",
        "401 Unauthorized",
        JSON,
        r#"{"error":{"message":"User not found.","code":401}}"#,
    )];
    const DOWN: &[Route] = &[(
        "/v1/key",
        "503 Service Unavailable",
        JSON,
        r#"{"error":{"message":"try later","code":503}}"#,
    )];

    let (url, server) = stub(1, OK);
    assert_eq!(gateway(url, "m").check_key().await, KeyVerdict::Accepted);
    let _ = server.join();

    let (url, server) = stub(1, REFUSED);
    match gateway(url, "m").check_key().await {
        // The gateway's sentence, without the envelope it came in.
        KeyVerdict::Refused(said) => assert_eq!(said, "User not found."),
        other => panic!("a 401 refuses the key: {other:?}"),
    }
    let _ = server.join();

    // A refusal that is not an envelope is said by its status.
    const BARE: &[Route] = &[("/v1/key", "403 Forbidden", "text/html", "<html>no</html>")];
    let (url, server) = stub(1, BARE);
    assert_eq!(
        gateway(url, "m").check_key().await,
        KeyVerdict::Refused("403 Forbidden".into())
    );
    let _ = server.join();

    let (url, server) = stub(1, DOWN);
    match gateway(url, "m").check_key().await {
        KeyVerdict::Unjudged(said) => assert!(said.contains("try later"), "{said}"),
        other => panic!("an outage judges nothing: {other:?}"),
    }
    let _ = server.join();

    // Nobody listening: a port bound, learned and dropped.
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}/v1", l.local_addr().unwrap())
    };
    assert!(
        matches!(
            gateway(dead, "m").check_key().await,
            KeyVerdict::NoAnswer(_)
        ),
        "no answer is not a refusal"
    );
}

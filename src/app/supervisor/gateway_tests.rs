//! The OpenRouter gateway in the supervisor: the one cloud that is asked about
//! its key before it is called ready (docs/research/openrouter-mode.md §3.3,
//! fork F6), and the provider-wide switch every slot's client is built with
//! (fork F5).
//!
//! Over a real socket, like the client's own tests: what is asserted is what
//! left the machine.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio_util::sync::CancellationToken;

use super::{
    EmbedSettings, EngineSettings, ImpersonationEngineSettings, ImpersonationMode, LlamaSupervisor,
    Locale, Monitor, OpenAiClient, OpenRouterSettings, ServerMode, ServerStatus, ServerSupervisor,
    spawn_key_check_every,
};
use crate::shared::api::EmbedRole;
use crate::shared::config::CloudSettings;
use crate::shared::i18n::{self, Lang};

fn en() -> &'static Locale {
    i18n::locale(Lang::En)
}

/// What the stub does with one connection.
#[derive(Clone, Copy)]
enum Step {
    /// Answers with this status line and body.
    Answer(&'static str, &'static str),
    /// Takes the connection and closes it without a word — no answer at all, as
    /// far as the client can tell.
    Hang,
    /// Holds the connection this long, then answers `200`.
    Late(Duration),
}

const ACCEPTED: Step = Step::Answer("200 OK", r#"{"data":{"limit":null,"usage":0.5}}"#);
const REFUSED: Step = Step::Answer(
    "401 Unauthorized",
    r#"{"error":{"message":"User not found.","code":401}}"#,
);
const OUTAGE: Step = Step::Answer(
    "503 Service Unavailable",
    r#"{"error":{"message":"try later","code":503}}"#,
);

/// Serves the script, one step per connection, and hands back each request's
/// head. The deadline lives in the thread, so a client that never connects
/// leaves a stub that ends by itself (docs/lessons.md §2).
fn stub(script: &'static [Step]) -> (String, std::thread::JoinHandle<Vec<String>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let handle = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();
        for step in script {
            let mut sock = loop {
                match listener.accept() {
                    Ok((sock, _)) => break sock,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return seen,
                }
            };
            sock.set_nonblocking(false).unwrap();
            let mut buf = [0u8; 4096];
            let n = sock.read(&mut buf).unwrap_or(0);
            seen.push(String::from_utf8_lossy(&buf[..n]).to_string());
            let (status, body) = match step {
                Step::Hang => continue,
                Step::Late(wait) => {
                    std::thread::sleep(*wait);
                    ("200 OK", "{}")
                }
                Step::Answer(status, body) => (*status, *body),
            };
            let _ = sock.write_all(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        }
        seen
    });
    (format!("http://{addr}/v1"), handle)
}

/// A gateway section pointed at the stub.
fn section(url: &str) -> CloudSettings {
    CloudSettings {
        model_name: Some("anthropic/claude-haiku-4.5".into()),
        url: Some(url.to_string()),
        ..Default::default()
    }
}

fn chat(url: &str) -> EngineSettings {
    EngineSettings {
        mode: ServerMode::OpenRouter,
        openrouter: section(url),
        ..Default::default()
    }
}

/// The next status the check publishes, or `None` when it stays silent for as
/// long as any of these tests gives it.
async fn next(rx: &mut UnboundedReceiver<ServerStatus>) -> Option<ServerStatus> {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .ok()
        .flatten()
}

/// Off the runtime thread: a bare `join()` would block the executor the check
/// runs on.
async fn requests(seen: std::thread::JoinHandle<Vec<String>>) -> Vec<String> {
    tokio::task::spawn_blocking(move || seen.join().unwrap())
        .await
        .unwrap()
}

/// The contract that sets this cloud apart from the other four: it starts
/// `Connecting`, asks `GET /key` with the key, and is `Ready` on the answer —
/// one request, and none after it.
#[tokio::test]
async fn the_gateway_is_asked_about_the_key_before_it_is_called_ready() {
    const SCRIPT: &[Step] = &[ACCEPTED];
    let (url, seen) = stub(SCRIPT);
    let (tx, mut rx) = unbounded_channel();
    let setup = LlamaSupervisor::default().apply_chat(
        &chat(&url),
        Some("sk-or-stored"),
        CancellationToken::new(),
        tx,
        en(),
    );
    assert_eq!(setup.status, ServerStatus::Connecting);
    assert!(setup.backend.is_some());
    assert!(setup.handle.is_none(), "a cloud has no child process");

    assert_eq!(next(&mut rx).await, Some(ServerStatus::Ready));
    let asked = requests(seen).await;
    assert_eq!(asked.len(), 1);
    let head = asked[0].to_ascii_lowercase();
    assert!(head.starts_with("get /v1/key "), "{head}");
    assert!(
        head.contains("authorization: bearer sk-or-stored"),
        "{head}"
    );
    assert!(head.contains("x-openrouter-title: mindfork"), "{head}");

    // The task has ended: a cloud is not watched.
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(300), rx.recv()).await,
        Ok(None),
        "the check's sender is dropped once the key was judged"
    );
}

/// A key the gateway refuses is the **slot's status**, in the gateway's own
/// words and with its name — not the error of the first message sent.
#[tokio::test]
async fn a_refused_key_is_the_slots_status_in_the_gateways_words() {
    const SCRIPT: &[Step] = &[REFUSED];
    let (url, seen) = stub(SCRIPT);
    let (tx, mut rx) = unbounded_channel();
    LlamaSupervisor::default().apply_chat(
        &chat(&url),
        Some("sk-or-wrong"),
        CancellationToken::new(),
        tx,
        en(),
    );
    match next(&mut rx).await {
        Some(ServerStatus::Disconnected(said)) => {
            assert!(said.contains("OpenRouter"), "{said}");
            assert!(said.contains("User not found"), "{said}");
        }
        other => panic!("a refused key disconnects the slot: {other:?}"),
    }
    let _ = requests(seen).await;
}

/// An outage judges nothing. Reading it as a refusal would block a key that
/// works on the strength of a gateway that was briefly down.
#[tokio::test]
async fn an_outage_does_not_judge_the_key() {
    const SCRIPT: &[Step] = &[OUTAGE];
    let (url, seen) = stub(SCRIPT);
    let (tx, mut rx) = unbounded_channel();
    LlamaSupervisor::default().apply_chat(
        &chat(&url),
        Some("sk-or-stored"),
        CancellationToken::new(),
        tx,
        en(),
    );
    assert_eq!(next(&mut rx).await, Some(ServerStatus::Ready));
    let _ = requests(seen).await;
}

/// No answer at all is the one case that repeats — and it says so **once**,
/// however many attempts go unanswered, then reports the verdict when
/// something does answer.
#[tokio::test]
async fn no_answer_is_said_once_and_asked_again_until_something_answers() {
    const SCRIPT: &[Step] = &[Step::Hang, Step::Hang, ACCEPTED];
    let (url, seen) = stub(SCRIPT);
    let (tx, mut rx) = unbounded_channel();
    let client = Arc::new(
        OpenAiClient::new(url)
            .with_api_key(Some("sk-or-stored".into()))
            .with_model(Some("m".into()))
            .for_openrouter(true),
    );
    spawn_key_check_every(
        client,
        Monitor {
            cancel: CancellationToken::new(),
            status_tx: tx,
            loc: en(),
        },
        Duration::from_millis(20),
    );
    assert!(
        matches!(next(&mut rx).await, Some(ServerStatus::Disconnected(_))),
        "the slot says why it is waiting"
    );
    assert_eq!(
        next(&mut rx).await,
        Some(ServerStatus::Ready),
        "the second unanswered attempt is not announced again"
    );
    assert_eq!(requests(seen).await.len(), 3);
}

/// A check that was overtaken by another apply says nothing: its late verdict
/// would be about a key the slot no longer uses.
#[tokio::test]
async fn a_check_that_was_overtaken_says_nothing() {
    const SCRIPT: &[Step] = &[Step::Late(Duration::from_millis(300))];
    let (url, seen) = stub(SCRIPT);
    let (tx, mut rx) = unbounded_channel();
    let cancel = CancellationToken::new();
    LlamaSupervisor::default().apply_chat(
        &chat(&url),
        Some("sk-or-stored"),
        cancel.clone(),
        tx,
        en(),
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancel.cancel();
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(900), rx.recv()).await,
        Ok(None),
        "no status, and the sender is gone"
    );
    let _ = requests(seen).await;
}

/// A section that cannot work is refused where it stands, as on every cloud —
/// and nothing is asked of the network on its behalf.
#[tokio::test]
async fn a_section_without_a_model_or_a_key_asks_nothing() {
    const SCRIPT: &[Step] = &[ACCEPTED];
    let (url, seen) = stub(SCRIPT);
    let no_model = EngineSettings {
        mode: ServerMode::OpenRouter,
        openrouter: CloudSettings {
            model_name: None,
            ..section(&url)
        },
        ..Default::default()
    };
    let (tx, _rx) = unbounded_channel();
    let setup = LlamaSupervisor::default().apply_chat(
        &no_model,
        Some("sk-or-stored"),
        CancellationToken::new(),
        tx,
        en(),
    );
    assert!(
        matches!(setup.status, ServerStatus::Disconnected(_)),
        "{:?}",
        setup.status
    );
    assert!(setup.backend.is_none());

    let (tx, _rx) = unbounded_channel();
    let setup = LlamaSupervisor::default().apply_chat(
        &chat(&url),
        None,
        CancellationToken::new(),
        tx,
        en(),
    );
    assert!(
        matches!(setup.status, ServerStatus::Disconnected(_)),
        "{:?}",
        setup.status
    );

    // The stub is still waiting for its one connection; give a stray request
    // the time to arrive, then see that none did.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!seen.is_finished(), "nobody connected");
    // Let the stub's thread end: make the connection it is waiting for.
    let _ = std::net::TcpStream::connect(url.trim_start_matches("http://").trim_end_matches("/v1"));
    let _ = requests(seen).await;
}

/// Fork F5 at the seam where the switch meets the clients: what the supervisor
/// was last told is what the next client is built with.
#[tokio::test]
async fn the_attribution_switch_reaches_the_client_the_supervisor_builds() {
    const SCRIPT: &[Step] = &[ACCEPTED];
    let head_of = |asked: Vec<String>| asked[0].to_ascii_lowercase();

    let (url, seen) = stub(SCRIPT);
    let supervisor = LlamaSupervisor::default();
    supervisor.set_gateway(&OpenRouterSettings { attribution: false });
    let (tx, mut rx) = unbounded_channel();
    supervisor.apply_chat(
        &chat(&url),
        Some("sk-or-stored"),
        CancellationToken::new(),
        tx,
        en(),
    );
    assert_eq!(next(&mut rx).await, Some(ServerStatus::Ready));
    let head = head_of(requests(seen).await);
    assert!(!head.contains("http-referer"), "{head}");
    assert!(!head.contains("x-openrouter-title"), "{head}");

    let (url, seen) = stub(SCRIPT);
    supervisor.set_gateway(&OpenRouterSettings { attribution: true });
    let (tx, mut rx) = unbounded_channel();
    supervisor.apply_chat(
        &chat(&url),
        Some("sk-or-stored"),
        CancellationToken::new(),
        tx,
        en(),
    );
    assert_eq!(next(&mut rx).await, Some(ServerStatus::Ready));
    let head = head_of(requests(seen).await);
    assert!(head.contains("http-referer: https://mindfork.io"), "{head}");
}

/// Impersonation is served by the same arm, on the same key — a model of its
/// own is all it adds.
#[tokio::test]
async fn impersonation_through_the_gateway_is_checked_the_same_way() {
    const SCRIPT: &[Step] = &[ACCEPTED];
    let (url, seen) = stub(SCRIPT);
    let settings = ImpersonationEngineSettings {
        mode: ImpersonationMode::OpenRouter,
        openrouter: section(&url),
        ..Default::default()
    };
    let (tx, mut rx) = unbounded_channel();
    let setup = LlamaSupervisor::default().apply_impersonation(
        &settings,
        Some("sk-or-stored"),
        CancellationToken::new(),
        tx,
        en(),
    );
    assert_eq!(setup.status, ServerStatus::Connecting);
    assert!(setup.backend.is_some());
    assert_eq!(next(&mut rx).await, Some(ServerStatus::Ready));
    let _ = requests(seen).await;
}

/// An embeddings answer of `n` vectors, as a body the stub can serve.
fn vectors(n: usize) -> &'static str {
    let data: Vec<String> = (0..n)
        .map(|i| format!(r#"{{"embedding":[{i}.0,1.0],"index":{i}}}"#))
        .collect();
    Box::leak(format!(r#"{{"data":[{}]}}"#, data.join(",")).into_boxed_str())
}

fn script(steps: Vec<Step>) -> &'static [Step] {
    Box::leak(steps.into_boxed_slice())
}

/// An embedder's settings in `mode`, its section pointed at the stub.
fn embedder(mode: ServerMode, url: &str) -> EmbedSettings {
    let section = CloudSettings {
        model_name: Some("baai/bge-m3".into()),
        ..section(url)
    };
    EmbedSettings {
        mode,
        openai: section.clone(),
        openrouter: section,
        external: crate::shared::config::ExternalSettings {
            url: Some(url.to_string()),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// `n` short texts, each a token the request's body can be counted by.
fn texts(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("t{i}")).collect()
}

/// How many texts a request carried.
fn inputs(request: &str) -> usize {
    request.matches("\"t").count()
}

/// The gateway's embedder is asked about its key as its chat engine is: the
/// slot is `Connecting` until the gateway answered, and the request that
/// follows carries the model, the key and the app's name.
#[tokio::test]
async fn the_gateways_embedder_is_asked_about_its_key_too() {
    let (url, seen) = stub(script(vec![ACCEPTED, Step::Answer("200 OK", vectors(1))]));
    let (tx, mut rx) = unbounded_channel();
    let setup = LlamaSupervisor::default().apply_embed(
        &embedder(ServerMode::OpenRouter, &url),
        Some("sk-or-stored"),
        CancellationToken::new(),
        tx,
        en(),
    );
    assert_eq!(setup.status, ServerStatus::Connecting);
    assert_eq!(next(&mut rx).await, Some(ServerStatus::Ready));

    let answered = setup
        .embedder
        .embed(vec!["hello".into()], EmbedRole::Passage)
        .await
        .expect("an embedding");
    assert_eq!(answered, vec![vec![0.0, 1.0]]);
    let seen = requests(seen).await;
    let heads: Vec<String> = seen.iter().map(|r| r.to_ascii_lowercase()).collect();
    assert!(heads[0].starts_with("get /v1/key "), "{}", heads[0]);
    assert!(heads[1].starts_with("post /v1/embeddings "), "{}", heads[1]);
    for head in &heads {
        assert!(
            head.contains("authorization: bearer sk-or-stored"),
            "{head}"
        );
        assert!(head.contains("x-openrouter-title: mindfork"), "{head}");
    }
    assert!(seen[1].contains(r#""model":"baai/bge-m3""#), "{}", seen[1]);
}

/// A key the gateway refuses is the embedder's status, in the gateway's words,
/// when the engine is applied — and not first heard of as a `401` inside the
/// result of a tool that embeds.
#[tokio::test]
async fn a_refused_key_is_the_embedders_status() {
    let (url, seen) = stub(script(vec![REFUSED]));
    let (tx, mut rx) = unbounded_channel();
    let setup = LlamaSupervisor::default().apply_embed(
        &embedder(ServerMode::OpenRouter, &url),
        Some("sk-or-wrong"),
        CancellationToken::new(),
        tx,
        en(),
    );
    assert_eq!(setup.status, ServerStatus::Connecting);
    assert_eq!(
        next(&mut rx).await,
        Some(ServerStatus::Disconnected(
            "OpenRouter refused the API key: User not found.".into()
        ))
    );
    let _ = requests(seen).await;
}

/// No other cloud is asked anything before it is used: its embedder is ready
/// at once, and the first request it makes is the first embedding.
#[tokio::test]
async fn another_clouds_embedder_is_ready_at_once_and_asks_nothing() {
    let (url, seen) = stub(script(vec![Step::Answer("200 OK", vectors(1))]));
    let (tx, mut rx) = unbounded_channel();
    let setup = LlamaSupervisor::default().apply_embed(
        &embedder(ServerMode::OpenAi, &url),
        Some("sk-stored"),
        CancellationToken::new(),
        tx,
        en(),
    );
    assert_eq!(setup.status, ServerStatus::Ready);
    setup
        .embedder
        .embed(texts(1), EmbedRole::Passage)
        .await
        .expect("an embedding");
    let seen = requests(seen).await;
    assert_eq!(seen.len(), 1);
    assert!(seen[0].starts_with("POST /v1/embeddings "), "{}", seen[0]);
    assert!(
        !seen[0].to_ascii_lowercase().contains("x-openrouter-title"),
        "the app's name is the gateway's to be told: {}",
        seen[0]
    );
    assert!(rx.try_recv().is_err(), "and no status follows");
}

/// A cloud sheds load as a matter of course, and its embedder asks again: a
/// rate limit followed by an answer is an answer.
#[tokio::test]
async fn a_clouds_embedder_asks_again_after_a_rate_limit() {
    const SLOW_DOWN: Step = Step::Answer(
        "429 Too Many Requests",
        r#"{"error":{"message":"The engine is currently overloaded","code":429}}"#,
    );
    for mode in [ServerMode::OpenAi, ServerMode::OpenRouter] {
        let mut steps = vec![SLOW_DOWN, Step::Answer("200 OK", vectors(2))];
        if mode == ServerMode::OpenRouter {
            steps.insert(0, ACCEPTED);
        }
        let asked = steps.len();
        let (url, seen) = stub(script(steps));
        let (tx, mut rx) = unbounded_channel();
        let setup = LlamaSupervisor::default().apply_embed(
            &embedder(mode, &url),
            Some("k"),
            CancellationToken::new(),
            tx,
            en(),
        );
        if mode == ServerMode::OpenRouter {
            assert_eq!(next(&mut rx).await, Some(ServerStatus::Ready));
        }
        let answered = setup
            .embedder
            .embed(texts(2), EmbedRole::Passage)
            .await
            .unwrap_or_else(|e| panic!("{mode:?}: {e}"));
        assert_eq!(answered.len(), 2, "{mode:?}");
        assert_eq!(requests(seen).await.len(), asked, "{mode:?}");
    }
}

/// A server of the user's own is not retried: it is up or it is down, and a
/// wait before saying so is a wait on a server nobody started.
#[tokio::test]
async fn a_server_of_the_users_own_is_asked_once() {
    const DOWN: Step = Step::Answer(
        "503 Service Unavailable",
        r#"{"error":{"message":"Loading model","code":503}}"#,
    );
    let (url, seen) = stub(script(vec![DOWN]));
    // Cancelled before it starts: the readiness probe is not what is asked
    // about here, and would take the stub's one answer.
    let overtaken = CancellationToken::new();
    overtaken.cancel();
    let (tx, _rx) = unbounded_channel();
    let setup = LlamaSupervisor::default().apply_embed(
        &embedder(ServerMode::External, &url),
        None,
        overtaken,
        tx,
        en(),
    );
    let failed = setup
        .embedder
        .embed(texts(1), EmbedRole::Passage)
        .await
        .expect_err("the server answered 503");
    assert!(failed.to_string().contains("Loading model"), "{failed}");
    assert_eq!(requests(seen).await.len(), 1);
}

/// Every embedder's requests are bounded, the user's own server's and a
/// cloud's alike: one text more than the cap is two requests, and the answer
/// is one list in the order the texts were given.
#[tokio::test]
async fn a_long_input_reaches_any_embedder_in_parts() {
    use crate::shared::api::embed_policy::MAX_INPUTS;

    for mode in [ServerMode::External, ServerMode::OpenAi] {
        let (url, seen) = stub(script(vec![
            Step::Answer("200 OK", vectors(MAX_INPUTS)),
            Step::Answer("200 OK", vectors(1)),
        ]));
        let overtaken = CancellationToken::new();
        overtaken.cancel();
        let (tx, _rx) = unbounded_channel();
        let setup = LlamaSupervisor::default().apply_embed(
            &embedder(mode, &url),
            Some("k"),
            overtaken,
            tx,
            en(),
        );
        let answered = setup
            .embedder
            .embed(texts(MAX_INPUTS + 1), EmbedRole::Passage)
            .await
            .unwrap_or_else(|e| panic!("{mode:?}: {e}"));
        assert_eq!(answered.len(), MAX_INPUTS + 1, "{mode:?}");
        // The stub numbers its vectors from zero in every answer: the last
        // one is the second request's first.
        assert_eq!(answered[MAX_INPUTS - 1][0], (MAX_INPUTS - 1) as f32);
        assert_eq!(answered[MAX_INPUTS][0], 0.0);
        let seen = requests(seen).await;
        assert_eq!(
            seen.iter().map(|r| inputs(r)).collect::<Vec<_>>(),
            [MAX_INPUTS, 1],
            "{mode:?}"
        );
    }
}

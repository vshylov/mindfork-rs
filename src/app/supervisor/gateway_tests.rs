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

use super::*;
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

/// The gateway's embeddings ride with its chat mode (the same enum selects
/// both): the slot is ready at once, like the other clouds' — the embedder has
/// no probe — and its request carries the model, the key and the app's name.
#[tokio::test]
async fn the_gateways_embedder_is_built_like_a_clouds() {
    const SCRIPT: &[Step] = &[Step::Answer(
        "200 OK",
        r#"{"data":[{"embedding":[0.6,0.8],"index":0}]}"#,
    )];
    let (url, seen) = stub(SCRIPT);
    let settings = EmbedSettings {
        mode: ServerMode::OpenRouter,
        openrouter: CloudSettings {
            model_name: Some("baai/bge-m3".into()),
            ..section(&url)
        },
        ..Default::default()
    };
    let (tx, _rx) = unbounded_channel();
    let setup = LlamaSupervisor::default().apply_embed(
        &settings,
        Some("sk-or-stored"),
        CancellationToken::new(),
        tx,
        en(),
    );
    assert_eq!(setup.status, ServerStatus::Ready);
    let vectors = setup
        .embedder
        .embed(vec!["hello".into()], EmbedRole::Passage)
        .await
        .expect("an embedding");
    assert_eq!(vectors, vec![vec![0.6, 0.8]]);
    let head = requests(seen).await[0].to_ascii_lowercase();
    assert!(head.starts_with("post /v1/embeddings "), "{head}");
    assert!(
        head.contains("authorization: bearer sk-or-stored"),
        "{head}"
    );
    assert!(head.contains("x-openrouter-title: mindfork"), "{head}");
}

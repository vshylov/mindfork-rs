//! Canned HTTP servers for the engine clients' stream tests: a one-shot SSE
//! answer ([`serve`]) and a sequence of answers ([`serve_in_turn`]).
//!
//! A real socket and a real SSE body, so the whole client is exercised —
//! `eventsource` framing, the wire enum, the chunks that come out — rather than
//! serde alone. Asserting on a wire type only is what let Anthropic's swallowed
//! `error` event survive: the type parsed fine, it was the client that dropped it.

use futures_util::StreamExt;

use super::contract::{ApiMessage, ChatChunk, ChatRequest, ChatStream};

/// Serves one canned `text/event-stream` response, each event as a `data:` line,
/// on a local port, and returns its base URL. The request path is not read, so any
/// client's endpoint under that base reaches it.
pub(crate) fn serve(events: &'static [&'static str]) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        let Ok((mut sock, _)) = listener.accept() else {
            return;
        };
        // Read the request head so the client's write completes.
        let mut buf = [0u8; 4096];
        let _ = sock.read(&mut buf);
        let mut body = String::new();
        for e in events {
            body.push_str(&format!("data: {e}\n\n"));
        }
        let _ = sock.write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        );
        let _ = sock.flush();
    });
    format!("http://127.0.0.1:{port}")
}

/// One canned answer of [`serve_in_turn`]: `(status line, content type, body)`.
pub(crate) type Canned = (&'static str, &'static str, &'static str);

/// Serves `answers` in order, one per connection, and hands back the body of
/// every request it was sent — for a client that asks **again** after a refusal,
/// where what the second request says is the whole test.
///
/// The thread ends by itself at a deadline: a client that makes fewer requests
/// than the test expects must leave the test failing on what was seen, not
/// hanging on a `join` (docs/lessons.md §2).
pub(crate) fn serve_in_turn(
    answers: &'static [Canned],
) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let handle = std::thread::spawn(move || {
        use std::io::Write;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut bodies = Vec::new();
        for (status, kind, answer) in answers {
            let mut sock = loop {
                match listener.accept() {
                    Ok((sock, _)) => break sock,
                    Err(_) if std::time::Instant::now() >= deadline => return bodies,
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
                }
            };
            sock.set_nonblocking(false).unwrap();
            bodies.push(request_body(&mut sock));
            // `Connection: close`, or the client pools the socket and sends its
            // next request down one this stub has already dropped.
            let _ = sock.write_all(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                    answer.len()
                )
                .as_bytes(),
            );
            let _ = sock.flush();
        }
        bodies
    });
    (format!("http://127.0.0.1:{port}"), handle)
}

/// Reads one request off the socket and returns its body — whole, by its
/// `Content-Length`, since a body can arrive in a later segment than its head.
fn request_body(sock: &mut std::net::TcpStream) -> String {
    use std::io::Read;
    let mut raw = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let text = String::from_utf8_lossy(&raw).to_string();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let want = head
                .lines()
                .filter_map(|l| l.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, len)| len.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if body.len() >= want {
                return body.to_string();
            }
        }
        match sock.read(&mut chunk) {
            Ok(n) if n > 0 => raw.extend_from_slice(&chunk[..n]),
            _ => return String::new(),
        }
    }
}

/// Every chunk of a stream, up to and including its `Finished`.
pub(crate) async fn collect(stream: ChatStream) -> Vec<ChatChunk> {
    let mut out = Vec::new();
    let mut stream = stream;
    while let Some(c) = stream.next().await {
        let last = matches!(c, ChatChunk::Finished(_));
        out.push(c);
        if last {
            break;
        }
    }
    out
}

/// The smallest request a client will send: one user message, no tools.
pub(crate) fn hello() -> ChatRequest {
    ChatRequest {
        continue_final: false,
        system: None,
        messages: vec![ApiMessage::user("hi")],
        sampling: Default::default(),
        tools: vec![],
    }
}

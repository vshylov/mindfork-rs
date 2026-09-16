//! A one-shot SSE server for the engine clients' stream tests.
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

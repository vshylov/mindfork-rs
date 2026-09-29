//! A scripted HTTP server for the tests of the clients that are not engines —
//! speech and video — as [`sse_stub`](crate::shared::api::sse_stub) is for the
//! engines' streams.
//!
//! A real socket, so what a test asserts is what left the machine: the path,
//! the headers, the body. The server answers from a script, one step per
//! request, and **keeps every request it was sent**. A request beyond the script
//! is kept too, and answered `500` — a test that counts requests sees the one
//! that should not have been made.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// One answer: the status line, the label (`Content-Type`), the body.
#[derive(Clone)]
pub(crate) struct Step {
    pub status: &'static str,
    pub label: Option<&'static str>,
    pub body: Vec<u8>,
}

/// A `200` with this label and this body.
pub(crate) fn answered(label: &'static str, body: &[u8]) -> Step {
    Step {
        status: "200 OK",
        label: Some(label),
        body: body.to_vec(),
    }
}

/// A `200` whose body is JSON.
pub(crate) fn json(body: &str) -> Step {
    answered("application/json", body.as_bytes())
}

/// A refusal: this status, and a JSON body.
pub(crate) fn refused(status: &'static str, body: &str) -> Step {
    Step {
        status,
        ..json(body)
    }
}

/// The server. `url` ends in `/v1`, as a base URL of these clients does.
pub(crate) struct Stub {
    pub url: String,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Stub {
    pub(crate) async fn serving(script: Vec<Step>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let script = Arc::new(Mutex::new(VecDeque::from(script)));
        let kept = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let request = whole_request(&mut sock).await;
                kept.lock().unwrap().push(request);
                let step = script.lock().unwrap().pop_front().unwrap_or(Step {
                    status: "500 Internal Server Error",
                    label: None,
                    body: b"not in the script".to_vec(),
                });
                let label = step
                    .label
                    .map(|l| format!("Content-Type: {l}\r\n"))
                    .unwrap_or_default();
                let head = format!(
                    "HTTP/1.1 {}\r\n{label}Connection: close\r\nContent-Length: {}\r\n\r\n",
                    step.status,
                    step.body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&step.body).await;
                let _ = sock.shutdown().await;
            }
        });
        Self { url, seen }
    }

    /// Every request so far, whole: the head, a blank line, the body.
    pub(crate) fn requests(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }

    /// The request lines so far — `post /v1/audio/speech http/1.1` — in lower
    /// case.
    pub(crate) fn asked(&self) -> Vec<String> {
        let first = |r: &String| r.lines().next().unwrap_or_default().to_ascii_lowercase();
        self.requests().iter().map(first).collect()
    }

    /// The JSON bodies of the requests that had one, in the order they came.
    pub(crate) fn bodies(&self) -> Vec<serde_json::Value> {
        self.requests()
            .iter()
            .filter_map(|r| serde_json::from_str(body_of(r)).ok())
            .collect()
    }
}

/// Reads a request to the end of its body: the head, then as many bytes as
/// `Content-Length` names.
async fn whole_request(sock: &mut tokio::net::TcpStream) -> String {
    let mut got = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let text = String::from_utf8_lossy(&got).to_string();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let wanted = head
                .lines()
                .find_map(|l| {
                    let (name, value) = l.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            if body.len() >= wanted {
                return text;
            }
        }
        match sock.read(&mut buf).await {
            Ok(0) | Err(_) => return String::from_utf8_lossy(&got).to_string(),
            Ok(n) => got.extend_from_slice(&buf[..n]),
        }
    }
}

/// The body of a request as [`Stub::requests`] holds it.
pub(crate) fn body_of(request: &str) -> &str {
    request.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

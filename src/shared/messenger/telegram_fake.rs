//! A Telegram Bot API of the tests' own (ADR 0013, stage B): a real socket
//! answering `getMe`, `getUpdates`, `sendChatAction` and `sendMessage`, so the
//! bridge is driven end to end — its poller, its sender, the markup — with no
//! account and no network. A test puts a message "from the phone" with
//! [`TelegramFake::say`]; the next poll carries it; everything the bot sends is
//! kept, in order, for the test to read.
//!
//! A poll with nothing to deliver is held a moment, as Telegram's long poll
//! is, so the poller does not spin.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::sync::Notify;

/// One call the bot made: its method and its body.
#[derive(Debug, Clone)]
pub(crate) struct Sent {
    pub method: String,
    pub body: serde_json::Value,
}

struct State {
    username: String,
    /// The token the fake accepts; any other is refused `401`.
    token: String,
    updates: std::sync::Mutex<VecDeque<serde_json::Value>>,
    sent: std::sync::Mutex<Vec<Sent>>,
    arrived: Notify,
    sent_any: Notify,
    next_update: AtomicI64,
}

/// The server. `api` is what `bridge.telegram_api` names.
pub(crate) struct TelegramFake {
    pub api: String,
    state: Arc<State>,
}

impl TelegramFake {
    /// A fake whose bot is `@username` and whose token is `token`.
    pub(crate) async fn start(username: &str, token: &str) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(State {
            username: username.to_string(),
            token: token.to_string(),
            updates: Default::default(),
            sent: Default::default(),
            arrived: Notify::new(),
            sent_any: Notify::new(),
            next_update: AtomicI64::new(1),
        });
        let serving = state.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let state = serving.clone();
                tokio::spawn(async move {
                    let request = crate::shared::http_stub::whole_request(&mut sock).await;
                    let (status, body) = answer(&state, &request).await;
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(body.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        Self { api, state }
    }

    fn push(&self, message: serde_json::Value) {
        let id = self.state.next_update.fetch_add(1, Ordering::SeqCst);
        self.state
            .updates
            .lock()
            .unwrap()
            .push_back(serde_json::json!({ "update_id": id, "message": message }));
        self.state.arrived.notify_waiters();
    }

    /// A message from account `from` (`@username`) in its private chat.
    pub(crate) fn say(&self, from: i64, username: &str, text: &str) {
        self.push(serde_json::json!({
            "message_id": 1, "date": 0, "text": text,
            "from": { "id": from, "is_bot": false, "first_name": username, "username": username },
            "chat": { "id": from, "type": "private" },
        }));
    }

    /// The same, forwarded from someone called `origin`.
    pub(crate) fn forward(&self, from: i64, username: &str, origin: &str, text: &str) {
        self.push(serde_json::json!({
            "message_id": 1, "date": 0, "text": text,
            "from": { "id": from, "is_bot": false, "first_name": username, "username": username },
            "chat": { "id": from, "type": "private" },
            "forward_origin": { "type": "hidden_user", "sender_user_name": origin, "date": 0 },
        }));
    }

    /// A sticker — something the bridge does not read.
    pub(crate) fn sticker(&self, from: i64, username: &str) {
        self.push(serde_json::json!({
            "message_id": 1, "date": 0, "sticker": { "file_id": "x" },
            "from": { "id": from, "is_bot": false, "first_name": username, "username": username },
            "chat": { "id": from, "type": "private" },
        }));
    }

    /// Every call the bot made, in order.
    pub(crate) fn sent(&self) -> Vec<Sent> {
        self.state.sent.lock().unwrap().clone()
    }

    /// The texts the bot sent to `chat`, in order.
    pub(crate) fn texts_to(&self, chat: i64) -> Vec<String> {
        self.sent()
            .into_iter()
            .filter(|s| s.method == "sendMessage" && s.body["chat_id"] == chat)
            .filter_map(|s| s.body["text"].as_str().map(str::to_string))
            .collect()
    }

    /// Waits until the bot has sent `chat` `n` texts, at most `secs`; the
    /// texts so far either way.
    pub(crate) async fn wait_texts(&self, chat: i64, n: usize, secs: u64) -> Vec<String> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            // Armed before the check, so a send between the two is not missed.
            let notified = self.state.sent_any.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let got = self.texts_to(chat);
            if got.len() >= n {
                return got;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return self.texts_to(chat);
            }
        }
    }
}

/// The method of a request line `POST /bot<token>/<method> HTTP/1.1`, and
/// whether its token is the fake's.
fn route(state: &State, request: &str) -> (String, bool) {
    let path = request
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or_default();
    let mut parts = path.trim_start_matches('/').splitn(2, '/');
    let bot = parts.next().unwrap_or_default();
    let method = parts.next().unwrap_or_default().to_string();
    (method, bot == format!("bot{}", state.token))
}

async fn answer(state: &State, request: &str) -> (&'static str, String) {
    let (method, ok) = route(state, request);
    if !ok {
        return (
            "401 Unauthorized",
            r#"{"ok":false,"error_code":401,"description":"Unauthorized"}"#.into(),
        );
    }
    let body: serde_json::Value =
        serde_json::from_str(crate::shared::http_stub::body_of(request)).unwrap_or_default();
    let ok = |result: serde_json::Value| {
        (
            "200 OK",
            serde_json::json!({ "ok": true, "result": result }).to_string(),
        )
    };
    match method.as_str() {
        "getMe" => ok(serde_json::json!({
            "id": 1, "is_bot": true, "first_name": "Bot", "username": state.username,
        })),
        "getUpdates" => {
            let offset = body["offset"].as_i64().unwrap_or(0);
            let take = || {
                let mut q = state.updates.lock().unwrap();
                q.retain(|u| u["update_id"].as_i64().unwrap_or(0) >= offset);
                q.iter().cloned().collect::<Vec<_>>()
            };
            let mut got = take();
            if got.is_empty() {
                let _ = tokio::time::timeout(Duration::from_millis(300), state.arrived.notified())
                    .await;
                got = take();
            }
            ok(serde_json::Value::Array(got))
        }
        "sendMessage" | "sendChatAction" => {
            state.sent.lock().unwrap().push(Sent {
                method: method.clone(),
                body,
            });
            state.sent_any.notify_waiters();
            if method == "sendMessage" {
                ok(
                    serde_json::json!({ "message_id": 1, "date": 0, "chat": { "id": 0, "type": "private" } }),
                )
            } else {
                ok(serde_json::json!(true))
            }
        }
        _ => (
            "404 Not Found",
            r#"{"ok":false,"error_code":404,"description":"Not Found"}"#.into(),
        ),
    }
}

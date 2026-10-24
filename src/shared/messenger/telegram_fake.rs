//! A Telegram Bot API of the tests' own (ADR 0013, stage B): a real socket
//! answering `getMe`, `getUpdates`, `sendChatAction`, `sendMessage`, `getFile`
//! and a file's download, and `sendVoice` (stage C), so the bridge is driven end
//! to end — its poller, its sender, the markup, a picture's or a voice note's
//! fetch, a voice note sent back — with no account and no network. A test puts a message "from the phone" with
//! [`TelegramFake::say`]; the next poll carries it; everything the bot sends is
//! kept, in order, for the test to read.
//!
//! A poll with nothing to deliver is held a moment, as Telegram's long poll
//! is, so the poller does not spin.

use std::collections::{HashMap, VecDeque};
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
    /// The files a test put up, by id.
    files: std::sync::Mutex<HashMap<String, Vec<u8>>>,
    /// The ids `getFile` was asked for, in order.
    files_asked: std::sync::Mutex<Vec<String>>,
    /// The voice notes the bot sent, by chat, in order: the file's bytes.
    voices: std::sync::Mutex<Vec<(i64, Vec<u8>)>>,
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
            files: Default::default(),
            files_asked: Default::default(),
            voices: Default::default(),
            arrived: Notify::new(),
            sent_any: Notify::new(),
            next_update: AtomicI64::new(1),
        });
        let serving = state.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let state = serving.clone();
                tokio::spawn(async move {
                    let raw = crate::shared::http_stub::whole_request_bytes(&mut sock).await;
                    let request = String::from_utf8_lossy(&raw).to_string();
                    let (status, label, body) = match download(&state, &request) {
                        Some(file) => file,
                        None if route(&state, &request) == ("sendVoice".to_string(), true) => (
                            "200 OK",
                            "application/json",
                            voice_sent(&state, &raw).into_bytes(),
                        ),
                        None => {
                            let (status, body) = answer(&state, &request).await;
                            (status, "application/json", body.into_bytes())
                        }
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {label}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(&body).await;
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

    /// Puts `bytes` up as a file and returns its id.
    fn file(&self, bytes: &[u8]) -> String {
        let mut files = self.state.files.lock().unwrap();
        let id = format!("file{}", files.len() + 1);
        files.insert(id.clone(), bytes.to_vec());
        id
    }

    /// A photo from `from`, with its caption and the album it is part of.
    pub(crate) fn photo(
        &self,
        from: i64,
        username: &str,
        bytes: &[u8],
        caption: Option<&str>,
        album: Option<&str>,
    ) {
        let id = self.file(bytes);
        let mut m = serde_json::json!({
            "message_id": 1, "date": 0,
            "from": { "id": from, "is_bot": false, "first_name": username, "username": username },
            "chat": { "id": from, "type": "private" },
            "photo": [{ "file_id": format!("{id}-thumb"), "width": 90, "height": 90 },
                      { "file_id": id, "width": 1280, "height": 1280 }],
        });
        if let Some(c) = caption {
            m["caption"] = c.into();
        }
        if let Some(a) = album {
            m["media_group_id"] = a.into();
        }
        self.push(m);
    }

    /// An image sent as a file.
    pub(crate) fn image_file(&self, from: i64, username: &str, name: &str, bytes: &[u8]) {
        let id = self.file(bytes);
        self.push(serde_json::json!({
            "message_id": 1, "date": 0,
            "from": { "id": from, "is_bot": false, "first_name": username, "username": username },
            "chat": { "id": from, "type": "private" },
            "document": { "file_id": id, "file_name": name, "mime_type": "image/png" },
        }));
    }

    /// The file ids `getFile` was asked for.
    pub(crate) fn files_asked(&self) -> Vec<String> {
        self.state.files_asked.lock().unwrap().clone()
    }

    /// A voice note from `from`, `seconds` long — OGG/Opus, as a phone sends it.
    pub(crate) fn voice(&self, from: i64, username: &str, note: &[u8], seconds: u32) {
        let id = self.file(note);
        self.push(serde_json::json!({
            "message_id": 1, "date": 0,
            "from": { "id": from, "is_bot": false, "first_name": username, "username": username },
            "chat": { "id": from, "type": "private" },
            "voice": { "file_id": id, "file_unique_id": id, "duration": seconds, "mime_type": "audio/ogg" },
        }));
    }

    /// The voice notes the bot sent `chat`, in order.
    pub(crate) fn voices_to(&self, chat: i64) -> Vec<Vec<u8>> {
        let voices = self.state.voices.lock().unwrap();
        voices
            .iter()
            .filter(|(to, _)| *to == chat)
            .map(|(_, note)| note.clone())
            .collect()
    }

    /// Waits until the bot has sent `chat` `n` voice notes, at most `secs`.
    pub(crate) async fn wait_voices(&self, chat: i64, n: usize, secs: u64) -> Vec<Vec<u8>> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            let notified = self.state.sent_any.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let got = self.voices_to(chat);
            if got.len() >= n {
                return got;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return self.voices_to(chat);
            }
        }
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

/// `sendVoice`, a multipart form: the voice note kept with its chat, the call
/// kept as the others are (its fields, without the file), the answer the API's.
fn voice_sent(state: &State, raw: &[u8]) -> String {
    let fields = multipart_fields(raw);
    let field = |name: &str| {
        fields
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let chat: i64 = String::from_utf8_lossy(&field("chat_id"))
        .trim()
        .parse()
        .unwrap_or(0);
    let duration: u64 = String::from_utf8_lossy(&field("duration"))
        .trim()
        .parse()
        .unwrap_or(0);
    state.voices.lock().unwrap().push((chat, field("voice")));
    state.sent.lock().unwrap().push(Sent {
        method: "sendVoice".into(),
        body: serde_json::json!({ "chat_id": chat, "duration": duration }),
    });
    state.sent_any.notify_waiters();
    serde_json::json!({ "ok": true, "result": { "message_id": 1, "date": 0 } }).to_string()
}

/// The parts of a multipart form, by name, as bytes.
fn multipart_fields(raw: &[u8]) -> Vec<(String, Vec<u8>)> {
    let Some(head_end) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
        return Vec::new();
    };
    let head = String::from_utf8_lossy(&raw[..head_end]);
    let Some(boundary) = head
        .lines()
        .find_map(|l| l.split_once("boundary=").map(|(_, b)| b.trim().to_string()))
    else {
        return Vec::new();
    };
    let body = &raw[head_end + 4..];
    let marker = format!("--{boundary}");
    let marker = marker.as_bytes();
    let mut starts = Vec::new();
    let mut at = 0;
    while let Some(pos) = body[at..].windows(marker.len()).position(|w| w == marker) {
        starts.push(at + pos);
        at += pos + marker.len();
    }
    starts
        .windows(2)
        .filter_map(|pair| {
            let part = &body[pair[0] + marker.len()..pair[1]];
            let split = part.windows(4).position(|w| w == b"\r\n\r\n")?;
            let headers = String::from_utf8_lossy(&part[..split]);
            let name = headers
                .split("name=\"")
                .nth(1)?
                .split('"')
                .next()?
                .to_string();
            let content = &part[split + 4..];
            let content = content.strip_suffix(b"\r\n").unwrap_or(content);
            Some((name, content.to_vec()))
        })
        .collect()
}

/// A file's download — `GET /file/bot<token>/files/<id>` — answered with its
/// bytes; `None` for any other request.
fn download(state: &State, request: &str) -> Option<(&'static str, &'static str, Vec<u8>)> {
    let path = request
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))?;
    let rest = path.strip_prefix(&format!("/file/bot{}/files/", state.token))?;
    Some(match state.files.lock().unwrap().get(rest) {
        Some(bytes) => ("200 OK", "application/octet-stream", bytes.clone()),
        None => ("404 Not Found", "text/plain", b"no such file".to_vec()),
    })
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
        "getFile" => {
            let id = body["file_id"].as_str().unwrap_or_default().to_string();
            state.files_asked.lock().unwrap().push(id.clone());
            match state.files.lock().unwrap().get(&id) {
                Some(bytes) => ok(serde_json::json!({
                    "file_id": id, "file_size": bytes.len(), "file_path": format!("files/{id}"),
                })),
                None => (
                    "400 Bad Request",
                    r#"{"ok":false,"error_code":400,"description":"Bad Request: invalid file_id"}"#
                        .into(),
                ),
            }
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

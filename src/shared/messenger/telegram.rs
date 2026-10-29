//! The Telegram Bot API, as much of it as the bridge uses (ADR 0013):
//! `getMe`, `getUpdates` (long polling), `sendMessage`, `sendChatAction`,
//! `getFile` with the download, `sendVoice`, and `sendPhoto`.
//!
//! Long polling only: every call is an outbound HTTPS request, so a pod behind
//! NAT needs no address of its own, and nothing here listens. The token is part
//! of every URL, so no error this module returns carries a URL
//! (`reqwest::Error::without_url`) — a log line must never hold the bot.

use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;

/// Telegram's own Bot API server. A self-hosted one (Telegram's
/// `telegram-bot-api`) is named in the settings instead.
pub const DEFAULT_API: &str = "https://api.telegram.org";

/// How long a `getUpdates` waits on the server for something to arrive.
pub const POLL_TIMEOUT_SECS: u64 = 50;

/// How long any other call may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// The longest text one message may carry (Bot API, `sendMessage`).
pub const MESSAGE_LIMIT: usize = 4096;

/// The largest file a bot may download (Bot API, `getFile`): 20 MB.
pub const FILE_LIMIT: u64 = 20 * 1024 * 1024;

/// The largest photo a bot may send (Bot API, `sendPhoto`): 10 MB.
pub const PHOTO_LIMIT: u64 = 10 * 1024 * 1024;

/// A photo's width and height together, at most (Bot API, `sendPhoto`; its
/// other rule, one side at most 20 times the other, no scaling mends).
pub const PHOTO_SIDES: u32 = 10_000;

/// How long a file download — or an upload — may take.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// What a call came to when it did not succeed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TelegramError {
    /// A file larger than the caller takes (or than a bot may download):
    /// its size in bytes, as Telegram or the transfer reported it.
    #[error("the file is {0} bytes, more than may be taken")]
    TooBig(u64),
    /// `401`: the token is wrong or was revoked — the bot is gone until a new
    /// token is set.
    #[error("Telegram refused the bot's token (401)")]
    Unauthorized,
    /// `409`: another program polls this bot, or a webhook is set on it —
    /// Telegram's own words.
    #[error("{0}")]
    Conflict(String),
    /// Any other refusal, in Telegram's words; `retry_after` when it asked to
    /// wait (`429`).
    #[error("Telegram refused the request ({code}): {description}")]
    Refused {
        code: u16,
        description: String,
        retry_after: Option<u64>,
    },
    /// The request did not get an answer: no network, a timeout, a body that
    /// is not the API's.
    #[error("{0}")]
    Network(String),
}

impl TelegramError {
    /// The message's markup was refused — the caller sends it again as plain
    /// text rather than lose it.
    pub fn is_markup_refused(&self) -> bool {
        matches!(self, Self::Refused { code: 400, description, .. }
            if description.contains("can't parse entities"))
    }
}

/// The bot itself, as `getMe` answers.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BotUser {
    pub id: i64,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub first_name: String,
}

/// One update. Only messages are read; the rest are skipped by their id.
#[derive(Debug, Clone, Deserialize)]
pub struct Update {
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<TgMessage>,
}

/// A message, as much of it as the bridge reads.
#[derive(Debug, Clone, Deserialize)]
pub struct TgMessage {
    #[serde(default)]
    pub from: Option<TgUser>,
    pub chat: TgChat,
    #[serde(default)]
    pub text: Option<String>,
    /// Where a forwarded message came from (`MessageOrigin`): kept raw, since
    /// only a name is read out of it ([`forward_name`]).
    #[serde(default)]
    pub forward_origin: Option<serde_json::Value>,
    /// A photo, in the sizes Telegram keeps of it, smallest first.
    #[serde(default)]
    pub photo: Option<Vec<PhotoSize>>,
    /// A file sent as a file — an image among them when its type says so.
    #[serde(default)]
    pub document: Option<TgDocument>,
    /// The words under a photo or a file.
    #[serde(default)]
    pub caption: Option<String>,
    /// The album a photo belongs to: an album arrives as one message per photo,
    /// each carrying the same id.
    #[serde(default)]
    pub media_group_id: Option<String>,
    /// A voice note recorded in the app: OGG/Opus.
    #[serde(default)]
    pub voice: Option<TgVoice>,
}

/// A voice note, as Telegram describes it.
#[derive(Debug, Clone, Deserialize)]
pub struct TgVoice {
    pub file_id: String,
    /// Its length in seconds, as the sender's app measured it.
    #[serde(default)]
    pub duration: u32,
}

/// One size of a photo. Its size in bytes is read where it is downloaded
/// (`getFile`), which every file goes through.
#[derive(Debug, Clone, Deserialize)]
pub struct PhotoSize {
    pub file_id: String,
}

/// A file sent as a file.
#[derive(Debug, Clone, Deserialize)]
pub struct TgDocument {
    pub file_id: String,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
}

/// What `getFile` answers: where the file is to be downloaded from.
#[derive(Debug, Clone, Deserialize)]
pub struct TgFile {
    #[serde(default)]
    pub file_size: Option<u64>,
    #[serde(default)]
    pub file_path: Option<String>,
}

/// The sender of a message. An id has at most 52 significant bits and never
/// changes; the username can.
#[derive(Debug, Clone, Deserialize)]
pub struct TgUser {
    pub id: i64,
    #[serde(default)]
    pub is_bot: bool,
    #[serde(default)]
    pub first_name: String,
    #[serde(default)]
    pub username: Option<String>,
}

impl TgUser {
    /// How the person is named on screen: `@username`, else the first name.
    pub fn handle(&self) -> String {
        match &self.username {
            Some(u) if !u.is_empty() => format!("@{u}"),
            _ => self.first_name.clone(),
        }
    }
}

/// The chat a message is in. The bridge answers only `private` ones, whose
/// id is the sender's own — so the answer goes to the account.
#[derive(Debug, Clone, Deserialize)]
pub struct TgChat {
    #[serde(rename = "type", default)]
    pub kind: String,
}

/// Who a forwarded message came from, as `MessageOrigin` names them: a user, a
/// user who hides their account, a chat or a channel. `None` when the origin
/// names nobody this reads.
pub fn forward_name(origin: &serde_json::Value) -> Option<String> {
    let str_at = |path: &[&str]| {
        let mut v = origin;
        for key in path {
            v = v.get(key)?;
        }
        v.as_str().filter(|s| !s.is_empty()).map(str::to_string)
    };
    match origin.get("type")?.as_str()? {
        "user" => str_at(&["sender_user", "username"])
            .map(|u| format!("@{u}"))
            .or_else(|| str_at(&["sender_user", "first_name"])),
        "hidden_user" => str_at(&["sender_user_name"]),
        "chat" => str_at(&["sender_chat", "title"]),
        "channel" => str_at(&["chat", "title"]),
        _ => None,
    }
}

/// The API's envelope: `ok`, and either the result or the refusal.
#[derive(Deserialize)]
struct Envelope<T> {
    ok: bool,
    #[serde(default = "Option::default")]
    result: Option<T>,
    #[serde(default)]
    error_code: Option<u16>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    parameters: Option<Parameters>,
}

#[derive(Deserialize)]
struct Parameters {
    #[serde(default)]
    retry_after: Option<u64>,
}

/// One bot: its token and the server it talks to.
#[derive(Clone)]
pub struct TelegramClient {
    http: reqwest::Client,
    /// `{api}/bot{token}` — never logged, never in an error.
    base: String,
    /// `{api}/file/bot{token}`, where a file is downloaded from — the same.
    files: String,
}

impl TelegramClient {
    /// A client for the bot with `token` on the server at `api`
    /// ([`DEFAULT_API`] unless the settings name a self-hosted one).
    pub fn new(api: &str, token: &str) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            // Fails only when TLS cannot initialise, which `Client::new` panics
            // on: the same behaviour, without the panic's message.
            .unwrap_or_else(|_| reqwest::Client::new());
        let api = api.trim_end_matches('/');
        let token = token.trim();
        Self {
            http,
            base: format!("{api}/bot{token}"),
            files: format!("{api}/file/bot{token}"),
        }
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        body: serde_json::Value,
        timeout: Duration,
    ) -> Result<T, TelegramError> {
        let resp = self
            .http
            .post(format!("{}/{method}", self.base))
            .json(&body)
            .timeout(timeout)
            .send()
            .await
            .map_err(|e| TelegramError::Network(e.without_url().to_string()))?;
        Self::answer(resp).await
    }

    /// An answer read as the API's envelope: the result, or the refusal told
    /// apart.
    async fn answer<T: DeserializeOwned>(resp: reqwest::Response) -> Result<T, TelegramError> {
        let status = resp.status().as_u16();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| TelegramError::Network(e.without_url().to_string()))?;
        let envelope: Envelope<T> = serde_json::from_slice(&bytes).map_err(|_| {
            let text = String::from_utf8_lossy(&bytes);
            let excerpt: String = text.chars().take(200).collect();
            TelegramError::Refused {
                code: status,
                description: excerpt,
                retry_after: None,
            }
        })?;
        if envelope.ok
            && let Some(result) = envelope.result
        {
            return Ok(result);
        }
        let code = envelope.error_code.unwrap_or(status);
        let description = envelope.description.unwrap_or_default();
        Err(match code {
            401 => TelegramError::Unauthorized,
            409 => TelegramError::Conflict(description),
            _ => TelegramError::Refused {
                code,
                description,
                retry_after: envelope.parameters.and_then(|p| p.retry_after),
            },
        })
    }

    /// The bot's own account — what the pairing link names, and the first
    /// proof that the token works.
    pub async fn get_me(&self) -> Result<BotUser, TelegramError> {
        self.call("getMe", serde_json::json!({}), CALL_TIMEOUT)
            .await
    }

    /// The updates after `offset`, waiting up to `wait_secs` on the server for
    /// the first. Asking with an offset past an update confirms it: Telegram
    /// forgets it, so a restart does not read it twice.
    pub async fn get_updates(
        &self,
        offset: Option<i64>,
        wait_secs: u64,
    ) -> Result<Vec<Update>, TelegramError> {
        let mut body = serde_json::json!({
            "timeout": wait_secs,
            "allowed_updates": ["message"],
        });
        if let Some(offset) = offset {
            body["offset"] = offset.into();
        }
        self.call(
            "getUpdates",
            body,
            Duration::from_secs(wait_secs) + CALL_TIMEOUT,
        )
        .await
    }

    /// Sends `text` to `chat`, in Telegram's HTML subset when `html`.
    pub async fn send_message(
        &self,
        chat: i64,
        text: &str,
        html: bool,
    ) -> Result<(), TelegramError> {
        let mut body = serde_json::json!({
            "chat_id": chat,
            "text": text,
            "link_preview_options": { "is_disabled": true },
        });
        if html {
            body["parse_mode"] = "HTML".into();
        }
        self.call::<serde_json::Value>("sendMessage", body, CALL_TIMEOUT)
            .await
            .map(|_| ())
    }

    /// The file `file_id` names, at most `max` bytes (and never more than a
    /// bot may download, [`FILE_LIMIT`]): `getFile`, then the download. A size
    /// Telegram reports above the cap is refused before anything is fetched,
    /// and a transfer that grows past it is cut off.
    pub async fn download(&self, file_id: &str, max: u64) -> Result<Vec<u8>, TelegramError> {
        let max = max.min(FILE_LIMIT);
        let file: TgFile = self
            .call(
                "getFile",
                serde_json::json!({ "file_id": file_id }),
                CALL_TIMEOUT,
            )
            .await?;
        if let Some(size) = file.file_size.filter(|s| *s > max) {
            return Err(TelegramError::TooBig(size));
        }
        let Some(path) = file.file_path else {
            return Err(TelegramError::Refused {
                code: 400,
                description: "getFile named no file path".into(),
                retry_after: None,
            });
        };
        let mut resp = self
            .http
            .get(format!("{}/{path}", self.files))
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .await
            .map_err(|e| TelegramError::Network(e.without_url().to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(TelegramError::Refused {
                code: status.as_u16(),
                description: status.canonical_reason().unwrap_or_default().to_string(),
                retry_after: None,
            });
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| TelegramError::Network(e.without_url().to_string()))?
        {
            bytes.extend_from_slice(&chunk);
            if bytes.len() as u64 > max {
                return Err(TelegramError::TooBig(bytes.len() as u64));
            }
        }
        Ok(bytes)
    }

    /// Shows *typing…* in `chat` — for five seconds at most, or until the
    /// bot's next message.
    pub async fn send_typing(&self, chat: i64) -> Result<(), TelegramError> {
        self.send_action(chat, "typing").await
    }

    /// Shows *recording a voice message…* in `chat`, as long as
    /// [`Self::send_typing`] shows its own.
    pub async fn send_recording(&self, chat: i64) -> Result<(), TelegramError> {
        self.send_action(chat, "record_voice").await
    }

    /// Shows *sending a photo…* in `chat`, as long as [`Self::send_typing`]
    /// shows its own.
    pub async fn send_uploading(&self, chat: i64) -> Result<(), TelegramError> {
        self.send_action(chat, "upload_photo").await
    }

    async fn send_action(&self, chat: i64, action: &str) -> Result<(), TelegramError> {
        self.call::<bool>(
            "sendChatAction",
            serde_json::json!({ "chat_id": chat, "action": action }),
            CALL_TIMEOUT,
        )
        .await
        .map(|_| ())
    }

    /// Sends `note` — OGG/Opus, which Telegram shows as a voice message — to
    /// `chat`, `seconds` long.
    pub async fn send_voice(
        &self,
        chat: i64,
        note: Vec<u8>,
        seconds: u32,
    ) -> Result<(), TelegramError> {
        let file = reqwest::multipart::Part::bytes(note)
            .file_name("voice.ogg")
            .mime_str("audio/ogg")
            .map_err(|e| TelegramError::Network(e.without_url().to_string()))?;
        let form = reqwest::multipart::Form::new()
            .text("chat_id", chat.to_string())
            .text("duration", seconds.to_string())
            .part("voice", file);
        self.upload("sendVoice", form).await
    }

    /// Sends `photo` — PNG or JPEG within what `sendPhoto` takes
    /// ([`PHOTO_LIMIT`], [`PHOTO_SIDES`]) — to `chat` as a photo, with
    /// `caption` under it. Telegram keeps a recompressed copy.
    pub async fn send_photo(
        &self,
        chat: i64,
        photo: Vec<u8>,
        mime: &str,
        caption: &str,
    ) -> Result<(), TelegramError> {
        let name = if mime == "image/jpeg" {
            "photo.jpg"
        } else {
            "photo.png"
        };
        let file = reqwest::multipart::Part::bytes(photo)
            .file_name(name)
            .mime_str(mime)
            .map_err(|e| TelegramError::Network(e.without_url().to_string()))?;
        let form = reqwest::multipart::Form::new()
            .text("chat_id", chat.to_string())
            .text("caption", caption.to_string())
            .part("photo", file);
        self.upload("sendPhoto", form).await
    }

    /// A file sent up as a multipart form, its answer read as any other's.
    async fn upload(
        &self,
        method: &str,
        form: reqwest::multipart::Form,
    ) -> Result<(), TelegramError> {
        let resp = self
            .http
            .post(format!("{}/{method}", self.base))
            .multipart(form)
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .await
            .map_err(|e| TelegramError::Network(e.without_url().to_string()))?;
        Self::answer::<serde_json::Value>(resp).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::http_stub::{Stub, json, refused};

    fn client(stub: &Stub) -> TelegramClient {
        // The stub's URL ends in `/v1`; the client appends `/bot<token>`.
        TelegramClient::new(&stub.url, "123:secret")
    }

    #[tokio::test]
    async fn get_me_reads_the_bot_and_puts_the_token_in_the_path() {
        let stub = Stub::serving(vec![json(
            r#"{"ok":true,"result":{"id":7,"is_bot":true,"first_name":"Mara","username":"mara_bot"}}"#,
        )])
        .await;
        let me = client(&stub).get_me().await.unwrap();
        assert_eq!(me.username.as_deref(), Some("mara_bot"));
        assert_eq!(stub.asked(), ["post /v1/bot123:secret/getme http/1.1"]);
    }

    #[tokio::test]
    async fn the_three_refusals_the_bridge_tells_apart() {
        let stub = Stub::serving(vec![
            refused(
                "401 Unauthorized",
                r#"{"ok":false,"error_code":401,"description":"Unauthorized"}"#,
            ),
            refused(
                "409 Conflict",
                r#"{"ok":false,"error_code":409,"description":"Conflict: terminated by other getUpdates request"}"#,
            ),
            refused(
                "429 Too Many Requests",
                r#"{"ok":false,"error_code":429,"description":"Too Many Requests: retry after 7","parameters":{"retry_after":7}}"#,
            ),
        ])
        .await;
        let c = client(&stub);
        assert_eq!(c.get_me().await, Err(TelegramError::Unauthorized));
        assert!(matches!(
            c.get_updates(None, 0).await,
            Err(TelegramError::Conflict(d)) if d.contains("other getUpdates")
        ));
        assert!(matches!(
            c.send_typing(5).await,
            Err(TelegramError::Refused {
                code: 429,
                retry_after: Some(7),
                ..
            })
        ));
    }

    #[tokio::test]
    async fn an_error_never_carries_the_token() {
        // Nothing listens on the port: the request fails before any answer.
        let dead = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        let c = TelegramClient::new(&format!("http://{dead}"), "123:secret");
        let err = c.get_me().await.unwrap_err();
        assert!(matches!(err, TelegramError::Network(_)));
        assert!(!err.to_string().contains("secret"), "{err}");
    }

    #[tokio::test]
    async fn updates_carry_the_offset_and_ask_for_messages_only() {
        let stub = Stub::serving(vec![json(
            r#"{"ok":true,"result":[{"update_id":10,"message":{"message_id":1,"date":0,
                "from":{"id":42,"is_bot":false,"first_name":"Ann","username":"ann"},
                "chat":{"id":42,"type":"private"},"text":"hi"}},
                {"update_id":11,"edited_message":{"message_id":1}}]}"#,
        )])
        .await;
        let ups = client(&stub).get_updates(Some(10), 0).await.unwrap();
        assert_eq!(ups.len(), 2);
        let m = ups[0].message.as_ref().unwrap();
        assert_eq!(m.from.as_ref().unwrap().handle(), "@ann");
        assert_eq!(m.chat.kind, "private");
        assert!(ups[1].message.is_none(), "an edit is skipped, not refused");
        let body = &stub.bodies()[0];
        assert_eq!(body["offset"], 10);
        assert_eq!(body["allowed_updates"], serde_json::json!(["message"]));
    }

    #[tokio::test]
    async fn a_markup_refusal_is_told_apart_from_the_rest() {
        let stub = Stub::serving(vec![refused(
            "400 Bad Request",
            r#"{"ok":false,"error_code":400,"description":"Bad Request: can't parse entities: unclosed tag"}"#,
        )])
        .await;
        let err = client(&stub)
            .send_message(5, "<b>x", true)
            .await
            .unwrap_err();
        assert!(err.is_markup_refused());
        assert_eq!(stub.bodies()[0]["parse_mode"], "HTML");
    }

    #[tokio::test]
    async fn a_file_is_fetched_by_its_path_and_refused_above_the_cap() {
        let stub = Stub::serving(vec![
            json(r#"{"ok":true,"result":{"file_id":"f1","file_size":5,"file_path":"photos/a.jpg"}}"#),
            crate::shared::http_stub::answered("image/jpeg", b"12345"),
            json(r#"{"ok":true,"result":{"file_id":"f2","file_size":900,"file_path":"photos/b.jpg"}}"#),
            json(r#"{"ok":true,"result":{"file_id":"f3","file_path":"photos/c.jpg"}}"#),
            crate::shared::http_stub::answered("image/jpeg", b"0123456789"),
        ])
        .await;
        let c = client(&stub);
        assert_eq!(c.download("f1", 100).await.unwrap(), b"12345");
        assert_eq!(c.download("f2", 100).await, Err(TelegramError::TooBig(900)));
        // No size said: the transfer itself is cut at the cap.
        assert_eq!(c.download("f3", 4).await, Err(TelegramError::TooBig(10)));
        let asked = stub.asked();
        assert_eq!(asked[0], "post /v1/bot123:secret/getfile http/1.1");
        assert_eq!(asked[1], "get /v1/file/bot123:secret/photos/a.jpg http/1.1");
        assert_eq!(
            asked.len(),
            5,
            "the refused one was never fetched: {asked:?}"
        );
    }

    #[tokio::test]
    async fn a_voice_note_goes_up_as_a_file_and_comes_down_with_its_length() {
        let stub = Stub::serving(vec![
            json(r#"{"ok":true,"result":{"message_id":3}}"#),
            json(r#"{"ok":true,"result":true}"#),
            json(
                r#"{"ok":true,"result":[{"update_id":1,"message":{"message_id":1,"date":0,
                "from":{"id":42,"is_bot":false,"first_name":"Ann"},"chat":{"id":42,"type":"private"},
                "voice":{"file_id":"v1","file_unique_id":"u","duration":12,"mime_type":"audio/ogg"}}}]}"#,
            ),
        ])
        .await;
        let c = client(&stub);
        c.send_voice(42, b"OggS-note".to_vec(), 7).await.unwrap();
        c.send_recording(42).await.unwrap();
        let ups = c.get_updates(None, 0).await.unwrap();
        let voice = ups[0].message.as_ref().unwrap().voice.as_ref().unwrap();
        assert_eq!((voice.file_id.as_str(), voice.duration), ("v1", 12));
        let reqs = stub.requests();
        assert!(
            reqs[0]
                .to_ascii_lowercase()
                .starts_with("post /v1/bot123:secret/sendvoice")
        );
        assert!(reqs[0].contains("filename=\"voice.ogg\""));
        assert!(reqs[0].contains("audio/ogg") && reqs[0].contains("OggS-note"));
        assert!(reqs[0].contains("name=\"duration\"\r\n\r\n7"));
        assert_eq!(stub.bodies()[0]["action"], "record_voice");
    }

    #[tokio::test]
    async fn a_photo_goes_up_as_a_file_with_its_caption() {
        let stub = Stub::serving(vec![
            json(r#"{"ok":true,"result":true}"#),
            json(r#"{"ok":true,"result":{"message_id":4}}"#),
            refused(
                "400 Bad Request",
                r#"{"ok":false,"error_code":400,"description":"Bad Request: PHOTO_INVALID_DIMENSIONS"}"#,
            ),
        ])
        .await;
        let c = client(&stub);
        c.send_uploading(42).await.unwrap();
        c.send_photo(
            42,
            b"\xFF\xD8\xFF-jpeg".to_vec(),
            "image/jpeg",
            "image-1.png",
        )
        .await
        .unwrap();
        let err = c
            .send_photo(42, b"png".to_vec(), "image/png", "x")
            .await
            .unwrap_err();
        assert!(
            matches!(&err, TelegramError::Refused { code: 400, description, .. }
                if description.contains("PHOTO_INVALID_DIMENSIONS")),
            "{err:?}"
        );
        assert_eq!(stub.bodies()[0]["action"], "upload_photo");
        let reqs = stub.requests();
        assert!(
            reqs[1]
                .to_ascii_lowercase()
                .starts_with("post /v1/bot123:secret/sendphoto")
        );
        assert!(reqs[1].contains("name=\"photo\"; filename=\"photo.jpg\""));
        assert!(reqs[1].contains("image/jpeg") && reqs[1].contains("-jpeg"));
        assert!(reqs[1].contains("name=\"caption\"\r\n\r\nimage-1.png"));
        assert!(reqs[2].contains("filename=\"photo.png\""));
    }

    #[test]
    fn a_forward_is_named_by_whoever_its_origin_names() {
        let name = |v: serde_json::Value| forward_name(&v);
        assert_eq!(
            name(
                serde_json::json!({"type":"user","sender_user":{"id":1,"first_name":"Bo","username":"bo"}})
            ),
            Some("@bo".into())
        );
        assert_eq!(
            name(serde_json::json!({"type":"user","sender_user":{"id":1,"first_name":"Bo"}})),
            Some("Bo".into())
        );
        assert_eq!(
            name(serde_json::json!({"type":"hidden_user","sender_user_name":"Someone"})),
            Some("Someone".into())
        );
        assert_eq!(
            name(serde_json::json!({"type":"channel","chat":{"id":-1,"title":"News"}})),
            Some("News".into())
        );
        assert_eq!(name(serde_json::json!({"type":"other"})), None);
    }
}

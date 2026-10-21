//! Notifications for someone who is not looking (spec §11.12;
//! docs/research/companion-platform.md C5 (a)): when a companion writes unasked,
//! a terminal escape that the terminal turns into a desktop notification, the
//! bell, and a push through a self-hosted or public ntfy server.
//!
//! **Which escape.** OSC 777 (`notify;title;body`) is what Windows Terminal (since
//! mid-2026), Ghostty, WezTerm, foot and rxvt-unicode show; OSC 9 (`9;text`) is
//! iTerm2's and kitty's. A terminal that knows neither ignores both, and one that
//! knows both would show two, so one is sent — OSC 777 unless the person picks the
//! other. Inside tmux an escape is wrapped in DCS passthrough, as OSC 52's is, and
//! only reaches the terminal with `allow-passthrough on`; the **bell** needs nothing
//! — tmux marks the window and passes it on — which is why it is on by default too.
//!
//! **ntfy** is an HTTP `POST` of the text to a topic's URL; the title rides in the
//! `title` query parameter, which takes UTF-8 where a header would not. Whoever
//! can read the topic reads what is sent, so by default only who wrote and where
//! is sent, never the message's words.

use std::io::{self, Write};

use serde::Serialize;

/// The terminal escape a notification is sent as (`notifications.escape`).
///
/// Deliberately **not** `Deserialize`: read through [`lenient_escape`], so a value
/// a newer version adds reads here as the default instead of failing the whole
/// `settings.json` (docs/lessons.md §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyEscape {
    /// `ESC ] 777 ; notify ; title ; body` — Windows Terminal, Ghostty, WezTerm,
    /// foot, rxvt-unicode.
    #[default]
    Osc777,
    /// `ESC ] 9 ; text` — iTerm2, kitty.
    Osc9,
    /// No escape (the bell and ntfy still go, when on).
    Off,
}

impl NotifyEscape {
    /// Every value, in the order the settings row cycles through them.
    pub const ALL: [NotifyEscape; 3] =
        [NotifyEscape::Osc777, NotifyEscape::Osc9, NotifyEscape::Off];

    /// The name a value has in `settings.json`.
    pub fn name(self) -> &'static str {
        match self {
            NotifyEscape::Osc777 => "osc777",
            NotifyEscape::Osc9 => "osc9",
            NotifyEscape::Off => "off",
        }
    }

    /// The value by its `settings.json` name; `None` for one this binary does not know.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.name() == name)
    }

    /// The next value in [`Self::ALL`], in direction `dir`, wrapping.
    pub fn cycle(self, dir: i32) -> Self {
        let n = Self::ALL.len() as i32;
        let at = Self::ALL.iter().position(|e| *e == self).unwrap_or(0) as i32;
        Self::ALL[(at + dir).rem_euclid(n) as usize]
    }
}

/// Reads `notifications.escape` leniently (see [`NotifyEscape`]).
fn lenient_escape<'de, D>(de: D) -> Result<NotifyEscape, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let raw = serde_json::Value::deserialize(de)?;
    Ok(raw
        .as_str()
        .and_then(NotifyEscape::from_name)
        .unwrap_or_else(|| {
            tracing::warn!(
                value = %raw,
                "notifications.escape is not an escape this version knows — using the default"
            );
            NotifyEscape::default()
        }))
}

/// How a notification reaches the person (`settings.json` → `notifications`;
/// settings → *Interface* → *Notifications*). Whether one is sent at all is the
/// profile's *Notify me* switch (`companion.notify`, off by default).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(default)]
pub struct NotificationSettings {
    /// The terminal escape, OSC 777 by default.
    #[serde(deserialize_with = "lenient_escape")]
    pub escape: NotifyEscape,
    /// The bell (`BEL`) beside it — what tmux passes on without passthrough.
    pub bell: bool,
    /// A ntfy topic URL (`https://ntfy.sh/<topic>` or a self-hosted one); empty —
    /// no push. Its access token, when it needs one, is a stored secret.
    pub ntfy_url: String,
    /// Whether a notification carries the start of the message, rather than only
    /// who wrote and where — off: a topic is readable by whoever knows its name.
    pub with_text: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            escape: NotifyEscape::default(),
            bell: true,
            ntfy_url: String::new(),
            with_text: false,
        }
    }
}

/// The longest title or body a notification carries, in characters.
const MAX_CHARS: usize = 200;

/// A field made safe for an escape and a push: control characters (the escape's
/// own terminators among them) and line breaks become spaces, runs of spaces
/// collapse, and the text is cut to [`MAX_CHARS`] with an ellipsis.
pub fn clean(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let words: Vec<&str> = flat.split_whitespace().collect();
    let joined = words.join(" ");
    if joined.chars().count() <= MAX_CHARS {
        return joined;
    }
    let cut: String = joined.chars().take(MAX_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

/// The bytes a notification is written to the terminal as: the escape (wrapped
/// for tmux), then the bell. Empty when both are off.
pub fn sequence(escape: NotifyEscape, bell: bool, title: &str, body: &str, tmux: bool) -> String {
    let (title, body) = (clean(title), clean(body));
    let inner = match escape {
        // The title ends at the next `;`, so it cannot carry one.
        NotifyEscape::Osc777 => Some(format!(
            "\x1b]777;notify;{};{}\x07",
            title.replace(';', ","),
            body
        )),
        // One field: a text that begins with a digit would read as a ConEmu
        // sub-command in a terminal that implements those (Windows Terminal),
        // and the title in front is never a number alone.
        NotifyEscape::Osc9 => Some(format!("\x1b]9;{title}: {body}\x07")),
        NotifyEscape::Off => None,
    };
    let mut out = match inner {
        Some(seq) if tmux => format!("\x1bPtmux;{}\x1b\\", seq.replace('\x1b', "\x1b\x1b")),
        Some(seq) => seq,
        None => String::new(),
    };
    if bell {
        out.push('\x07');
    }
    out
}

/// Writes `seq` to `out` and flushes — between frames, which an OSC does not
/// disturb (a terminal draws nothing for one).
pub fn write_to<W: Write>(out: &mut W, seq: &str) -> io::Result<()> {
    out.write_all(seq.as_bytes())?;
    out.flush()
}

/// The URL a push is sent to: the topic's, with the title as a query parameter.
pub fn ntfy_url(topic: &str, title: &str) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(topic.trim()).map_err(|e| e.to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("not an http(s) URL: {topic}"));
    }
    url.query_pairs_mut().append_pair("title", &clean(title));
    Ok(url)
}

/// Sends one push. `Err` carries what went wrong, in the server's or the
/// network's words.
pub async fn ntfy_send(
    topic: &str,
    token: Option<&str>,
    title: &str,
    body: &str,
) -> Result<(), String> {
    let url = ntfy_url(topic, title)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client.post(url).body(clean(body));
    if let Some(token) = token.map(str::trim).filter(|t| !t.is_empty()) {
        req = req.bearer_auth(token);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        Ok(())
    } else {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Err(format!("{status} {}", clean(&text)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc777_carries_title_and_body_and_the_bell_follows() {
        assert_eq!(
            sequence(
                NotifyEscape::Osc777,
                true,
                "Sol",
                "wrote in «Interview»",
                false
            ),
            "\x1b]777;notify;Sol;wrote in «Interview»\x07\x07"
        );
        // The title cannot end early on a `;`, and no field can end the escape.
        assert_eq!(
            sequence(NotifyEscape::Osc777, false, "a;b", "x\x07y\x1b]z\nw", false),
            "\x1b]777;notify;a,b;x y ]z w\x07"
        );
    }

    #[test]
    fn osc9_is_one_field_off_is_the_bell_alone_and_nothing_is_nothing() {
        assert_eq!(
            sequence(NotifyEscape::Osc9, false, "Sol", "hi", false),
            "\x1b]9;Sol: hi\x07"
        );
        assert_eq!(
            sequence(NotifyEscape::Off, true, "Sol", "hi", false),
            "\x07"
        );
        assert_eq!(sequence(NotifyEscape::Off, false, "Sol", "hi", false), "");
    }

    /// Inside tmux the escape is wrapped and its escapes doubled; the bell is not
    /// — tmux handles that one itself.
    #[test]
    fn tmux_wraps_the_escape_and_not_the_bell() {
        assert_eq!(
            sequence(NotifyEscape::Osc777, true, "S", "b", true),
            "\x1bPtmux;\x1b\x1b]777;notify;S;b\x07\x1b\\\x07"
        );
    }

    #[test]
    fn a_long_text_is_cut_with_an_ellipsis() {
        let long = "word ".repeat(100);
        let c = clean(&long);
        assert_eq!(c.chars().count(), MAX_CHARS);
        assert!(c.ends_with('…'));
    }

    #[test]
    fn the_escape_cycles_and_reads_leniently() {
        assert_eq!(NotifyEscape::Osc777.cycle(1), NotifyEscape::Osc9);
        assert_eq!(NotifyEscape::Osc777.cycle(-1), NotifyEscape::Off);
        let s: NotificationSettings =
            serde_json::from_str(r#"{"escape":"osc1337","bell":false}"#).unwrap();
        assert_eq!(s.escape, NotifyEscape::Osc777);
        assert!(!s.bell);
        let back: NotificationSettings = serde_json::from_str(
            &serde_json::to_string(&NotificationSettings {
                escape: NotifyEscape::Osc9,
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(back.escape, NotifyEscape::Osc9);
        assert!(back.bell, "the bell is on by default");
    }

    #[test]
    fn the_push_url_keeps_the_topic_and_adds_the_title() {
        let u = ntfy_url("https://ntfy.example/my-topic", "Sol · mindfork").unwrap();
        assert_eq!(u.path(), "/my-topic");
        assert_eq!(
            u.query_pairs().find(|(k, _)| k == "title").unwrap().1,
            "Sol · mindfork"
        );
        assert!(ntfy_url("ftp://x/y", "t").is_err());
        assert!(ntfy_url("not a url", "t").is_err());
    }

    /// A push reaches the server as a `POST` of the text, with the title in the
    /// query and the token as a bearer — checked against a listener of our own,
    /// no outside service.
    #[tokio::test]
    async fn a_push_is_a_post_of_the_text_with_the_title_and_the_token() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let mut got = Vec::new();
            loop {
                let n = sock.read(&mut buf).await.unwrap();
                got.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&got);
                if let Some(head) = text.find("\r\n\r\n")
                    && text.len() >= head + 4 + "Hey, how did it go?".len()
                {
                    break;
                }
                if n == 0 {
                    break;
                }
            }
            sock.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}")
                .await
                .unwrap();
            String::from_utf8_lossy(&got).to_string()
        });
        ntfy_send(
            &format!("http://{addr}/topic"),
            Some("tk_secret"),
            "Sol",
            "Hey, how did it go?",
        )
        .await
        .unwrap();
        let request = server.await.unwrap();
        assert!(
            request.starts_with("POST /topic?title=Sol HTTP/1.1"),
            "{request}"
        );
        assert!(
            request
                .to_lowercase()
                .contains("authorization: bearer tk_secret"),
            "{request}"
        );
        assert!(request.ends_with("Hey, how did it go?"), "{request}");
    }

    #[tokio::test]
    async fn a_refused_push_says_what_the_server_said() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let _ = sock
                .write_all(b"HTTP/1.1 403 Forbidden\r\ncontent-length: 9\r\n\r\nforbidden")
                .await;
        });
        let err = ntfy_send(&format!("http://{addr}/t"), None, "S", "b")
            .await
            .unwrap_err();
        assert!(err.contains("403") && err.contains("forbidden"), "{err}");
    }
}

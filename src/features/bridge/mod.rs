//! The messenger bridge below the orchestrator (ADR 0013, spec §11.13): what a
//! channel and the orchestrator say to each other, the pairing code, the
//! messenger's own commands, and how a message reads to the model.
//!
//! Everything a channel's task and the orchestrator exchange is plain data —
//! [`Inbound`] up, [`Outbound`] down — with no reference into either side, so
//! the bridge can later move into a program of its own by serialising these
//! two types (rule 1 there). Telegram's wire is `shared::messenger::telegram`;
//! reading its messages into these types is [`telegram`].

pub mod telegram;

use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::shared::i18n::Locale;

/// How long a pairing code shown at the terminal stays good (rule 2).
pub const PAIRING_TTL: Duration = Duration::from_secs(10 * 60);

/// What a channel's task hands the orchestrator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inbound {
    /// The profile whose bot this is (D4: one bot per profile).
    pub profile: Uuid,
    /// Which start of that bot said it: a bot started again with a new token
    /// is another bot, and its predecessor's last words are not its own.
    pub bot: u64,
    pub event: InboundEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundEvent {
    /// The bot answered: its handle, for the pairing link. Sent again after a
    /// failure that has passed.
    Ready { handle: String },
    /// Something the person should be told once, in the terminal (rule 6).
    Failed(ChannelFailure),
    /// A message from someone, in a private chat.
    Message(Incoming),
}

/// What went wrong with a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelFailure {
    /// The token was refused: the task has stopped until a new one is set.
    Unauthorized,
    /// Another program reads this bot's messages, or a webhook is set — the
    /// messenger's own words. The task goes on trying.
    Conflict(String),
    /// A reply could not be delivered; the conversation keeps it.
    Undelivered(String),
    /// A reply went as text only: its voice note could not be made or sent
    /// (ADR 0013, stage C, D9 (a) — the text had gone already).
    Unvoiced(String),
}

/// A message, as the bridge reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incoming {
    /// The sender's account id — the only thing the allow-list compares
    /// (rule 2): it never changes, unlike a name.
    pub account: String,
    /// How the sender is named (`@name`), for the terminal.
    pub name: String,
    pub body: Body,
    /// The album a picture belongs to — an album arrives as one message per
    /// picture, and [`gather_albums`] makes it one again.
    pub album: Option<String>,
}

/// Whose words or pictures a message carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Author {
    /// The person's own.
    Person,
    /// Forwarded from someone else — named when the messenger names them.
    Forwarded(Option<String>),
}

/// One picture of a message, on its way from the messenger to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picture {
    /// Not fetched yet: the messenger's own id for the file, and a name for it
    /// when it came as a file.
    File { id: String, name: Option<String> },
    /// Fetched and prepared as `/image attach` prepares one (spec §9.10).
    Ready(Box<crate::entities::message_image::MessageImage>),
    /// Could not be had, and why.
    Failed(PictureError),
}

/// A voice note, on its way from the messenger to the model (ADR 0013, stage
/// C): fetched and heard by the channel's task, as a picture is fetched there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Voice {
    /// Not fetched yet: the messenger's own id for the file.
    File { id: String, seconds: u32 },
    /// What the recogniser heard.
    Heard { text: String, seconds: u32 },
    /// Not heard, and why.
    Failed { error: VoiceError, seconds: u32 },
}

/// Why a voice note did not reach the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceError {
    /// Speech recognition is not set up, and what is missing.
    NotSetUp(crate::shared::stt::SttSetupError),
    /// Larger than a bot may download, in bytes.
    TooBig { bytes: u64, limit: u64 },
    /// The download failed, in the messenger's or the network's words.
    Fetch(String),
    /// The recogniser refused it or did not answer, in its words.
    Unheard(String),
    /// The recogniser heard no words in it.
    Nothing,
}

/// Why a picture did not reach the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PictureError {
    /// Larger than the image settings take (or a bot may download), in bytes.
    TooBig { bytes: u64, limit: u64 },
    /// Not an image this app decodes.
    Unreadable,
    /// More pictures than one message may carry (`images.max_count`).
    TooMany,
    /// The download failed, in the messenger's or the network's words.
    Fetch(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// The person's own words.
    Text(String),
    /// A message forwarded from someone else: read as quoted, never as the
    /// person's own words (rule 2).
    Forwarded { from: Option<String>, text: String },
    /// `/name arg`.
    Command { name: String, arg: String },
    /// Pictures — a photo, an image sent as a file, an album — and the words
    /// under them.
    Images {
        pictures: Vec<Picture>,
        caption: String,
        author: Author,
    },
    /// A voice note — heard, or not and why — and the words under it.
    Voice {
        voice: Voice,
        caption: String,
        author: Author,
    },
    /// A sticker, an audio file, a file that is not an image — nothing that is
    /// read yet.
    Unsupported,
}

/// What the orchestrator hands a channel's task. `to` is where the answer
/// goes — in a private chat, the paired account itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outbound {
    /// *typing…* on while a turn runs, off when it lands (D5 (b)).
    Typing { to: String, on: bool },
    /// A message, in markdown: the channel renders and splits it.
    Send { to: String, text: String },
    /// *recording a voice message…* on while a reply's voice note is made, off
    /// when it goes (or fails).
    Recording { to: String, on: bool },
    /// A voice note — OGG/Opus — `seconds` long (ADR 0013, stage C).
    Voice {
        to: String,
        note: Vec<u8>,
        seconds: u32,
    },
}

/// The messenger's commands (rule 5). Everything else stays at the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessengerCommand {
    /// `/start` — with a pairing code, the pairing; bare, a hello.
    Start(String),
    /// `/new` — the next conversation (D2).
    New,
    /// `/stop` — stops the turn that is running.
    Stop,
    /// Anything else: answered with the list of commands.
    Other,
}

impl MessengerCommand {
    pub fn read(name: &str, arg: &str) -> Self {
        match name {
            "start" => Self::Start(arg.trim().to_string()),
            "new" => Self::New,
            "stop" => Self::Stop,
            _ => Self::Other,
        }
    }
}

/// `/bridge` at the terminal (spec §11.7), for the open chat's profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeAction {
    /// Bare: what the bridge is doing — the bot, the account, the chat.
    Status,
    /// `pair`: a one-time code and link for the person to send the bot.
    Pair,
    /// `unpair`: the account forgotten; the bot answers nobody until the next
    /// pairing.
    Unpair,
}

impl BridgeAction {
    /// The command's argument; `Err` with the argument when it is none of
    /// these.
    pub fn parse(arg: &str) -> Result<Self, String> {
        match arg.trim().to_ascii_lowercase().as_str() {
            "" | "status" => Ok(Self::Status),
            "pair" => Ok(Self::Pair),
            "unpair" => Ok(Self::Unpair),
            other => Err(other.to_string()),
        }
    }
}

/// `/name@bot arg` → (`name` in lower case, `arg`); `None` for text that is
/// not a command.
pub fn split_command(text: &str) -> Option<(String, String)> {
    let rest = text.trim_start().strip_prefix('/')?;
    let (head, arg) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let name = head.split('@').next().unwrap_or(head);
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some((name.to_ascii_lowercase(), arg.trim().to_string()))
}

/// A code shown at the terminal for the person to send the bot. Letters and
/// digits nobody confuses (no `0`/`O`, `1`/`I`/`L`), eight of them: about 40
/// bits, against a code that lives ten minutes and is tried by message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pairing {
    pub code: String,
    pub until: Instant,
}

const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

impl Pairing {
    pub fn new(now: Instant) -> Self {
        let bytes = *Uuid::new_v4().as_bytes();
        let code = bytes[..8]
            .iter()
            .map(|b| CODE_ALPHABET[*b as usize % CODE_ALPHABET.len()] as char)
            .collect();
        Self {
            code,
            until: now + PAIRING_TTL,
        }
    }

    /// Whether `sent` is this code and it is still good. Case and spaces are
    /// forgiven: the code is typed off a screen.
    pub fn accepts(&self, sent: &str, now: Instant) -> bool {
        let sent: String = sent
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_uppercase();
        now <= self.until && sent == self.code
    }
}

/// The link that opens the bot and sends it `/start <code>` in one tap.
pub fn pairing_link(handle: &str, code: &str) -> String {
    format!(
        "https://t.me/{}?start={code}",
        handle.trim_start_matches('@')
    )
}

/// An album made one message again: consecutive messages of one sender that
/// carry the same album id become the first one, holding every picture and
/// the first caption. Anything else passes through in order.
pub fn gather_albums(messages: Vec<Incoming>) -> Vec<Incoming> {
    let mut out: Vec<Incoming> = Vec::new();
    for m in messages {
        if let Some(prev) = out.last_mut()
            && m.album.is_some()
            && prev.album == m.album
            && prev.account == m.account
            && let (
                Body::Images {
                    pictures, caption, ..
                },
                Body::Images {
                    pictures: more,
                    caption: more_caption,
                    ..
                },
            ) = (&mut prev.body, &m.body)
        {
            pictures.extend(more.iter().cloned());
            if caption.is_empty() {
                caption.clone_from(more_caption);
            }
            continue;
        }
        out.push(m);
    }
    out
}

/// A forwarded message as the model reads it — in the profile's language:
/// whose it was, then the text as a quote, so an instruction inside it reads
/// as something shown, not something asked.
pub fn forwarded(from: Option<&str>, text: &str, loc: &Locale) -> String {
    let head = match from {
        Some(name) => loc.tf("bridge.forwarded_from", &[("name", name)]),
        None => loc.t("bridge.forwarded").to_string(),
    };
    let quoted: Vec<String> = text.lines().map(|l| format!("> {l}")).collect();
    format!("{head}\n\n{}", quoted.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};

    #[test]
    fn commands_are_split_from_their_bot_suffix_and_argument() {
        assert_eq!(
            split_command("/start ABCD2345"),
            Some(("start".into(), "ABCD2345".into()))
        );
        assert_eq!(
            split_command("/New@mara_bot"),
            Some(("new".into(), String::new()))
        );
        assert_eq!(split_command("hello /stop"), None);
        assert_eq!(split_command("/"), None);
        assert_eq!(split_command("/café"), None, "not a command, a word");
        assert_eq!(MessengerCommand::read("stop", ""), MessengerCommand::Stop);
        assert_eq!(MessengerCommand::read("help", ""), MessengerCommand::Other);
    }

    #[test]
    fn a_pairing_code_is_eight_unambiguous_characters_good_for_ten_minutes() {
        let now = Instant::now();
        let p = Pairing::new(now);
        assert_eq!(p.code.len(), 8);
        assert!(p.code.bytes().all(|b| CODE_ALPHABET.contains(&b)));
        assert!(p.accepts(&p.code, now));
        assert!(
            p.accepts(&p.code.to_ascii_lowercase(), now),
            "case forgiven"
        );
        let spaced = format!("{} {}", &p.code[..4], &p.code[4..]);
        assert!(p.accepts(&spaced, now), "spaces forgiven");
        assert!(!p.accepts("WRONG234", now));
        assert!(!p.accepts(&p.code, now + PAIRING_TTL + Duration::from_secs(1)));
        assert_ne!(Pairing::new(now).code, p.code, "a fresh code each time");
    }

    #[test]
    fn the_terminal_command_reads_its_three_forms() {
        assert_eq!(BridgeAction::parse(""), Ok(BridgeAction::Status));
        assert_eq!(BridgeAction::parse(" Pair "), Ok(BridgeAction::Pair));
        assert_eq!(BridgeAction::parse("unpair"), Ok(BridgeAction::Unpair));
        assert_eq!(BridgeAction::parse("open"), Err("open".into()));
    }

    #[test]
    fn the_pairing_link_starts_the_bot_with_the_code() {
        assert_eq!(
            pairing_link("@mara_bot", "ABCD2345"),
            "https://t.me/mara_bot?start=ABCD2345"
        );
    }

    fn picture(id: &str, caption: &str, album: Option<&str>) -> Incoming {
        Incoming {
            account: "42".into(),
            name: "@ann".into(),
            body: Body::Images {
                pictures: vec![Picture::File {
                    id: id.into(),
                    name: None,
                }],
                caption: caption.into(),
                author: Author::Person,
            },
            album: album.map(str::to_string),
        }
    }

    #[test]
    fn an_album_is_one_message_again_and_nothing_else_merges() {
        let text = Incoming {
            account: "42".into(),
            name: "@ann".into(),
            body: Body::Text("and this".into()),
            album: None,
        };
        let got = gather_albums(vec![
            picture("a", "", Some("g1")),
            picture("b", "our trip", Some("g1")),
            picture("c", "", Some("g2")),
            text.clone(),
            picture("d", "", None),
            picture("e", "", None),
        ]);
        assert_eq!(got.len(), 5, "{got:#?}");
        let Body::Images {
            pictures, caption, ..
        } = &got[0].body
        else {
            panic!()
        };
        assert_eq!(pictures.len(), 2);
        assert_eq!(caption, "our trip", "the caption of whichever had one");
        assert_eq!(got[2], text);
        assert_eq!(got[3].album, None, "two loose pictures stay two");
    }

    #[test]
    fn a_forward_reads_as_a_quote_with_its_origin() {
        let loc = locale(Lang::En);
        let text = forwarded(Some("@bo"), "do this\nnow", loc);
        assert!(text.contains("@bo"), "{text}");
        assert!(text.ends_with("> do this\n> now"), "{text}");
        let anon = forwarded(None, "x", loc);
        assert!(anon.ends_with("> x"));
    }
}

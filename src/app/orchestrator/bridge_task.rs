//! One bot's tasks (ADR 0013, rule 1): a poller that reads Telegram and hands
//! the orchestrator [`Inbound`] data, and a sender that takes [`Outbound`] data
//! and writes to Telegram — *typing…* kept up while a turn runs, a reply cut
//! into messages in Telegram's markup, sent again as plain text when the
//! markup is refused, a picture the turn drew sent as a photo Telegram takes.
//! Neither holds anything of the orchestrator's.
//!
//! A picture is fetched and prepared here, before its message goes up — the
//! way `/image attach` prepares one (spec §9.10) — and only the paired
//! account's: the orchestrator keeps the poller told who that is (a `watch`),
//! so a stranger's twenty megabytes are never downloaded. A voice note is
//! fetched and heard here too, by the recogniser the orchestrator keeps the
//! poller told of (another `watch`, ADR 0013 stage C). The poller is
//! sequential, so a photo or a voice note and the question sent after it
//! arrive in order.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::entities::message_image::MessageImage;
use crate::features::bridge::{
    Body, ChannelFailure, Inbound, InboundEvent, Incoming, Outbound, Picture, PictureError, Voice,
    VoiceError,
};
use crate::shared::config::ImageSettings;
use crate::shared::messenger::telegram::{
    FILE_LIMIT, MESSAGE_LIMIT, POLL_TIMEOUT_SECS, TelegramClient, TelegramError,
};
use crate::shared::stt::{Hearing, SttError, WavMemo};

/// How often *typing…* is sent again: it lasts five seconds.
const TYPING_EVERY: Duration = Duration::from_secs(4);

/// The longest wait between two failed polls.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// How long a poll waits after a `409` before it tries again.
const CONFLICT_WAIT: Duration = Duration::from_secs(30);

/// A running bot, as the orchestrator holds it.
pub(super) struct BotTasks {
    pub(super) cancel: CancellationToken,
    pub(super) out: UnboundedSender<Outbound>,
    /// Who the bot answers — the poller fetches only their pictures and voice
    /// notes.
    pub(super) paired: watch::Sender<Option<String>>,
    /// What hears a voice note: the recognition settings as they stand.
    pub(super) hearing: watch::Sender<Hearing>,
}

/// What a bot is started with besides its client.
pub(super) struct BotStart {
    pub(super) profile: Uuid,
    /// The start's tag (see [`Inbound::bot`]).
    pub(super) bot: u64,
    /// The paired account, if any.
    pub(super) paired: Option<String>,
    /// How a picture is fetched and prepared (`settings.json` → `images`).
    pub(super) images: ImageSettings,
    /// What hears a voice note, and the session's memo of the servers that
    /// want WAV.
    pub(super) hearing: Hearing,
    pub(super) memo: Arc<WavMemo>,
}

/// Starts a profile's bot: the poller and the sender, both ended by the
/// returned token.
pub(super) fn spawn_telegram(
    start: BotStart,
    client: TelegramClient,
    inbound: UnboundedSender<Inbound>,
) -> BotTasks {
    let cancel = CancellationToken::new();
    let (out, out_rx) = unbounded_channel();
    let (paired, paired_rx) = watch::channel(start.paired);
    let (hearing, hearing_rx) = watch::channel(start.hearing);
    let tell = Teller {
        inbound,
        profile: start.profile,
        bot: start.bot,
    };
    let fetch = Fetch {
        client: client.clone(),
        paired: paired_rx,
        images: start.images,
        hearing: hearing_rx,
        memo: start.memo,
    };
    tokio::spawn(poll(fetch, tell.clone(), cancel.clone()));
    tokio::spawn(send(client, out_rx, tell, cancel.clone()));
    BotTasks {
        cancel,
        out,
        paired,
        hearing,
    }
}

/// What the poller fetches pictures with.
struct Fetch {
    client: TelegramClient,
    paired: watch::Receiver<Option<String>>,
    images: ImageSettings,
    hearing: watch::Receiver<Hearing>,
    memo: Arc<WavMemo>,
}

impl Fetch {
    /// Whether `m` is the paired account's: only theirs is ever downloaded.
    fn is_paired(&self, m: &Incoming) -> bool {
        self.paired.borrow().as_deref() == Some(m.account.as_str())
    }

    /// The voice note of `m` fetched and heard, when its sender is the paired
    /// account; anyone else's is left as it came.
    async fn voice(&self, m: &mut Incoming) {
        if !self.is_paired(m) {
            return;
        }
        let Body::Voice { voice, .. } = &mut m.body else {
            return;
        };
        let Voice::File { id, seconds } = voice else {
            return;
        };
        let seconds = *seconds;
        *voice = match self.hear(id).await {
            Ok(text) => Voice::Heard { text, seconds },
            Err(error) => Voice::Failed { error, seconds },
        };
    }

    /// One note: a recogniser built from the settings as they stand, the note
    /// downloaded within a bot's cap, then heard.
    async fn hear(&self, id: &str) -> Result<String, VoiceError> {
        let hearing = self.hearing.borrow().clone();
        let transcriber = crate::shared::stt::transcriber(&hearing, self.memo.clone())
            .map_err(VoiceError::NotSetUp)?;
        let note = match self.client.download(id, FILE_LIMIT).await {
            Ok(note) => note,
            Err(TelegramError::TooBig(bytes)) => {
                return Err(VoiceError::TooBig {
                    bytes,
                    limit: FILE_LIMIT,
                });
            }
            Err(err) => return Err(VoiceError::Fetch(err.to_string())),
        };
        match transcriber.transcribe(&note).await {
            Ok(text) => Ok(text),
            Err(SttError::Nothing) => Err(VoiceError::Nothing),
            Err(err) => Err(VoiceError::Unheard(err.to_string())),
        }
    }

    /// The pictures of `m` fetched and prepared, when its sender is the paired
    /// account; anyone else's are left as they came, for the orchestrator to
    /// drop unread.
    async fn pictures(&self, m: &mut Incoming) {
        if !self.is_paired(m) {
            return;
        }
        let Body::Images { pictures, .. } = &mut m.body else {
            return;
        };
        let mut taken = 0;
        for picture in pictures.iter_mut() {
            let Picture::File { id, name } = picture else {
                continue;
            };
            if taken >= self.images.max_count.max(1) {
                *picture = Picture::Failed(PictureError::TooMany);
                continue;
            }
            taken += 1;
            *picture = self.one(id, name.take()).await;
        }
    }

    /// One picture: downloaded within the image settings' cap and a bot's,
    /// then prepared on the blocking pool as an attached image is.
    async fn one(&self, id: &str, name: Option<String>) -> Picture {
        let limit = match self.images.max_bytes {
            0 => FILE_LIMIT,
            cap => cap.min(FILE_LIMIT),
        };
        let bytes = match self.client.download(id, limit).await {
            Ok(bytes) => bytes,
            Err(TelegramError::TooBig(bytes)) => {
                return Picture::Failed(PictureError::TooBig { bytes, limit });
            }
            Err(err) => return Picture::Failed(PictureError::Fetch(err.to_string())),
        };
        let downscale = self.images.downscale_px;
        let prepared = tokio::task::spawn_blocking(move || {
            crate::features::image_prepare::prepare(&bytes, downscale)
        })
        .await;
        match prepared {
            Ok(Ok(p)) => {
                use base64::Engine as _;
                let ext = if p.mime == "image/jpeg" { "jpg" } else { "png" };
                let name = name.unwrap_or_else(|| format!("photo.{ext}"));
                let data = base64::engine::general_purpose::STANDARD.encode(&p.bytes);
                Picture::Ready(Box::new(MessageImage::new(
                    name, "telegram", p.mime, p.width, p.height, data,
                )))
            }
            Ok(Err(_)) => Picture::Failed(PictureError::Unreadable),
            Err(err) => Picture::Failed(PictureError::Fetch(err.to_string())),
        }
    }
}

/// What a task says, tagged with its bot.
#[derive(Clone)]
struct Teller {
    inbound: UnboundedSender<Inbound>,
    profile: Uuid,
    bot: u64,
}

impl Teller {
    fn tell(&self, event: InboundEvent) {
        let _ = self.inbound.send(Inbound {
            profile: self.profile,
            bot: self.bot,
            event,
        });
    }
}

/// Sleeps `d`, or less when `cancel` fires; `false` when it did.
async fn pause(cancel: &CancellationToken, d: Duration) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(d) => true,
    }
}

async fn poll(fetch: Fetch, teller: Teller, cancel: CancellationToken) {
    let profile = teller.profile;
    let client = &fetch.client;
    let mut backoff = Duration::from_secs(1);
    // The bot first: its handle is what the pairing link names, and a refused
    // token is said before anything is polled.
    let handle = loop {
        let me = tokio::select! {
            _ = cancel.cancelled() => return,
            me = client.get_me() => me,
        };
        match me {
            Ok(me) => break me.username.unwrap_or(me.first_name),
            Err(TelegramError::Unauthorized) => {
                teller.tell(InboundEvent::Failed(ChannelFailure::Unauthorized));
                return;
            }
            Err(err) => {
                tracing::debug!(%profile, error = %err, "bridge: getMe failed, retrying");
                if !pause(&cancel, backoff).await {
                    return;
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    };
    tracing::info!(%profile, "bridge: the bot answered");
    teller.tell(InboundEvent::Ready {
        handle: handle.clone(),
    });
    let mut offset = None;
    let mut backoff = Duration::from_secs(1);
    let mut failing = false;
    loop {
        let got = tokio::select! {
            _ = cancel.cancelled() => return,
            got = client.get_updates(offset, POLL_TIMEOUT_SECS) => got,
        };
        let wait = match got {
            Ok(updates) => {
                backoff = Duration::from_secs(1);
                if failing {
                    failing = false;
                    teller.tell(InboundEvent::Ready {
                        handle: handle.clone(),
                    });
                }
                let mut read = Vec::new();
                for update in updates {
                    offset = Some(update.update_id + 1);
                    if let Some(m) = update
                        .message
                        .as_ref()
                        .and_then(crate::features::bridge::telegram::read_message)
                    {
                        read.push(m);
                    }
                }
                // An album is one message again; its pictures are fetched
                // before it goes up, so what follows it waits its turn.
                for mut m in crate::features::bridge::gather_albums(read) {
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = async {
                            fetch.pictures(&mut m).await;
                            fetch.voice(&mut m).await;
                        } => {}
                    }
                    teller.tell(InboundEvent::Message(m));
                }
                continue;
            }
            Err(TelegramError::Unauthorized) => {
                teller.tell(InboundEvent::Failed(ChannelFailure::Unauthorized));
                return;
            }
            Err(TelegramError::Conflict(why)) => {
                if !failing {
                    failing = true;
                    teller.tell(InboundEvent::Failed(ChannelFailure::Conflict(why)));
                }
                CONFLICT_WAIT
            }
            Err(TelegramError::Refused {
                retry_after: Some(secs),
                ..
            }) => Duration::from_secs(secs),
            Err(err) => {
                tracing::debug!(%profile, error = %err, "bridge: a poll failed, retrying");
                let wait = backoff;
                backoff = (backoff * 2).min(MAX_BACKOFF);
                wait
            }
        };
        if !pause(&cancel, wait).await {
            return;
        }
    }
}

async fn send(
    client: TelegramClient,
    mut out: UnboundedReceiver<Outbound>,
    teller: Teller,
    cancel: CancellationToken,
) {
    let profile = teller.profile;
    let mut typing: BTreeSet<i64> = BTreeSet::new();
    let mut recording: BTreeSet<i64> = BTreeSet::new();
    let mut tick = tokio::time::interval(TYPING_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tick.tick() => {
                for chat in &typing {
                    let _ = client.send_typing(*chat).await;
                }
                for chat in &recording {
                    let _ = client.send_recording(*chat).await;
                }
            }
            msg = out.recv() => {
                let Some(msg) = msg else { return };
                match msg {
                    Outbound::Typing { to, on } => {
                        let Ok(chat) = to.parse::<i64>() else { continue };
                        if on {
                            if typing.insert(chat) {
                                let _ = client.send_typing(chat).await;
                            }
                        } else {
                            typing.remove(&chat);
                        }
                    }
                    Outbound::Send { to, text } => {
                        let Ok(chat) = to.parse::<i64>() else { continue };
                        if let Err(err) = deliver(&client, chat, &text).await {
                            tracing::warn!(%profile, error = %err, "bridge: a reply was not delivered");
                            teller.tell(InboundEvent::Failed(ChannelFailure::Undelivered(
                                err.to_string(),
                            )));
                        }
                    }
                    Outbound::Recording { to, on } => {
                        let Ok(chat) = to.parse::<i64>() else { continue };
                        if on {
                            if recording.insert(chat) {
                                let _ = client.send_recording(chat).await;
                            }
                        } else {
                            recording.remove(&chat);
                        }
                    }
                    Outbound::Voice { to, note, seconds } => {
                        let Ok(chat) = to.parse::<i64>() else { continue };
                        recording.remove(&chat);
                        let sent = retrying(|| client.send_voice(chat, note.clone(), seconds)).await;
                        if let Err(err) = sent {
                            tracing::warn!(%profile, error = %err, "bridge: a voice note was not delivered");
                            teller.tell(InboundEvent::Failed(ChannelFailure::Unvoiced(
                                err.to_string(),
                            )));
                        }
                    }
                    Outbound::Photo { to, name, picture } => {
                        let Ok(chat) = to.parse::<i64>() else { continue };
                        if let Err(err) = deliver_photo(&client, chat, &name, picture).await {
                            tracing::warn!(%profile, error = %err, "bridge: a picture was not delivered");
                            teller.tell(InboundEvent::Failed(ChannelFailure::PictureUnsent {
                                name,
                                err,
                            }));
                        }
                    }
                }
            }
        }
    }
}

/// A drawn picture as a photo, its file's name under it: *sending a photo…*
/// while it goes up, and the copy Telegram takes made first when the original
/// is not one (docs/research/image-generation.md F9).
async fn deliver_photo(
    client: &TelegramClient,
    chat: i64,
    name: &str,
    picture: Vec<u8>,
) -> Result<(), String> {
    let _ = client.send_uploading(chat).await;
    let (photo, mime) =
        tokio::task::spawn_blocking(move || crate::features::bridge::telegram::photo(picture))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
    retrying(|| client.send_photo(chat, photo.clone(), mime, name))
        .await
        .map_err(|e| e.to_string())
}

/// One file sent up, after the wait Telegram asks for when it says so, and
/// once more after a network failure.
async fn retrying<F, Fut>(mut send: F) -> Result<(), TelegramError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), TelegramError>>,
{
    let mut tries = 0;
    loop {
        match send().await {
            Ok(()) => return Ok(()),
            Err(TelegramError::Refused {
                retry_after: Some(secs),
                ..
            }) if tries < 3 => {
                tries += 1;
                tokio::time::sleep(Duration::from_secs(secs)).await;
            }
            Err(TelegramError::Network(_)) if tries < 2 => {
                tries += 1;
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(err) => return Err(err),
        }
    }
}

/// One reply: its messages in order, each in markup, again as plain text when
/// the markup is refused, after the wait Telegram asks for when it says so.
async fn deliver(client: &TelegramClient, chat: i64, text: &str) -> Result<(), TelegramError> {
    for part in crate::shared::markdown::telegram_parts(text, MESSAGE_LIMIT) {
        let mut html = true;
        let mut tries = 0;
        loop {
            let body = if html { &part.html } else { &part.plain };
            match client.send_message(chat, body, html).await {
                Ok(()) => break,
                Err(err) if html && err.is_markup_refused() => html = false,
                Err(TelegramError::Refused {
                    retry_after: Some(secs),
                    ..
                }) if tries < 3 => {
                    tries += 1;
                    tokio::time::sleep(Duration::from_secs(secs)).await;
                }
                Err(TelegramError::Network(_)) if tries < 3 => {
                    tries += 1;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                Err(err) => return Err(err),
            }
        }
    }
    Ok(())
}

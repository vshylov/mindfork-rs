//! One bot's tasks (ADR 0013, rule 1): a poller that reads Telegram and hands
//! the orchestrator [`Inbound`] data, and a sender that takes [`Outbound`] data
//! and writes to Telegram — *typing…* kept up while a turn runs, a reply cut
//! into messages in Telegram's markup, sent again as plain text when the
//! markup is refused. Neither holds anything of the orchestrator's.

use std::collections::BTreeSet;
use std::time::Duration;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::features::bridge::{ChannelFailure, Inbound, InboundEvent, Outbound};
use crate::shared::messenger::telegram::{
    MESSAGE_LIMIT, POLL_TIMEOUT_SECS, TelegramClient, TelegramError,
};

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
}

/// Starts `profile`'s bot: the poller and the sender, both ended by the
/// returned token.
pub(super) fn spawn_telegram(
    profile: Uuid,
    bot: u64,
    client: TelegramClient,
    inbound: UnboundedSender<Inbound>,
) -> BotTasks {
    let cancel = CancellationToken::new();
    let (out, out_rx) = unbounded_channel();
    let tell = Teller {
        inbound,
        profile,
        bot,
    };
    tokio::spawn(poll(client.clone(), tell.clone(), cancel.clone()));
    tokio::spawn(send(client, out_rx, tell, cancel.clone()));
    BotTasks { cancel, out }
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

async fn poll(client: TelegramClient, teller: Teller, cancel: CancellationToken) {
    let profile = teller.profile;
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
                for update in updates {
                    offset = Some(update.update_id + 1);
                    if let Some(m) = update
                        .message
                        .as_ref()
                        .and_then(crate::features::bridge::telegram::read_message)
                    {
                        teller.tell(InboundEvent::Message(m));
                    }
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
    let mut tick = tokio::time::interval(TYPING_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tick.tick() => {
                for chat in &typing {
                    let _ = client.send_typing(*chat).await;
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
                }
            }
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

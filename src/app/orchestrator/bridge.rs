//! The messenger bridge in the orchestrator (ADR 0013, spec §11.13): one bot
//! per profile that has the switch on and a token, the one account paired to
//! it, the conversation its messages reach, and a **seat** for each turn that
//! came from the messenger.
//!
//! A seat is a turn of its own beside the terminal's (D1): it runs while the
//! person types elsewhere, under the shared session budget, with the profile's
//! tools less the dangerous four (D3). Its stream reaches the screen only while
//! its chat is open — from the same mirror a late opening resumes from — and
//! its reply goes to the messenger whole, with *typing…* until then (D5 (b)).
//! A chat never has two turns: what arrives meanwhile waits, and one turn
//! answers it when the running one lands. What the companion writes unasked in
//! a bridged chat — a follow-up, a background run's result — wakes a seat too,
//! so it reaches the phone (ADR 0013, *Consequences*).

use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::Instant;

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::entities::message::{Message, MessageFinish, MessageRole};
use crate::features::bridge::{
    Author, Body, BridgeAction, ChannelFailure, Inbound, InboundEvent, Incoming, MessengerCommand,
    Outbound, Pairing, Picture, PictureError,
};
use crate::shared::messenger::telegram::TelegramClient;
use crate::shared::secrets::SecretKey;

use super::bridge_task::{self, BotStart, BotTasks};
use super::generation::{StreamStep, TurnProgress};
use super::{InflightTurn, Orchestrator};

/// One profile's bot, as the orchestrator holds it.
pub(super) struct Bot {
    pub(super) tasks: BotTasks,
    /// What the bot was started with — its token and server, hashed. A change
    /// of either starts it again; the hash also tags what its tasks send, so a
    /// replaced bot's last words are not taken for the new one's.
    pub(super) key: u64,
    /// The bot's handle, once it answered: what the pairing link names.
    handle: Option<String>,
    /// The failure last told, so each is told once (rule 6).
    failure: Option<String>,
    /// The token was refused: the tasks have ended, and only a new token
    /// starts them again.
    dead: bool,
    /// The pairing code shown at the terminal, while it is good (rule 2).
    pairing: Option<Pairing>,
}

/// A messenger's turn, running in a seat of its own (D1).
pub(super) struct Seat {
    pub(super) generation: Uuid,
    profile: Uuid,
    /// Where the answer goes: the paired account.
    to: String,
    cancel: CancellationToken,
    /// The turn as a late opening of its chat resumes it.
    pub(super) turn: InflightTurn,
    /// The last error the turn told, for a reply that has no words.
    last_error: Option<String>,
    /// Stopped by `/stop` or `Esc`.
    stopped: bool,
    /// The turn answers a voice note: its reply goes back as one too, while
    /// the profile speaks its replies (ADR 0013 stage C, D8 (b)).
    voiced: bool,
}

/// What a landed seat turn leaves to say.
pub(super) struct SeatReply {
    text: Option<String>,
    finish: Option<MessageFinish>,
    silent: bool,
    woken: bool,
}

impl SeatReply {
    /// Read off the turn's rows before they move into the chat: the last
    /// assistant message's words and how it ended.
    pub(super) fn of(messages: &[Message], silent: bool, woken: bool) -> Self {
        let last = messages
            .iter()
            .rev()
            .find(|m| m.role == MessageRole::Assistant);
        Self {
            text: last
                .map(|m| m.text.trim().to_string())
                .filter(|t| !t.is_empty()),
            finish: last
                .and_then(|m| m.metadata.as_ref())
                .and_then(|md| md.finish),
            silent,
            woken,
        }
    }
}

/// What a bot is started with, hashed: its server, its token, and how it
/// prepares a picture — a change of any starts it again.
fn key_of(api: &str, token: &str, images: &crate::shared::config::ImageSettings) -> u64 {
    let mut h = DefaultHasher::new();
    (
        api,
        token,
        images.max_count,
        images.max_bytes,
        images.downscale_px,
    )
        .hash(&mut h);
    h.finish()
}

impl Orchestrator {
    // ---------- bots ----------

    /// The profile's bot token, from this machine's key store.
    fn bot_token(&self, profile: Uuid) -> Option<String> {
        crate::shared::secrets::stored_key(
            &self.config.api_keys,
            &SecretKey::BridgeToken(profile).storage_name(),
        )
        .filter(|t| !t.trim().is_empty())
    }

    /// Brings the running bots in line with the profiles and the key store: a
    /// bot for every visible profile with the switch on and a token, started
    /// again when its token or server changed, stopped when it is no longer
    /// wanted. Called at start and after every edit that could change that.
    pub(super) fn refresh_bridges(&mut self) {
        let api = self.config.bridge.telegram_api().to_string();
        let wanted: Vec<(Uuid, String)> = self
            .profiles
            .iter()
            .filter(|p| !p.is_hidden && p.companion.bridge)
            .filter_map(|p| Some((p.id, self.bot_token(p.id)?)))
            .collect();
        self.bots.retain(|profile, bot| {
            let keep = wanted.iter().any(|(p, _)| p == profile);
            if !keep {
                bot.tasks.cancel.cancel();
                tracing::info!(%profile, "bridge: the bot stopped");
            }
            keep
        });
        let images = self.config.images;
        for (profile, token) in wanted {
            let key = key_of(&api, &token, &images);
            if self.bots.get(&profile).is_some_and(|b| b.key == key) {
                continue;
            }
            if let Some(old) = self.bots.remove(&profile) {
                old.tasks.cancel.cancel();
            }
            let paired = self
                .profiles
                .iter()
                .find(|p| p.id == profile)
                .and_then(|p| p.bridge.account.clone());
            let tasks = bridge_task::spawn_telegram(
                BotStart {
                    profile,
                    bot: key,
                    paired,
                    images,
                    hearing: self.hearing(),
                    memo: self.stt_memo.clone(),
                },
                TelegramClient::new(&api, &token),
                self.bridge_tx.clone(),
            );
            tracing::info!(%profile, "bridge: the bot started");
            self.bots.insert(
                profile,
                Bot {
                    tasks,
                    key,
                    handle: None,
                    failure: None,
                    dead: false,
                    pairing: None,
                },
            );
        }
    }

    /// Every bot and every seat stopped (`Quit`).
    pub(super) fn stop_bridges(&mut self) {
        for bot in self.bots.values() {
            bot.tasks.cancel.cancel();
        }
        for seat in self.seats.values() {
            seat.cancel.cancel();
        }
    }

    fn bridge_out(&self, profile: Uuid, msg: Outbound) {
        if let Some(bot) = self.bots.get(&profile) {
            let _ = bot.tasks.out.send(msg);
        }
    }

    pub(super) fn bridge_send(&self, profile: Uuid, to: &str, text: impl Into<String>) {
        self.bridge_out(
            profile,
            Outbound::Send {
                to: to.to_string(),
                text: text.into(),
            },
        );
    }

    fn profile_name(&self, profile: Uuid) -> String {
        self.profiles
            .iter()
            .find(|p| p.id == profile)
            .map(|p| p.name.clone())
            .unwrap_or_default()
    }

    pub(super) fn profile_lang_locale(
        &self,
        profile: Uuid,
    ) -> &'static crate::shared::i18n::Locale {
        crate::shared::i18n::locale(
            self.profiles
                .iter()
                .find(|p| p.id == profile)
                .map(|p| p.language)
                .unwrap_or_default(),
        )
    }

    // ---------- what a bot says ----------

    /// One thing a bot's task said.
    pub(super) fn handle_bridge(&mut self, inbound: Inbound) {
        let Inbound {
            profile,
            bot,
            event,
        } = inbound;
        // A replaced or stopped bot's last words belong to nobody.
        if self.bots.get(&profile).is_none_or(|b| b.key != bot) {
            return;
        }
        match event {
            InboundEvent::Ready { handle } => {
                if let Some(bot) = self.bots.get_mut(&profile) {
                    bot.handle = Some(handle);
                    bot.failure = None;
                }
            }
            InboundEvent::Failed(failure) => self.bridge_failed(profile, failure),
            InboundEvent::Message(m) => self.bridge_message(profile, m),
        }
    }

    /// A failure, told once in the terminal (rule 6).
    fn bridge_failed(&mut self, profile: Uuid, failure: ChannelFailure) {
        let loc = self.ui_locale();
        let name = self.profile_name(profile);
        let text = match &failure {
            ChannelFailure::Unauthorized => {
                loc.tf("ui.bridge.err.unauthorized", &[("profile", &name)])
            }
            ChannelFailure::Conflict(why) => loc.tf(
                "ui.bridge.err.conflict",
                &[("profile", &name), ("reason", why)],
            ),
            ChannelFailure::Undelivered(err) => loc.tf(
                "ui.bridge.err.undelivered",
                &[("profile", &name), ("err", err)],
            ),
            ChannelFailure::Unvoiced(err) => loc.tf(
                "ui.bridge.err.unvoiced",
                &[("profile", &name), ("err", err)],
            ),
        };
        let Some(bot) = self.bots.get_mut(&profile) else {
            return;
        };
        if failure == ChannelFailure::Unauthorized {
            bot.dead = true;
        }
        if bot.failure.as_deref() == Some(text.as_str()) {
            return;
        }
        tracing::warn!(%profile, "bridge: {failure:?}");
        bot.failure = Some(text.clone());
        let _ = self.evt_tx.send(AppEvent::Error(text));
    }

    /// A message from someone. Only a pairing code is read from a stranger;
    /// everything else needs the paired account (rule 2).
    fn bridge_message(&mut self, profile: Uuid, m: Incoming) {
        let Some(p) = self.profiles.iter().find(|p| p.id == profile).cloned() else {
            return;
        };
        let loc = crate::shared::i18n::locale(p.language);
        if let Body::Command { name, arg } = &m.body
            && name == "start"
            && !arg.is_empty()
            && self
                .bots
                .get(&profile)
                .and_then(|b| b.pairing.as_ref())
                .is_some_and(|code| code.accepts(arg, Instant::now()))
        {
            self.bridge_pair(profile, &m);
            return;
        }
        if p.bridge.account.as_deref() != Some(m.account.as_str()) {
            // Nothing of a stranger's is kept, and nothing is answered: an
            // answer would tell them the bot is alive.
            tracing::info!(%profile, "bridge: dropped a message from an account that is not paired");
            return;
        }
        let to = m.account;
        match m.body {
            Body::Command { name, arg } => match MessengerCommand::read(&name, &arg) {
                MessengerCommand::Start(_) => {
                    self.bridge_send(profile, &to, loc.tf("bridge.hello", &[("name", &p.name)]));
                }
                MessengerCommand::New => {
                    let greeting = self.bridge_new_chat(profile).and_then(|chat| {
                        let c = self.chats.iter().find(|c| c.id == chat)?;
                        c.messages
                            .last()
                            .filter(|m| m.role == MessageRole::Assistant)
                            .map(|m| m.text.clone())
                    });
                    self.bridge_send(profile, &to, loc.t("bridge.new_chat"));
                    if let Some(greeting) = greeting {
                        self.bridge_send(profile, &to, greeting);
                    }
                }
                MessengerCommand::Stop => {
                    let stopped = p.bridge.chat.is_some_and(|chat| self.stop_seat(chat));
                    if !stopped {
                        self.bridge_send(profile, &to, loc.t("bridge.nothing_to_stop"));
                    }
                }
                MessengerCommand::Other => self.bridge_send(profile, &to, loc.t("bridge.commands")),
            },
            Body::Unsupported => self.bridge_send(profile, &to, loc.t("bridge.unsupported")),
            Body::Text(text) => self.bridge_say(profile, Message::user(&text)),
            Body::Forwarded { from, text } => {
                let text = crate::features::bridge::forwarded(from.as_deref(), &text, loc);
                self.bridge_say(profile, Message::user(&text));
            }
            Body::Images {
                pictures,
                caption,
                author,
            } => self.bridge_pictures(profile, &to, pictures, &caption, author),
            Body::Voice {
                voice,
                caption,
                author,
            } => self.bridge_voice_note(profile, &to, voice, &caption, author),
        }
    }

    /// Pictures from the phone: the ready ones on the person's message with its
    /// caption — as `/image attach` would have put them (spec §9.10) — and a
    /// line to the phone for each kind of picture that did not make it.
    fn bridge_pictures(
        &mut self,
        profile: Uuid,
        to: &str,
        pictures: Vec<Picture>,
        caption: &str,
        author: Author,
    ) {
        let loc = self.profile_lang_locale(profile);
        let mut images = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        for picture in pictures {
            let note = match picture {
                Picture::Ready(image) => {
                    images.push(*image);
                    continue;
                }
                Picture::Failed(PictureError::TooBig { bytes, limit }) => loc.tf(
                    "bridge.image_too_big",
                    &[("size", &megabytes(bytes)), ("limit", &megabytes(limit))],
                ),
                Picture::Failed(PictureError::Unreadable) => {
                    loc.t("bridge.image_unreadable").to_string()
                }
                Picture::Failed(PictureError::TooMany) => {
                    loc.t("bridge.image_too_many").to_string()
                }
                Picture::Failed(PictureError::Fetch(err)) => {
                    loc.tf("bridge.image_fetch_failed", &[("err", &err)])
                }
                // Sent in the moment its sender was paired, before the poller
                // knew: not fetched.
                Picture::File { .. } => loc.t("bridge.image_again").to_string(),
            };
            if !notes.contains(&note) {
                notes.push(note);
            }
        }
        if !notes.is_empty() {
            self.bridge_send(profile, to, notes.join("\n"));
        }
        // Nothing reached the model and nothing was said with it: no turn — a
        // forward's heading alone is not a message.
        if images.is_empty() && caption.trim().is_empty() {
            return;
        }
        let text = match author {
            Author::Person => caption.to_string(),
            Author::Forwarded(from) if caption.trim().is_empty() => match from {
                Some(name) => loc.tf("bridge.forwarded_from", &[("name", &name)]),
                None => loc.t("bridge.forwarded").to_string(),
            },
            Author::Forwarded(from) => {
                crate::features::bridge::forwarded(from.as_deref(), caption, loc)
            }
        };
        self.bridge_say(profile, Message::user(&text).with_images(images));
    }

    /// The pairing code arrived: its sender becomes the profile's one account.
    fn bridge_pair(&mut self, profile: Uuid, m: &Incoming) {
        if let Some(bot) = self.bots.get_mut(&profile) {
            bot.pairing = None;
            bot.tasks.paired.send_replace(Some(m.account.clone()));
        }
        let Some(p) = self.profiles.iter_mut().find(|p| p.id == profile) else {
            return;
        };
        p.bridge.account = Some(m.account.clone());
        p.bridge.account_name = Some(m.name.clone());
        let p = p.clone();
        if let Err(err) = self.storage.json().upsert_profile(&p) {
            tracing::warn!(%profile, error = %err, "bridge: the pairing was not saved");
        }
        tracing::info!(%profile, "bridge: an account was paired");
        let loc = crate::shared::i18n::locale(p.language);
        self.bridge_send(
            profile,
            &m.account,
            loc.tf("bridge.paired", &[("name", &p.name)]),
        );
        let _ = self.evt_tx.send(AppEvent::Notice(self.ui_locale().tf(
            "ui.bridge.paired",
            &[("profile", &p.name), ("account", &m.name)],
        )));
        self.emit_profile_list();
        self.emit_settings();
    }

    // ---------- the bridged conversation ----------

    /// The profile's bridged chat: the one it names, while it is still a
    /// visible chat of this profile — otherwise a new one (D2).
    fn bridged_chat(&mut self, profile: Uuid) -> Option<Uuid> {
        let named = self
            .profiles
            .iter()
            .find(|p| p.id == profile)
            .and_then(|p| p.bridge.chat);
        match named {
            Some(chat)
                if self
                    .chats
                    .iter()
                    .any(|c| c.id == chat && c.profile_id == profile) =>
            {
                Some(chat)
            }
            _ => self.bridge_new_chat(profile),
        }
    }

    /// `/new` in the messenger, or the first message: a new chat of the
    /// profile, now the bridged one. The previous stays in the list.
    fn bridge_new_chat(&mut self, profile: Uuid) -> Option<Uuid> {
        let chat = self.new_chat_value(Some(profile));
        let id = chat.id;
        if let Err(err) = self.storage.json().save_chat(&chat) {
            tracing::warn!(%profile, error = %err, "bridge: a new chat was not saved");
            return None;
        }
        self.chats.insert(0, chat);
        let p = self.profiles.iter_mut().find(|p| p.id == profile)?;
        p.bridge.chat = Some(id);
        let p = p.clone();
        if let Err(err) = self.storage.json().upsert_profile(&p) {
            tracing::warn!(%profile, error = %err, "bridge: the bridged chat was not saved");
        }
        self.emit_chat_list();
        Some(id)
    }

    /// The profile and account a chat answers to, when it is a live bridged
    /// chat: its profile's switch on, its bot running, an account paired, and
    /// this the chat the messenger reaches.
    pub(super) fn bridged(&self, chat: Uuid) -> Option<(Uuid, String)> {
        let c = self.chats.iter().find(|c| c.id == chat)?;
        let p = self.profiles.iter().find(|p| p.id == c.profile_id)?;
        let live = p.companion.bridge
            && p.bridge.chat == Some(chat)
            && self.bots.get(&p.id).is_some_and(|b| !b.dead);
        if !live {
            return None;
        }
        Some((p.id, p.bridge.account.clone()?))
    }

    /// The person's message, into the bridged chat — answered now, or after
    /// the turn running there lands. Made when it arrived, so its time is
    /// when it was sent.
    pub(super) fn bridge_say(&mut self, profile: Uuid, message: Message) {
        let Some(chat) = self.bridged_chat(profile) else {
            return;
        };
        if self.turn_running_in(chat) {
            self.bridge_waiting.entry(chat).or_default().push(message);
            return;
        }
        self.push_bridged(chat, vec![message]);
        self.start_seat(chat, false);
    }

    /// Rows from the messenger appended to `chat`; the open feed rebuilt so
    /// the person at the terminal sees them.
    fn push_bridged(&mut self, chat: Uuid, messages: Vec<Message>) {
        let Some(c) = self.chat_mut(chat) else {
            return;
        };
        for message in messages {
            c.push_message(message);
        }
        if self.active_id != Some(chat)
            && let Some(c) = self.chat_mut(chat)
        {
            c.unread = true;
        }
        self.mark_dirty(chat);
        self.emit_chat_list();
        if self.active_id == Some(chat) {
            self.activate(chat);
        }
    }

    // ---------- seats ----------

    /// Whether any turn runs in `chat`: the terminal's, or a seat's.
    pub(super) fn turn_running_in(&self, chat: Uuid) -> bool {
        self.inflight.as_ref().is_some_and(|t| t.chat == chat) || self.seats.contains_key(&chat)
    }

    /// Starts a seat turn on `chat`; a server that is not ready is said once,
    /// and the turn starts when it is ([`Self::bridge_engine_ready`]).
    fn start_seat(&mut self, chat: Uuid, woken: bool) {
        let Some((profile, to)) = self.bridged(chat) else {
            return;
        };
        let backend = match self.engines.backend_if_ready(self.ui_locale()) {
            Ok(backend) => backend,
            Err(_) => {
                if self.bridge_unanswered.insert(chat) && !woken {
                    let loc = self.profile_lang_locale(profile);
                    self.bridge_send(profile, &to, loc.t("bridge.not_ready"));
                }
                return;
            }
        };
        self.bridge_unanswered.remove(&chat);
        let first = self
            .chats
            .iter()
            .find(|c| c.id == chat)
            .is_some_and(|c| crate::features::rename_chat::is_first_user_message(&c.messages));
        let voiced = self.answers_voice(chat, woken);
        let Some((generation, cancel)) = self.start_seat_generation(chat, backend, woken) else {
            return;
        };
        self.bridge_out(
            profile,
            Outbound::Typing {
                to: to.clone(),
                on: true,
            },
        );
        self.seats.insert(
            chat,
            Seat {
                generation,
                profile,
                to,
                cancel,
                turn: InflightTurn {
                    generation,
                    chat,
                    rounds: Vec::new(),
                    partial: Default::default(),
                    children: Vec::new(),
                    continuation: false,
                },
                last_error: None,
                stopped: false,
                voiced,
            },
        );
        self.emit_chat_list();
        // The conversation's first message is titled from, as at the terminal
        // (spec §11.2) — after the reply's own request.
        if first {
            self.maybe_auto_title(chat, crate::shared::config::AutoTitleMode::AfterUserMessage);
        }
    }

    /// Stops the seat turn in `chat`; `false` when none runs.
    pub(super) fn stop_seat(&mut self, chat: Uuid) -> bool {
        let Some(seat) = self.seats.get_mut(&chat) else {
            return false;
        };
        seat.stopped = true;
        seat.cancel.cancel();
        true
    }

    /// The app wakes a bridged chat itself — a follow-up's notice or a
    /// background run's result is its last row — so the companion's words
    /// reach the phone. `false` when the chat is not a live bridged one.
    pub(super) fn bridge_wake(&mut self, chat: Uuid) -> bool {
        if self.bridged(chat).is_none() {
            return false;
        }
        if !self.turn_running_in(chat) {
            self.start_seat(chat, true);
        }
        true
    }

    /// A seat turn's event (everything but its stream steps): the screen gets
    /// it while the chat is open; an error is kept for the reply.
    pub(super) fn handle_seat_event(&mut self, generation: Uuid, event: AppEvent) {
        let Some(seat) = self.seats.values_mut().find(|s| s.generation == generation) else {
            return;
        };
        if let AppEvent::Error(text) = &event {
            seat.last_error = Some(text.clone());
        }
        if self.active_id == Some(seat.turn.chat) {
            let _ = self.evt_tx.send(event);
        }
    }

    /// A seat turn's progress, into its mirror and — while the chat is open —
    /// onto the screen. `false` when `id` is no seat's.
    pub(super) fn handle_seat_progress(&mut self, id: Uuid, progress: TurnProgress) -> bool {
        let Some(chat) = self
            .seats
            .iter()
            .find(|(_, s)| s.generation == id)
            .map(|(chat, _)| *chat)
        else {
            return false;
        };
        let open = self.active_id == Some(chat);
        match progress {
            TurnProgress::RoundFiled(messages) => {
                if let Some(seat) = self.seats.get_mut(&chat) {
                    seat.turn.rounds.extend(messages);
                    seat.turn.partial = Default::default();
                }
            }
            TurnProgress::OwnStep(step) => {
                if let Some(seat) = self.seats.get_mut(&chat) {
                    super::apply_step(&mut seat.turn.partial, &step);
                }
                if open {
                    let _ = self.evt_tx.send(StreamStep::into_event(step, id));
                }
            }
            TurnProgress::BackgroundStart(start) => self.spawn_background_run(*start, chat),
            // A sub-agent inside a seat's turn lands with it; it has no live
            // transcript while it runs.
            _ => {}
        }
        true
    }

    /// The seat whose turn this result is, taken out.
    pub(super) fn take_seat(&mut self, generation: Uuid) -> Option<Seat> {
        let chat = self
            .seats
            .iter()
            .find(|(_, s)| s.generation == generation)
            .map(|(chat, _)| *chat)?;
        self.seats.remove(&chat)
    }

    /// The landed seat's reply, to the messenger whole (D5 (b)): its words, a
    /// stopped one marked so; a silence says nothing; a turn with no words
    /// says why, unless nobody asked for it.
    pub(super) fn answer_seat(&mut self, seat: Seat, reply: SeatReply) {
        self.bridge_out(
            seat.profile,
            Outbound::Typing {
                to: seat.to.clone(),
                on: false,
            },
        );
        self.emit_chat_list();
        if reply.silent {
            return;
        }
        let loc = self.profile_lang_locale(seat.profile);
        let stopped = seat.stopped || reply.finish == Some(MessageFinish::Cancelled);
        // The words a voice note says back: a whole reply, to a voice note.
        let spoken = reply.text.clone().filter(|_| seat.voiced && !stopped);
        let text = match reply.text {
            Some(text) if stopped => format!("{text}\n\n_{}_", loc.t("bridge.stopped_mark")),
            Some(text) => text,
            None if stopped => loc.t("bridge.stopped").to_string(),
            None if reply.woken => return,
            None => loc.tf(
                "bridge.failed",
                &[(
                    "err",
                    seat.last_error
                        .as_deref()
                        .unwrap_or_else(|| loc.t("bridge.no_reply")),
                )],
            ),
        };
        self.bridge_send(seat.profile, &seat.to, text);
        if let Some(words) = spoken {
            self.bridge_voice_reply(seat.profile, &seat.to, &words);
        }
    }

    /// After any turn lands in `chat`: what the messenger sent meanwhile is
    /// answered now, and a notice the companion was woken on meanwhile too.
    pub(super) fn bridge_after_landing(&mut self, chat: Uuid) {
        if self.turn_running_in(chat) {
            return;
        }
        if let Some(waiting) = self.bridge_waiting.remove(&chat).filter(|t| !t.is_empty()) {
            self.push_bridged(chat, waiting);
            self.start_seat(chat, false);
            return;
        }
        let notice = self
            .chats
            .iter()
            .find(|c| c.id == chat)
            .and_then(|c| c.messages.last())
            .is_some_and(|m| m.is_followup_notice() || m.is_notification());
        if notice {
            self.bridge_wake(chat);
        }
    }

    /// The engine came up: the bridged chats that were told it was not ready
    /// are answered now.
    pub(super) fn bridge_engine_ready(&mut self) {
        let chats: Vec<Uuid> = self.bridge_unanswered.iter().copied().collect();
        for chat in chats {
            if !self.turn_running_in(chat) {
                let woken = self
                    .chats
                    .iter()
                    .find(|c| c.id == chat)
                    .and_then(|c| c.messages.last())
                    .is_some_and(|m| m.is_followup_notice() || m.is_notification());
                self.start_seat(chat, woken);
            }
        }
    }

    /// Whether the open chat has a seat turn running — what the terminal's own
    /// sends, regenerations and deletions wait for. Says so when it does.
    pub(super) fn seat_holds_open_chat(&self) -> bool {
        let held = self.active_id.is_some_and(|c| self.seats.contains_key(&c));
        if held {
            let _ = self.evt_tx.send(AppEvent::Notice(
                self.ui_locale().t("ui.bridge.busy").to_string(),
            ));
        }
        held
    }

    /// The mirror of the seat running in `chat`, for its activation.
    pub(super) fn seat_live_turn(&self, chat: Uuid) -> Option<Box<crate::app::events::LiveTurn>> {
        let seat = self.seats.get(&chat)?;
        Some(Box::new(crate::app::events::LiveTurn {
            turn: seat.generation,
            stream: seat.generation,
            role: MessageRole::Assistant,
            partial: Some(seat.turn.partial.clone()),
            continues: false,
            background: false,
        }))
    }

    /// The seat's filed rounds, for its chat's activation.
    pub(super) fn seat_rounds(&self, chat: Uuid) -> Vec<Message> {
        self.seats
            .get(&chat)
            .map(|s| s.turn.rounds.clone())
            .unwrap_or_default()
    }

    // ---------- the terminal's `/bridge` ----------

    /// `/bridge [pair|unpair]` at the terminal, for the open chat's profile.
    pub(super) fn handle_bridge_action(&mut self, action: BridgeAction) {
        let loc = self.ui_locale();
        let notice = |text: String| AppEvent::Notice(text);
        let Some(profile) = self.active_profile_id() else {
            return;
        };
        let Some(p) = self.profiles.iter().find(|p| p.id == profile).cloned() else {
            return;
        };
        let name = p.name.as_str();
        if !p.companion.bridge {
            let _ = self
                .evt_tx
                .send(notice(loc.tf("ui.bridge.off", &[("profile", name)])));
            return;
        }
        if self.bot_token(profile).is_none() {
            let _ = self
                .evt_tx
                .send(notice(loc.tf("ui.bridge.no_token", &[("profile", name)])));
            return;
        }
        match action {
            BridgeAction::Status => {
                let bot = self.bots.get(&profile);
                let state = match bot {
                    Some(b) if b.dead => loc.t("ui.bridge.state.refused").to_string(),
                    Some(b) => match (&b.failure, &b.handle) {
                        (Some(failure), _) => failure.clone(),
                        (None, Some(handle)) => {
                            loc.tf("ui.bridge.state.running", &[("bot", handle)])
                        }
                        (None, None) => loc.t("ui.bridge.state.starting").to_string(),
                    },
                    None => loc.t("ui.bridge.state.starting").to_string(),
                };
                let account = p
                    .bridge
                    .account_name
                    .clone()
                    .or_else(|| p.bridge.account.clone())
                    .unwrap_or_else(|| loc.t("ui.bridge.state.unpaired").to_string());
                let chat = p
                    .bridge
                    .chat
                    .and_then(|c| self.chats.iter().find(|x| x.id == c))
                    .map(|c| c.title.clone())
                    .unwrap_or_else(|| "—".to_string());
                let _ = self.evt_tx.send(notice(loc.tf(
                    "ui.bridge.status",
                    &[
                        ("profile", name),
                        ("state", &state),
                        ("account", &account),
                        ("chat", &chat),
                    ],
                )));
            }
            BridgeAction::Pair => {
                let Some(bot) = self.bots.get_mut(&profile) else {
                    return;
                };
                if bot.dead {
                    let _ = self.evt_tx.send(AppEvent::Error(
                        loc.tf("ui.bridge.err.unauthorized", &[("profile", name)]),
                    ));
                    return;
                }
                let Some(handle) = bot.handle.clone() else {
                    let _ = self
                        .evt_tx
                        .send(notice(loc.t("ui.bridge.not_answered").to_string()));
                    return;
                };
                let pairing = Pairing::new(Instant::now());
                let link = crate::features::bridge::pairing_link(&handle, &pairing.code);
                let text = loc.tf(
                    "ui.bridge.code",
                    &[
                        ("profile", name),
                        ("bot", &handle),
                        ("code", &pairing.code),
                        ("link", &link),
                    ],
                );
                bot.pairing = Some(pairing);
                let _ = self.evt_tx.send(notice(text));
            }
            BridgeAction::Unpair => {
                let Some(p) = self.profiles.iter_mut().find(|p| p.id == profile) else {
                    return;
                };
                p.bridge.account = None;
                p.bridge.account_name = None;
                let p = p.clone();
                if let Some(bot) = self.bots.get(&profile) {
                    bot.tasks.paired.send_replace(None);
                }
                if let Err(err) = self.storage.json().upsert_profile(&p) {
                    tracing::warn!(%profile, error = %err, "bridge: the unpairing was not saved");
                }
                let _ = self
                    .evt_tx
                    .send(notice(loc.tf("ui.bridge.unpaired", &[("profile", name)])));
                self.emit_profile_list();
                self.emit_settings();
            }
        }
    }
}

/// Bytes as megabytes, one decimal — what a person reads a size in.
pub(super) fn megabytes(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / (1024.0 * 1024.0))
}

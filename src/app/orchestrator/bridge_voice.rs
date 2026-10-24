//! Voice notes in the bridge (ADR 0013, stage C; spec §11.13): what the bots
//! hear them with, what a heard one becomes in the conversation, and a reply
//! that goes back as one.
//!
//! - **In.** A bot's poller fetches and hears a note itself, by the
//!   recognition settings this side keeps it told of ([`Orchestrator::hearing`],
//!   a `watch` per bot). What arrives here is the transcript, which becomes the
//!   person's message marked as spoken (D7 (a)); or why there is none, said to
//!   the phone in one line — and in the terminal once, when it is the settings'
//!   doing (rule 4).
//! - **Out.** A reply to a voice note goes back as one too, while the profile
//!   speaks its replies (D8 (b)) — after its text, which is not held for the
//!   synthesis (D9 (a)). It is synthesised as the terminal would speak it, in
//!   the profile's voice, made mono, and written once as OGG/Opus
//!   (`shared::voice_note`), whatever the speech mode answered in (rule 1).

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::entities::message::{Message, MessageRole};
use crate::features::bridge::{
    Author, ChannelFailure, Inbound, InboundEvent, Outbound, Voice, VoiceError,
};
use crate::features::tts_command::TtsScope;
use crate::shared::config::SecretSlot;
use crate::shared::stt::{Hearing, SttSetupError};
use crate::shared::tts::{TtsEngine, engines_from_config};
use crate::shared::voice_note;

use super::Orchestrator;
use super::bridge::megabytes;
use super::tts::{build_utterances, chunk_utterances, setup_error_key};

impl Orchestrator {
    /// What the bots hear a voice note with: the recognition settings, the key
    /// they read (a cloud provider's, shared with chat, or the `external`
    /// slot's own — ADR 0008), and the gateway's attribution switch.
    pub(super) fn hearing(&self) -> Hearing {
        let key = self.config.stt.secret_key().and_then(|k| {
            crate::shared::secrets::stored_key(&self.config.api_keys, &k.storage_name())
        });
        Hearing {
            stt: self.config.stt.clone(),
            key,
            attribution: self.config.openrouter.attribution,
        }
    }

    /// Every running bot told the recognition settings as they stand now.
    /// After a settings or key edit: a failure said before is news again.
    pub(super) fn refresh_hearing(&mut self) {
        let hearing = self.hearing();
        for bot in self.bots.values() {
            bot.tasks.hearing.send_if_modified(|current| {
                let changed = *current != hearing;
                if changed {
                    current.clone_from(&hearing);
                }
                changed
            });
        }
        self.stt_failed = false;
    }

    /// A voice note from the paired account: its transcript, marked as
    /// spoken, is the person's message (D7 (a)); a note that was not heard is
    /// said to the phone, and the words under it still go in.
    pub(super) fn bridge_voice_note(
        &mut self,
        profile: Uuid,
        to: &str,
        voice: Voice,
        caption: &str,
        author: Author,
    ) {
        let loc = self.profile_lang_locale(profile);
        let (heard, seconds) = match voice {
            Voice::Heard { text, seconds } => (Some(text), seconds),
            Voice::Failed { error, seconds } => {
                self.voice_unheard(profile, to, error);
                (None, seconds)
            }
            // Sent in the moment its sender was paired, before the poller
            // knew: not fetched.
            Voice::File { seconds, .. } => {
                self.bridge_send(profile, to, loc.t("bridge.voice_again"));
                (None, seconds)
            }
        };
        let caption = caption.trim();
        let words = match (&heard, caption.is_empty()) {
            (Some(text), true) => text.clone(),
            (Some(text), false) => format!("{text}\n\n{caption}"),
            (None, false) => caption.to_string(),
            // Nothing heard and nothing written: no turn.
            (None, true) => return,
        };
        let words = match author {
            Author::Person => words,
            Author::Forwarded(from) => {
                crate::features::bridge::forwarded(from.as_deref(), &words, loc)
            }
        };
        let message = Message::user(&words);
        let message = match heard {
            Some(_) => message.with_voice(seconds),
            None => message,
        };
        self.bridge_say(profile, message);
    }

    /// Why a note was not heard: one line to the phone, and — when it is the
    /// settings' or the recogniser's doing — the terminal told once, until the
    /// settings change (rule 4).
    fn voice_unheard(&mut self, profile: Uuid, to: &str, error: VoiceError) {
        let loc = self.profile_lang_locale(profile);
        let ui = self.ui_locale();
        let name = self
            .profiles
            .iter()
            .find(|p| p.id == profile)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let (phone, terminal) = match error {
            VoiceError::NotSetUp(why) => (
                loc.t("bridge.voice_not_set_up").to_string(),
                Some(ui.tf(
                    "ui.bridge.err.stt_setup",
                    &[("profile", &name), ("reason", ui.t(stt_setup_key(why)))],
                )),
            ),
            VoiceError::TooBig { bytes, limit } => (
                loc.tf(
                    "bridge.voice_too_big",
                    &[("size", &megabytes(bytes)), ("limit", &megabytes(limit))],
                ),
                None,
            ),
            VoiceError::Fetch(err) => (loc.tf("bridge.voice_fetch_failed", &[("err", &err)]), None),
            VoiceError::Unheard(err) => (
                loc.tf("bridge.voice_unheard", &[("err", &err)]),
                Some(ui.tf(
                    "ui.bridge.err.stt_refused",
                    &[("profile", &name), ("err", &err)],
                )),
            ),
            VoiceError::Nothing => (loc.t("bridge.voice_nothing").to_string(), None),
        };
        self.bridge_send(profile, to, phone);
        if let Some(text) = terminal
            && !self.stt_failed
        {
            self.stt_failed = true;
            tracing::warn!(%profile, "bridge: a voice note was not heard");
            let _ = self.evt_tx.send(AppEvent::Error(text));
        }
    }

    /// Whether the turn about to start in `chat` answers a voice note (D8 (b)):
    /// the person's last message came as one, and nobody but the person woke
    /// the turn — what the companion writes unasked stays text.
    pub(super) fn answers_voice(&self, chat: Uuid, woken: bool) -> bool {
        !woken
            && self
                .chats
                .iter()
                .find(|c| c.id == chat)
                .and_then(|c| c.messages.last())
                .is_some_and(|m| m.role == MessageRole::User && m.voice.is_some())
    }

    /// A seat's reply, again as a voice note (D8 (b), D9 (a)): while the
    /// profile speaks its replies, synthesised off the orchestrator's thread
    /// and sent after the text, which has gone already. A reply with nothing to
    /// say aloud (a code block) goes as text alone.
    pub(super) fn bridge_voice_reply(&mut self, profile: Uuid, to: &str, text: &str) {
        let Some(p) = self.profiles.iter().find(|p| p.id == profile) else {
            return;
        };
        if !p.companion.speak_replies {
            return;
        }
        let voice = p.voice_for(self.config.tts.mode).map(str::to_string);
        let speech_loc = self.profile_locale(profile);
        let reply = Message::assistant(text);
        let Some(utterances) = build_utterances(
            std::slice::from_ref(&reply),
            TtsScope::Last,
            false,
            speech_loc,
        ) else {
            return;
        };
        let tts = self.config.tts.with_voice(voice.as_deref());
        let stored = self.config.tts.secret_key().and_then(|k| {
            crate::shared::secrets::stored_key(&self.config.api_keys, &k.storage_name())
        });
        let engine = match engines_from_config(&tts, stored, &self.speech_gateway()) {
            Ok((engine, _)) => engine,
            Err(err) => {
                let msg = self.ui_locale().t(setup_error_key(err)).to_string();
                self.fail_speech(&msg, true);
                return;
            }
        };
        let chunks: Vec<String> = chunk_utterances(&utterances, engine.max_input_chars())
            .into_iter()
            .map(|(_, chunk)| chunk)
            .collect();
        let Some(bot) = self.bots.get(&profile) else {
            return;
        };
        let out = bot.tasks.out.clone();
        let cancel = bot.tasks.cancel.child_token();
        let inbound = self.bridge_tx.clone();
        let bot_key = bot.key;
        let to = to.to_string();
        let _ = out.send(Outbound::Recording {
            to: to.clone(),
            on: true,
        });
        tokio::spawn(async move {
            let made = tokio::select! {
                _ = cancel.cancelled() => return,
                made = make_note(engine, chunks, cancel.clone()) => made,
            };
            match made {
                Ok((note, seconds)) => {
                    let _ = out.send(Outbound::Voice { to, note, seconds });
                }
                Err(err) => {
                    tracing::warn!(%profile, error = %err, "bridge: a reply's voice note was not made");
                    let _ = out.send(Outbound::Recording { to, on: false });
                    let _ = inbound.send(Inbound {
                        profile,
                        bot: bot_key,
                        event: InboundEvent::Failed(ChannelFailure::Unvoiced(format!("{err:#}"))),
                    });
                }
            }
        });
    }
}

/// A reply's chunks synthesised in turn, made mono at one rate, and written as
/// one voice note: the note and its length in seconds.
async fn make_note(
    engine: Box<dyn TtsEngine>,
    chunks: Vec<String>,
    cancel: CancellationToken,
) -> Result<(Vec<u8>, u32)> {
    let mut pieces: Vec<(Vec<i16>, u32)> = Vec::new();
    for chunk in &chunks {
        let clip = engine.synthesize(chunk, &cancel).await?;
        if clip.is_empty() {
            continue;
        }
        let piece = tokio::task::spawn_blocking(move || clip.into_mono()).await??;
        pieces.push(piece);
    }
    let rate = pieces
        .iter()
        .map(|(_, rate)| *rate)
        .max()
        .context("the speech server returned no sound")?;
    let samples: Vec<i16> = pieces
        .into_iter()
        .flat_map(|(samples, from)| voice_note::resample(&samples, from, rate))
        .collect();
    let seconds = (samples.len() as u32).div_ceil(rate).max(1);
    let note = tokio::task::spawn_blocking(move || voice_note::encode(&samples, rate)).await??;
    Ok((note, seconds))
}

/// Bundle key for a structured recognition-setup error.
fn stt_setup_key(err: SttSetupError) -> &'static str {
    match err {
        SttSetupError::Model => "ui.err.stt_no_model",
        SttSetupError::ApiKey => "ui.err.stt_no_api_key",
        SttSetupError::Url => "ui.err.stt_no_url",
    }
}

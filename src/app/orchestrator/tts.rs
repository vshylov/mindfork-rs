//! Chat message speech (`/tts [N|all|stop]` command, spec §11.9).
//!
//! Modeled on [`super::rag`]: command → a data snapshot from the orchestrator
//! (the sole owner of `Chat`) → a cancellable background task. The task runs as
//! a **pipeline**: chunks are synthesized one after another while the earlier
//! ones play, as long as less than [`LOOKAHEAD`] of sound is queued — enough to
//! hide the next chunk's synthesis, and no more paid for than a stop could
//! throw away. The chat isn't mutated, generation isn't gated: a **snapshot**
//! of the conversation at command time is spoken.
//!
//! A message's blocks — paragraphs, headings, list items, the note of a
//! skipped code block — are packed into chunks of a few hundred characters
//! ([`chunk_utterances`]): one request per block made a short block's sound
//! too short to hide a long one's synthesis, and the reply fell silent in the
//! middle (docs/research/tts-pauses.md). Every chunk and every run leaves its
//! timings in the log — synthesis, sound, and any silence the listener heard.
//!
//! Stop points are collected into one helper [`Orchestrator::stop_tts`]
//! (precedent — `reset_rag_cancel`): two by setting (switching chats, starting
//! generation) and three **unconditional** — deleting an exchange,
//! regeneration, deleting a chat: the text being spoken no longer exists. See
//! docs/research/tts.md §7, §8.
//!
//! A profile may ask for each reply to be spoken as it lands
//! (`speak_replies`, [`Orchestrator::speak_landed_reply`]), in a voice of its
//! own under the active speech mode (`Profile::voices`) — the same road as the
//! command, on the one message (spec §11.9, docs/research/companion-platform.md
//! C7 (a)).

use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::entities::message::{Message, MessageRole};
use crate::features::tts_command::TtsScope;
use crate::shared::config::SecretSlot;
use crate::shared::i18n::Locale;
use crate::shared::tts::{
    GatewaySpeech, TtsEngine, TtsSetupError, engines_from_config,
    playback::{Playback, SpeechQueue},
};

use super::Orchestrator;

/// How much sound to keep queued ahead of the listener: the next chunk is
/// synthesized as soon as less than this is left. It has to outlast one
/// chunk's synthesis — a 400-character chunk is ~31 s of sound, and at the
/// slowest throughput measured (0.9× real time) takes ~34 s — and it bounds
/// what a stop throws away: this, plus the chunk in flight
/// (docs/research/tts-pauses.md F2).
const LOOKAHEAD: Duration = Duration::from_secs(40);

/// Playback-queue poll interval (`rodio` has no async "queue is empty"
/// notification, and `sleep_until_end` is blocking). A silence is measured to
/// this precision.
const POLL_INTERVAL: Duration = Duration::from_millis(80);

/// A silence shorter than this is a seam between chunks, not a pause anyone
/// hears; the run's summary counts only the longer ones.
const AUDIBLE_SILENCE: Duration = Duration::from_millis(250);

/// The soft size of a run's first chunk, in characters: the first sound waits
/// for its whole synthesis (docs/research/tts-pauses.md F5).
const FIRST_CHUNK_CHARS: usize = 200;

/// How much each later chunk's soft size grows, up to [`CHUNK_CHARS`]: chunk 2
/// is made while chunk 1 plays, and a chunk twice as long as the one hiding it
/// breaks even only at twice real time.
const CHUNK_STEP_CHARS: usize = 100;

/// The soft size of every chunk once the run is under way.
const CHUNK_CHARS: usize = 400;

/// Pause after the queue drains — before closing the device (dropping
/// [`Playback`]). The queue reports empty once the source is exhausted by the
/// **mixer**; the sound card is still playing out its buffer at that point, and
/// without this pause the last chunk's tail would get cut off. The value has
/// margin over the measured gap (~30 ms).
const DRAIN_TAIL: Duration = Duration::from_millis(300);

impl Orchestrator {
    /// Speaks the active chat's messages (`/tts`, `/tts N`, `/tts all` commands).
    pub(super) fn handle_tts(&mut self, scope: TtsScope) {
        // A new command always interrupts the previous playback.
        self.stop_tts();
        // A chat, or a sub-agent transcript — spoken like the chat it looks
        // like, in its parent's profile language.
        let Some((profile_id, messages)) = self.active_id.and_then(|id| match self.view(id)? {
            super::ChatView::Top(chat) => Some((chat.profile_id, chat.messages.clone())),
            super::ChatView::Child { parent, run } => {
                Some((parent.profile_id, run.messages.clone()))
            }
        }) else {
            self.fail_tts(self.ui_locale().t("ui.err.tts_no_active_chat"));
            return;
        };
        self.speak(profile_id, &messages, scope, false);
    }

    /// A reply that just landed in the open conversation, spoken because its
    /// profile asks for every reply to be (`speak_replies`, spec §11.9) — the
    /// same road as `/tts` on that one message, so what it says, in which voice
    /// and how it stops are the command's. A conversation not in view is not
    /// spoken over the one that is.
    pub(super) fn speak_landed_reply(&mut self, chat_id: Uuid, reply: Uuid) {
        if self.active_id != Some(chat_id) {
            return;
        }
        let Some(chat) = self.chats.iter().find(|c| c.id == chat_id) else {
            return;
        };
        let profile_id = chat.profile_id;
        let wanted = self
            .profiles
            .iter()
            .find(|p| p.id == profile_id)
            .is_some_and(|p| p.companion.speak_replies);
        if !wanted {
            return;
        }
        let Some(message) = chat.messages.iter().find(|m| m.id == reply).cloned() else {
            return;
        };
        self.stop_tts();
        self.speak(profile_id, &[message], TtsScope::Last, true);
    }

    /// Speaks `messages` (a snapshot) in `profile_id`'s voice. `auto` — a reply
    /// spoken unasked: a failure to set speech up is said once and then kept
    /// quiet until the settings change, rather than after every reply, and a
    /// reply with nothing to say aloud (a code block) is passed over.
    fn speak(&mut self, profile_id: Uuid, messages: &[Message], scope: TtsScope, auto: bool) {
        // Markers and role prefixes are speech content, so they're in the
        // **profile's** language (axis A, docs/history/i18n.md), not the
        // interface's.
        let speech_loc = self.profile_locale(profile_id);
        let chunks =
            match build_utterances(messages, scope, self.config.tts.speak_roles, speech_loc) {
                Some(text) => text,
                None if auto => return,
                None => {
                    self.fail_tts(self.ui_locale().t("ui.err.tts_nothing_to_speak"));
                    return;
                }
            };
        // The profile's own voice under the active mode, over the settings'
        // (spec §11.9).
        let voice = self
            .profiles
            .iter()
            .find(|p| p.id == profile_id)
            .and_then(|p| p.voice_for(self.config.tts.mode))
            .map(str::to_string);
        let tts = self.config.tts.with_voice(voice.as_deref());

        // Build the client from a settings snapshot: in a cloud mode the stored
        // provider key is shared with chat (ADR 0008) — no need to enter it again;
        // in `external` mode it is the speech slot's own key
        // (docs/history/external-api-key.md §3).
        let stored = self.config.tts.secret_key().and_then(|k| {
            crate::shared::secrets::stored_key(&self.config.api_keys, &k.storage_name())
        });
        let gateway = self.speech_gateway();
        // Two engines: the assistant's and (opt.) the user's — when a separate
        // "User voice" is set (spec §11.9). Both use the same provider → a shared
        // limit.
        let (engine, user_engine) = match engines_from_config(&tts, stored, &gateway) {
            Ok(pair) => pair,
            Err(err) => {
                self.fail_speech(self.ui_locale().t(setup_error_key(err)), auto);
                return;
            }
        };
        let chunks = chunk_utterances(&chunks, engine.max_input_chars());
        if chunks.is_empty() {
            if !auto {
                self.fail_tts(self.ui_locale().t("ui.err.tts_nothing_to_speak"));
            }
            return;
        }

        // We open the device here (not in the task) to share `Arc<Playback>` with
        // the orchestrator for instant `/tts pause`/`resume`. Opening is still
        // **lazy** — on the `/tts` command, not at app startup (cpal#384). No
        // audio → a clear note, the task isn't spawned.
        let playback = match Playback::open() {
            Ok(p) => std::sync::Arc::new(p),
            Err(err) => {
                let msg = self
                    .ui_locale()
                    .tf("ui.err.tts_no_audio", &[("err", &err.to_string())]);
                self.fail_speech(&msg, auto);
                return;
            }
        };
        // Speech works again: a later failure of a spoken reply is news.
        self.auto_speech_failed = false;

        let cancel = CancellationToken::new();
        let task_id = Uuid::new_v4();
        self.tts_cancel = Some(cancel.clone());
        self.tts_gen = Some(task_id);
        self.tts_playback = Some(playback.clone());
        let _ = self.evt_tx.send(AppEvent::TtsActive(true));
        tracing::info!(
            chunks = chunks.len(),
            chars = chunks.iter().map(|(_, c)| c.chars().count()).sum::<usize>(),
            auto,
            "speech: a run starts"
        );
        spawn_tts(TtsTask {
            engine,
            user_engine,
            chunks,
            cancel,
            task_id,
            playback,
            loc: self.ui_locale(),
            evt_tx: self.evt_tx.clone(),
            done_tx: self.tts_done_tx.clone(),
        });
    }

    /// What a speech client of the gateway's is built with besides the slot's
    /// settings, and is the gateway's own rather than the slot's: the
    /// provider-wide switch as it stands now, and what this session has learned
    /// about the gateway's models — the same memo for every command.
    pub(super) fn speech_gateway(&self) -> GatewaySpeech {
        GatewaySpeech {
            attribution: self.config.openrouter.attribution,
            formats: self.tts_formats.clone(),
        }
    }

    /// Pauses the current playback (`/tts pause`), keeping the queue. No-op if
    /// nothing is speaking. Resumed via [`Self::handle_tts_resume`].
    pub(super) fn handle_tts_pause(&mut self) {
        if let Some(pb) = &self.tts_playback {
            pb.pause();
        }
    }

    /// Continues paused playback (`/tts resume`). No-op if nothing is speaking
    /// or it's already playing.
    pub(super) fn handle_tts_resume(&mut self) {
        if let Some(pb) = &self.tts_playback {
            pb.resume();
        }
    }

    /// Stops speech (the `/tts stop` command and every stop point).
    /// Idempotent: a no-op if nothing is playing.
    pub(super) fn stop_tts(&mut self) {
        if let Some(token) = self.tts_cancel.take() {
            token.cancel();
        }
        // Release the device handle (the task holds its own `Arc` until it
        // finishes). Also lift the pause: otherwise a paused task would never
        // drain after cancellation.
        if let Some(pb) = self.tts_playback.take() {
            pb.resume();
        }
        // Clear the chip right away and forget the generation: a late `done`
        // from an already-cancelled task will be discarded (otherwise it would
        // clear the chip of a new speech run).
        if self.tts_gen.take().is_some() {
            let _ = self.evt_tx.send(AppEvent::TtsActive(false));
        }
    }

    /// The background speech task finished on its own (played out / errored).
    /// Clears the chip only if this is the current task.
    pub(super) fn handle_tts_done(&mut self, task_id: Uuid) {
        if self.tts_gen == Some(task_id) {
            self.tts_gen = None;
            self.tts_cancel = None;
            self.tts_playback = None;
            let _ = self.evt_tx.send(AppEvent::TtsActive(false));
        }
    }

    /// Reports a speech error as a feed note (interface language, axis B).
    fn fail_tts(&self, msg: &str) {
        let _ = self.evt_tx.send(AppEvent::Error(msg.to_string()));
    }

    /// [`Self::fail_tts`] for a speech attempt; one the person did not ask for
    /// (`auto`) says its failure once — every reply would repeat it — and stays
    /// quiet until the settings or a profile change, or speech works again.
    pub(super) fn fail_speech(&mut self, msg: &str, auto: bool) {
        if auto {
            if self.auto_speech_failed {
                return;
            }
            self.auto_speech_failed = true;
        }
        self.fail_tts(msg);
    }
}

/// Bundle key for a structured speech-setup error.
pub(super) fn setup_error_key(err: TtsSetupError) -> &'static str {
    match err {
        TtsSetupError::Model => "ui.err.tts_no_model",
        TtsSetupError::ApiKey => "ui.err.tts_no_api_key",
        TtsSetupError::Url => "ui.err.tts_no_url",
    }
}

/// Selects messages by the command's scope and turns them into utterances for
/// synthesis.
///
/// What counts as a message: `user`/`assistant` with non-empty text (system/
/// tool are skipped — the same rules as in the `F5` export). Order is
/// chronological. `None` — nothing to speak (an empty chat / only service
/// messages).
pub(super) fn build_utterances(
    messages: &[Message],
    scope: TtsScope,
    speak_roles: bool,
    loc: &'static Locale,
) -> Option<Vec<(MessageRole, String)>> {
    let spoken: Vec<&Message> = messages
        .iter()
        .filter(|m| matches!(m.role, MessageRole::User | MessageRole::Assistant))
        .filter(|m| !m.text.trim().is_empty())
        .collect();
    let take = match scope {
        TtsScope::Last => 1,
        TtsScope::Recent(n) => n,
        TtsScope::All => spoken.len(),
    };
    let start = spoken.len().saturating_sub(take);
    // The role travels with the text: multi-voice speech (`/tts all`) picks the
    // user's/assistant's voice by it.
    let out: Vec<(MessageRole, String)> = spoken[start..]
        .iter()
        .filter_map(|m| {
            // Thoughts (CoT) and tool blocks are never spoken: the former live in
            // a separate `Message.thoughts` field, the latter aren't in `text`.
            let text = crate::shared::markdown::speakable_text(&m.text, loc);
            if text.is_empty() {
                return None;
            }
            let text = if speak_roles {
                let role = match m.role {
                    MessageRole::User => loc.t("speak.role.user"),
                    _ => loc.t("speak.role.assistant"),
                };
                format!("{role} {text}")
            } else {
                text
            };
            Some((m.role, text))
        })
        .collect();
    (!out.is_empty()).then_some(out)
}

/// Cuts utterances into chunks for synthesis — packing a message's blocks
/// together, sentence by sentence, up to a soft size (docs/research/tts-pauses.md
/// F4, F5).
///
/// - **Blocks are packed.** A block — a paragraph, a heading, a list item, the
///   note of a skipped code block — is one line of the speakable text; the
///   blocks of one message share chunks, a line break kept where a block ends
///   (the pause the speech extractor puts there for every engine). So a heading
///   travels with its paragraph and a list's items go together.
/// - **A seam falls on a sentence boundary**, reusing RAG's sentence splitter:
///   a chunk is closed before the sentence that would take it past its size.
/// - **The size is soft and grows**: the first chunk of the run aims at
///   [`FIRST_CHUNK_CHARS`], each next one [`CHUNK_STEP_CHARS`] more, up to
///   [`CHUNK_CHARS`]. A sentence longer than its chunk's size goes whole; only
///   the provider's hard ceiling, `max_chars`, cuts inside one — by words, then
///   by characters.
/// - **Utterances aren't stitched together**: a message boundary is both a
///   natural pause and a cancellation point, and the next message may be
///   another voice.
pub(super) fn chunk_utterances(
    utterances: &[(MessageRole, String)],
    max_chars: usize,
) -> Vec<(MessageRole, String)> {
    let max = max_chars.max(1);
    let mut out: Vec<(MessageRole, String)> = Vec::new();
    for (role, utterance) in utterances {
        let mut packer = Packer {
            role: *role,
            max,
            cur: String::new(),
            chars: 0,
        };
        for block in utterance.lines().map(str::trim).filter(|b| !b.is_empty()) {
            packer.block(block, &mut out);
        }
        packer.finish(&mut out);
    }
    out
}

/// The chunk being built from one utterance ([`chunk_utterances`]).
struct Packer {
    role: MessageRole,
    /// The provider's hard ceiling.
    max: usize,
    cur: String,
    /// `cur`'s length in characters.
    chars: usize,
}

impl Packer {
    /// Adds a block's sentences, a line break before its first.
    fn block(&mut self, block: &str, out: &mut Vec<(MessageRole, String)>) {
        let mut sep = '\n';
        for sentence in crate::features::tools::rag::split_sentences(block) {
            for piece in split_long(&sentence, self.max) {
                if self.piece(piece.trim(), sep, out) {
                    sep = ' ';
                }
            }
        }
    }

    /// Adds a piece of a sentence after `sep`, first closing the chunk if the
    /// piece would take it past its soft size; `false` for an empty piece.
    fn piece(&mut self, piece: &str, sep: char, out: &mut Vec<(MessageRole, String)>) -> bool {
        if piece.is_empty() {
            return false;
        }
        let piece_chars = piece.chars().count();
        if self.chars > 0 && self.chars + 1 + piece_chars > chunk_size(out.len(), self.max) {
            out.push((self.role, std::mem::take(&mut self.cur)));
            self.chars = 0;
        }
        if self.chars > 0 {
            self.cur.push(sep);
            self.chars += 1;
        }
        self.cur.push_str(piece);
        self.chars += piece_chars;
        true
    }

    /// Closes the last chunk.
    fn finish(self, out: &mut Vec<(MessageRole, String)>) {
        if self.chars > 0 {
            out.push((self.role, self.cur));
        }
    }
}

/// The soft size of the run's chunk number `index` (from 0), capped by the
/// provider's ceiling.
fn chunk_size(index: usize, max: usize) -> usize {
    FIRST_CHUNK_CHARS
        .saturating_add(index.saturating_mul(CHUNK_STEP_CHARS))
        .min(CHUNK_CHARS)
        .min(max)
}

/// Cuts by character a sentence that's itself longer than the limit (a rare
/// case — text with no punctuation). A word boundary is preferred over the
/// middle of a word.
fn split_long(sentence: &str, max: usize) -> Vec<String> {
    if sentence.chars().count() <= max {
        return vec![sentence.to_string()];
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in sentence.split_whitespace() {
        let wlen = word.chars().count();
        if wlen > max {
            // A word longer than the limit — cut it by character (otherwise the
            // chunk wouldn't fit).
            split_giant_word(word, max, &mut cur, &mut out);
            continue;
        }
        let extra = if cur.is_empty() { 0 } else { 1 };
        if cur.chars().count() + extra + wlen > max {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Cuts a single word longer than the limit into `max`-character parts,
/// flushing the chunk built so far first.
fn split_giant_word(word: &str, max: usize, cur: &mut String, out: &mut Vec<String>) {
    if !cur.is_empty() {
        out.push(std::mem::take(cur));
    }
    let chars: Vec<char> = word.chars().collect();
    for part in chars.chunks(max) {
        out.push(part.iter().collect());
    }
}

/// Parameters of the background speech task.
struct TtsTask<Q: SpeechQueue> {
    /// The assistant's voice engine (and every utterance's, if a separate user
    /// voice isn't set).
    engine: Box<dyn TtsEngine>,
    /// The user's voice engine (`Some` only when a "User voice" is set).
    user_engine: Option<Box<dyn TtsEngine>>,
    /// Chunks with their source role — used to pick the engine.
    chunks: Vec<(MessageRole, String)>,
    cancel: CancellationToken,
    /// The task's generation — the orchestrator uses it to tell its own `done`
    /// apart from a stale one.
    task_id: Uuid,
    /// The queue played into: the audio device, opened by the handler and
    /// shared with the orchestrator (`Arc`, for `/tts pause`/`resume`). Its
    /// `Arc` is dropped when the task finishes.
    playback: Arc<Q>,
    /// The interface language (axis B) — human-facing error text.
    loc: &'static Locale,
    evt_tx: tokio::sync::mpsc::UnboundedSender<AppEvent>,
    done_tx: tokio::sync::mpsc::UnboundedSender<Uuid>,
}

/// One chunk's timings, as the log tells them.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ChunkTiming {
    chars: usize,
    /// From the request to the whole clip.
    synth: Duration,
    /// How long the clip plays.
    sound: Duration,
    /// The sound that was still queued when its synthesis began.
    ahead: Duration,
    /// How long the listener heard nothing before it: the queue ran dry while
    /// it was being made. Zero for the run's first chunk — that wait is the
    /// first sound's, not a pause.
    silence: Duration,
}

/// What a speech run did, for its summary in the log.
#[derive(Debug, Default)]
struct SpeechRun {
    /// Every chunk queued, in order.
    chunks: Vec<ChunkTiming>,
    /// From the start to the first sound queued.
    first_sound: Option<Duration>,
    /// Stopped (or failed) before the last chunk played.
    stopped: bool,
}

impl SpeechRun {
    /// The silences long enough to hear ([`AUDIBLE_SILENCE`]).
    fn audible_silences(&self) -> impl Iterator<Item = Duration> + '_ {
        self.chunks
            .iter()
            .map(|c| c.silence)
            .filter(|s| *s >= AUDIBLE_SILENCE)
    }
}

/// Starts the background speech run on its queue — the audio device, opened
/// by the handler (`Playback::open` on the `/tts` command, shared via `Arc`) —
/// and tells the log how it went.
fn spawn_tts<Q: SpeechQueue>(task: TtsTask<Q>) {
    tokio::spawn(async move {
        let run = run_speech(&task).await;
        let silences: Vec<Duration> = run.audible_silences().collect();
        tracing::info!(
            chunks = run.chunks.len(),
            sound_s = run
                .chunks
                .iter()
                .map(|c| c.sound)
                .sum::<Duration>()
                .as_secs(),
            first_sound_ms = run.first_sound.map(millis),
            silences = silences.len(),
            silence_ms = millis(silences.iter().sum()),
            stopped = run.stopped,
            "speech: the run ended"
        );
        let _ = task.done_tx.send(task.task_id);
    });
}

/// Synthesizes the chunks into the queue, holding back while [`LOOKAHEAD`] of
/// sound is queued, then waits for the queue to play out — or stops it on
/// cancellation.
async fn run_speech<Q: SpeechQueue>(task: &TtsTask<Q>) -> SpeechRun {
    let started = Instant::now();
    let mut run = SpeechRun::default();
    let total = task.chunks.len();
    for (index, (role, chunk)) in task.chunks.iter().enumerate() {
        if task.cancel.is_cancelled() {
            break;
        }
        let played = run.first_sound.is_some();
        let Some(timing) = synth_chunk(task, *role, chunk, played).await else {
            break;
        };
        if run.first_sound.is_none() && timing.sound > Duration::ZERO {
            run.first_sound = Some(started.elapsed());
        }
        tracing::info!(
            chunk = index + 1,
            of = total,
            chars = timing.chars,
            synth_ms = millis(timing.synth),
            sound_ms = millis(timing.sound),
            ahead_ms = millis(timing.ahead),
            silence_ms = millis(timing.silence),
            "speech: a chunk is queued"
        );
        run.chunks.push(timing);
    }
    run.stopped = run.chunks.len() < total;

    // Wait for the queue to finish playing (or for cancellation).
    while !task.playback.is_drained() && !task.cancel.is_cancelled() {
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    if task.cancel.is_cancelled() {
        task.playback.stop();
        run.stopped = true;
    } else {
        // The queue reports "empty" when the source is **exhausted by the
        // mixer**, not when the sound card has finished playing out its
        // buffer (measured by a smoke test: a 400 ms clip reports
        // `is_drained` after ~370 ms). Dropping `Playback` closes the device,
        // so without this pause the last chunk's tail would be cut off by a
        // few dozen milliseconds.
        tokio::time::sleep(DRAIN_TAIL).await;
    }
    run
}

/// Synthesizes one chunk and queues it, first waiting until less than
/// [`LOOKAHEAD`] of sound is queued. `played` — whether anything was queued
/// before, so that the queue running dry is a silence rather than the wait
/// for the first sound. `None` when the run must stop: cancellation, or an
/// error already reported to the feed.
async fn synth_chunk<Q: SpeechQueue>(
    task: &TtsTask<Q>,
    role: MessageRole,
    chunk: &str,
    played: bool,
) -> Option<ChunkTiming> {
    let fail = |msg: String| {
        let _ = task.evt_tx.send(AppEvent::Error(msg));
    };
    let queue = task.playback.as_ref();

    // Pipeline: don't synthesize further ahead than the listener needs.
    while queue.ahead() >= LOOKAHEAD && !task.cancel.is_cancelled() {
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    if task.cancel.is_cancelled() {
        return None;
    }
    // User utterances get their own voice, when set; otherwise (and for
    // the assistant) — the main engine.
    let active: &dyn TtsEngine = match role {
        MessageRole::User => task.user_engine.as_deref().unwrap_or(task.engine.as_ref()),
        _ => task.engine.as_ref(),
    };
    let ahead = queue.ahead();
    let begun = Instant::now();
    // While the request runs, watch for the queue running dry: from then on
    // the listener hears nothing. A paused queue is not dry, so `/tts pause`
    // is never counted as a silence.
    let dry = |since: &mut Option<Instant>| {
        if played && since.is_none() && queue.is_drained() {
            *since = Some(Instant::now());
        }
    };
    let mut dry_since = None;
    dry(&mut dry_since);
    let synthesis = active.synthesize(chunk, &task.cancel);
    tokio::pin!(synthesis);
    let result = loop {
        tokio::select! {
            result = &mut synthesis => break result,
            () = tokio::time::sleep(POLL_INTERVAL) => dry(&mut dry_since),
        }
    };
    let synth = begun.elapsed();
    let clip = match result {
        Ok(clip) => clip,
        Err(err) => {
            // Cancellation isn't an error: the user stopped it themselves.
            if !task.cancel.is_cancelled() {
                fail(
                    task.loc
                        .tf("ui.err.tts_synth", &[("err", &err.to_string())]),
                );
            }
            return None;
        }
    };
    dry(&mut dry_since);
    // An empty clip queues nothing and plays for zero — not an error.
    let sound = match queue.append(clip).await {
        Ok(sound) => sound,
        Err(err) => {
            fail(
                task.loc
                    .tf("ui.err.tts_playback", &[("err", &err.to_string())]),
            );
            return None;
        }
    };
    Some(ChunkTiming {
        chars: chunk.chars().count(),
        synth,
        sound,
        ahead,
        silence: dry_since.map_or(Duration::ZERO, |since| since.elapsed()),
    })
}

/// A duration in whole milliseconds, for the log.
fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};

    fn ru() -> &'static Locale {
        locale(Lang::Ru)
    }

    fn chat_messages() -> Vec<Message> {
        vec![
            Message::user("первое от пользователя"),
            Message::assistant("первый ответ"),
            Message::user("второе от пользователя"),
            Message::assistant("второй ответ"),
        ]
    }

    /// Utterance texts with no roles — for checks where the role doesn't matter.
    fn texts(v: Vec<(MessageRole, String)>) -> Vec<String> {
        v.into_iter().map(|(_, s)| s).collect()
    }

    /// An assistant utterance for the chunker's input (the role doesn't matter there, but the type needs it).
    fn asst(s: &str) -> (MessageRole, String) {
        (MessageRole::Assistant, s.to_string())
    }

    /// Chunk texts with no roles.
    fn chunk_texts(chunks: &[(MessageRole, String)]) -> Vec<String> {
        chunks.iter().map(|(_, s)| s.clone()).collect()
    }

    #[test]
    fn last_scope_takes_only_final_message() {
        let out = build_utterances(&chat_messages(), TtsScope::Last, false, ru()).unwrap();
        assert_eq!(
            out,
            vec![(MessageRole::Assistant, "второй ответ.".to_string())]
        );
    }

    #[test]
    fn recent_scope_takes_tail_in_chronological_order() {
        let out = build_utterances(&chat_messages(), TtsScope::Recent(3), false, ru()).unwrap();
        assert_eq!(out.len(), 3);
        // The order is chronological, and roles are preserved (for multi-voice speech).
        assert_eq!(out[0].0, MessageRole::Assistant);
        assert!(out[0].1.contains("первый ответ"), "order: {out:?}");
        assert_eq!(out[1].0, MessageRole::User);
        assert!(out[2].1.contains("второй ответ"));
        // Requesting more than there is hands out everything (a clamp, not an error).
        let all = build_utterances(&chat_messages(), TtsScope::Recent(99), false, ru()).unwrap();
        assert_eq!(all.len(), 4);
    }

    #[test]
    fn all_scope_takes_everything_spoken() {
        let out = build_utterances(&chat_messages(), TtsScope::All, false, ru()).unwrap();
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn service_messages_and_empty_text_are_skipped() {
        let messages = vec![
            Message::new(MessageRole::System, "системная инструкция"),
            Message::user("   "),
            Message::new(MessageRole::Tool, "результат инструмента"),
            Message::assistant("настоящий ответ"),
        ];
        let out = build_utterances(&messages, TtsScope::All, false, ru()).unwrap();
        assert_eq!(out.len(), 1, "only user/assistant get spoken: {out:?}");
        assert!(out[0].1.contains("настоящий ответ"));
        // Nothing at all to speak — None (the caller shows a clear error).
        assert!(build_utterances(&[], TtsScope::All, false, ru()).is_none());
        assert!(
            build_utterances(
                &[Message::new(MessageRole::System, "только системное")],
                TtsScope::All,
                false,
                ru()
            )
            .is_none()
        );
    }

    #[test]
    fn role_prefixes_apply_to_every_scope_including_single() {
        // The "Speak roles" toggle applies to a single `/tts` too (decision point R6).
        let one = texts(build_utterances(&chat_messages(), TtsScope::Last, true, ru()).unwrap());
        assert!(
            one[0].starts_with(ru().t("speak.role.assistant")),
            "the role prefix appears on a single message too: {one:?}"
        );
        let many =
            texts(build_utterances(&chat_messages(), TtsScope::Recent(2), true, ru()).unwrap());
        assert!(many[0].starts_with(ru().t("speak.role.user")));
        assert!(many[1].starts_with(ru().t("speak.role.assistant")));
    }

    #[test]
    fn message_whose_text_is_all_skippable_is_dropped() {
        // A message consisting of a single code block yields only a marker — it gets
        // spoken, whereas an empty extractor result would drop the message.
        let messages = vec![Message::assistant("```rust\nfn main() {}\n```")];
        let out = build_utterances(&messages, TtsScope::All, false, ru()).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].1.contains(ru().t("speak.skip.code")), "{out:?}");
    }

    #[test]
    fn chunk_inherits_role_of_source_message() {
        // An utterance's role carries over to all its chunks — the voice is picked by it.
        let src = vec![
            (MessageRole::User, "Раз. Два.".to_string()),
            (MessageRole::Assistant, "Три. Четыре.".to_string()),
        ];
        let chunks = chunk_utterances(&src, 6);
        assert!(chunks.iter().all(|(_, c)| c.chars().count() <= 6));
        // The first chunks — the user's, the last — the assistant's.
        assert_eq!(chunks.first().unwrap().0, MessageRole::User);
        assert_eq!(chunks.last().unwrap().0, MessageRole::Assistant);
    }

    #[test]
    fn chunking_respects_limit_and_sentence_boundaries() {
        let text = "Первое предложение. Второе предложение! Третье предложение?";
        let chunks = chunk_utterances(&[asst(text)], 25);
        let ch = chunk_texts(&chunks);
        assert!(
            ch.iter().all(|c| c.chars().count() <= 25),
            "the limit is respected: {ch:?}"
        );
        // Boundaries — on sentences (punctuation is preserved at the end of a chunk).
        assert!(
            ch.iter()
                .all(|c| c.ends_with('.') || c.ends_with('!') || c.ends_with('?')),
            "a chunk ends at a sentence boundary: {ch:?}"
        );
        // Nothing lost.
        assert_eq!(ch.join(" ").replace("  ", " "), text);
    }

    #[test]
    fn short_text_stays_single_chunk() {
        let chunks = chunk_utterances(&[asst("Коротко.")], 4096);
        assert_eq!(chunk_texts(&chunks), vec!["Коротко.".to_string()]);
    }

    #[test]
    fn utterances_are_not_merged_across_messages() {
        // A message boundary is a natural pause and a cancellation point: we don't stitch
        // together even short utterances.
        let chunks = chunk_utterances(&[asst("Раз."), asst("Два.")], 4096);
        assert_eq!(
            chunk_texts(&chunks),
            vec!["Раз.".to_string(), "Два.".to_string()]
        );
    }

    #[test]
    fn overlong_sentence_is_split_by_words_then_chars() {
        let long_words = "слово ".repeat(20);
        let chunks = chunk_utterances(&[asst(long_words.trim())], 20);
        let ch = chunk_texts(&chunks);
        assert!(ch.iter().all(|c| c.chars().count() <= 20), "{ch:?}");
        assert!(ch.len() > 1);
        // A single word longer than the limit is cut by character, not lost.
        let giant = "я".repeat(50);
        let chunks = chunk_utterances(&[asst(&giant)], 20);
        let ch = chunk_texts(&chunks);
        assert!(ch.iter().all(|c| c.chars().count() <= 20), "{ch:?}");
        assert_eq!(ch.concat().chars().count(), giant.chars().count());
    }

    /// One assistant utterance of these blocks (lines), as `speakable_text`
    /// joins them.
    fn blocks(lines: &[&str]) -> Vec<(MessageRole, String)> {
        vec![asst(&lines.join("\n"))]
    }

    /// A paragraph of `n` sentences of `len` characters each (the period
    /// included), all different.
    fn paragraph(n: usize, len: usize) -> String {
        (0..n)
            .map(|i| format!("{:>width$}.", i, width = len - 1).replace(' ', "ж"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn a_heading_travels_with_its_paragraph() {
        let body = paragraph(3, 50);
        let chunks = chunk_texts(&chunk_utterances(&blocks(&["Мой вывод.", &body]), 4096));
        // The block boundary stays a line break inside the chunk.
        assert_eq!(chunks, vec![format!("Мой вывод.\n{body}")]);
    }

    #[test]
    fn a_lists_items_go_together() {
        let items = [
            "Скорость ответа. Нужно разделять:",
            "ожидание первого видимого токена;",
            "скорость последующей генерации;",
            "длительность всей работы.",
        ];
        let chunks = chunk_texts(&chunk_utterances(&blocks(&items), 4096));
        assert_eq!(chunks, vec![items.join("\n")]);
    }

    /// The note of a skipped code block was a request of its own — a second
    /// of sound before a paragraph's synthesis. Now it rides with its
    /// neighbours.
    #[test]
    fn the_code_block_note_rides_with_its_neighbours() {
        let md = "Вот пример.\n\n```rust\nfn main() {}\n```\n\nПосле кода идёт абзац.";
        let utterances =
            build_utterances(&[Message::assistant(md)], TtsScope::Last, false, ru()).unwrap();
        let chunks = chunk_texts(&chunk_utterances(&utterances, 4096));
        // The extractor closes the note with a period, as every block.
        let note = ru().t("speak.skip.code");
        assert_eq!(
            chunks,
            vec![format!("Вот пример.\n{note}.\nПосле кода идёт абзац.")]
        );
    }

    /// The first chunk is short — the first sound waits for all of it — and
    /// each next one may be longer, up to the steady size; a chunk is closed
    /// only when the next sentence would not fit.
    #[test]
    fn chunk_sizes_ramp_up_from_a_short_first_chunk() {
        let text = paragraph(40, 50);
        let chunks = chunk_texts(&chunk_utterances(&[asst(&text)], 4096));
        let sizes: Vec<usize> = chunks.iter().map(|c| c.chars().count()).collect();
        // 50-character sentences and a space between: 3 fit in 200, 5 in 300,
        // 7 in 400 — then 7 a chunk, and the last 4 of the 40.
        assert_eq!(sizes, vec![152, 254, 356, 356, 356, 356, 203], "{chunks:?}");
        for (i, size) in sizes.iter().enumerate() {
            assert!(*size <= chunk_size(i, 4096), "chunk {i}: {size}");
        }
    }

    /// A sentence longer than its chunk's soft size goes whole: only the
    /// provider's ceiling cuts inside a sentence.
    #[test]
    fn a_sentence_longer_than_its_chunk_goes_whole() {
        let long = format!("{}.", "слово ".repeat(80).trim());
        assert!(long.chars().count() > CHUNK_CHARS);
        let chunks = chunk_texts(&chunk_utterances(&blocks(&["Коротко.", &long]), 4096));
        assert_eq!(chunks, vec!["Коротко.".to_string(), long]);
    }

    /// Packing loses nothing and reorders nothing: the words come out as they
    /// went in, across blocks and chunks.
    #[test]
    fn packing_keeps_every_word_in_order() {
        let body = paragraph(12, 70);
        let lines = ["Заголовок.", &body, "пункт один;", "пункт два.", &body];
        let source = blocks(&lines);
        let chunks = chunk_texts(&chunk_utterances(&source, 4096));
        assert!(chunks.len() > 1, "{chunks:?}");
        let words_in: Vec<&str> = source[0].1.split_whitespace().collect();
        let joined = chunks.join(" ");
        let words_out: Vec<&str> = joined.split_whitespace().collect();
        assert_eq!(words_out, words_in);
    }

    /// A reply shaped like the one that fell silent — headings, two lists, a
    /// code block, long paragraphs — goes in a handful of requests, not one
    /// per block.
    #[test]
    fn a_structured_reply_goes_in_a_handful_of_requests() {
        let para = paragraph(5, 60);
        let md = format!(
            "{para}\n\n### Что не сходится\n\nТри утверждения:\n\n1. Первое.\n2. Второе.\n3. Третье.\n\n\
             {para}\n\n```python\nprint(1)\n```\n\n{para}\n\n- один;\n- два;\n- три.\n\n### Вывод\n\n{para}"
        );
        let utterances =
            build_utterances(&[Message::assistant(md)], TtsScope::Last, false, ru()).unwrap();
        let blocks = utterances[0].1.lines().count();
        let chunks = chunk_utterances(&utterances, 4096);
        assert_eq!(blocks, 14);
        assert!(chunks.len() <= 6, "{} chunks: {chunks:?}", chunks.len());
        assert!(chunks[0].1.chars().count() <= FIRST_CHUNK_CHARS);
    }

    #[test]
    fn setup_error_keys_exist_in_bundle() {
        for err in [
            TtsSetupError::Model,
            TtsSetupError::ApiKey,
            TtsSetupError::Url,
        ] {
            let key = setup_error_key(err);
            assert!(ru().has_key(key), "key {key} must be in the bundle");
        }
    }
}

#[cfg(test)]
#[path = "tts/tests/pipeline.rs"]
mod pipeline_tests;

#[cfg(test)]
#[path = "tts/tests/live.rs"]
mod live_tests;

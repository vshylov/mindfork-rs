//! Chat message speech (`/tts [N|all|stop]` command, spec §11.9).
//!
//! Modeled on [`super::rag`]: command → a data snapshot from the orchestrator
//! (the sole owner of `Chat`) → a cancellable background task. The task runs as
//! a **pipeline**: while chunk N plays, N+1 is being synthesized — the first
//! sound arrives fast, and no more than one chunk is synthesized ahead (we
//! don't pay for what the user might cut off). The chat isn't mutated,
//! generation isn't gated: a **snapshot** of the conversation at command time
//! is spoken.
//!
//! Stop points are collected into one helper [`Orchestrator::stop_tts`]
//! (precedent — `reset_rag_cancel`): two by setting (switching chats, starting
//! generation) and three **unconditional** — deleting an exchange,
//! regeneration, deleting a chat: the text being spoken no longer exists. See
//! docs/research/tts.md §7, §8.

use std::time::Duration;

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::entities::message::{Message, MessageRole};
use crate::features::tts_command::TtsScope;
use crate::shared::config::SecretSlot;
use crate::shared::i18n::Locale;
use crate::shared::tts::{TtsEngine, TtsSetupError, engines_from_config, playback::Playback};

use super::Orchestrator;

/// How many chunks to keep in the playback queue (the one playing + one
/// ready). More — pay for synthesis that might not be needed; less — risk a
/// pause between chunks.
const QUEUE_AHEAD: usize = 2;

/// Playback-queue poll interval (`rodio` has no async "queue is empty"
/// notification, and `sleep_until_end` is blocking).
const POLL_INTERVAL: Duration = Duration::from_millis(80);

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
        // Markers and role prefixes are speech content, so they're in the
        // **profile's** language (axis A, docs/history/i18n.md), not the
        // interface's.
        let speech_loc = self.profile_locale(profile_id);
        let chunks =
            match build_utterances(&messages, scope, self.config.tts.speak_roles, speech_loc) {
                Some(text) => text,
                None => {
                    self.fail_tts(self.ui_locale().t("ui.err.tts_nothing_to_speak"));
                    return;
                }
            };

        // Build the client from a settings snapshot: in a cloud mode the stored
        // provider key is shared with chat (ADR 0008) — no need to enter it again;
        // in `external` mode it is the speech slot's own key
        // (docs/history/external-api-key.md §3).
        let stored = self.config.tts.secret_key().and_then(|k| {
            crate::shared::secrets::stored_key(&self.config.api_keys, &k.storage_name())
        });
        // Two engines: the assistant's and (opt.) the user's — when a separate
        // "User voice" is set (spec §11.9). Both use the same provider → a shared
        // limit.
        let (engine, user_engine) = match engines_from_config(&self.config.tts, stored) {
            Ok(pair) => pair,
            Err(err) => {
                self.fail_tts(self.ui_locale().t(setup_error_key(err)));
                return;
            }
        };
        let chunks = chunk_utterances(&chunks, engine.max_input_chars());
        if chunks.is_empty() {
            self.fail_tts(self.ui_locale().t("ui.err.tts_nothing_to_speak"));
            return;
        }

        // We open the device here (not in the task) to share `Arc<Playback>` with
        // the orchestrator for instant `/tts pause`/`resume`. Opening is still
        // **lazy** — on the `/tts` command, not at app startup (cpal#384). No
        // audio → a clear note, the task isn't spawned.
        let playback = match Playback::open() {
            Ok(p) => std::sync::Arc::new(p),
            Err(err) => {
                self.fail_tts(
                    &self
                        .ui_locale()
                        .tf("ui.err.tts_no_audio", &[("err", &err.to_string())]),
                );
                return;
            }
        };

        let cancel = CancellationToken::new();
        let task_id = Uuid::new_v4();
        self.tts_cancel = Some(cancel.clone());
        self.tts_gen = Some(task_id);
        self.tts_playback = Some(playback.clone());
        let _ = self.evt_tx.send(AppEvent::TtsActive(true));
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
}

/// Bundle key for a structured speech-setup error.
fn setup_error_key(err: TtsSetupError) -> &'static str {
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

/// Cuts utterances into chunks no longer than `max_chars` characters — on
/// sentence boundaries (reusing RAG's chunker), so a chunk seam never falls
/// mid-phrase. Utterances aren't stitched together: a message boundary is both
/// a natural pause and a cancellation point.
pub(super) fn chunk_utterances(
    utterances: &[(MessageRole, String)],
    max_chars: usize,
) -> Vec<(MessageRole, String)> {
    let max = max_chars.max(1);
    let mut out = Vec::new();
    for (role, utterance) in utterances {
        for block in utterance.lines() {
            let block = block.trim();
            if block.is_empty() {
                continue;
            }
            let mut chunks = Vec::new();
            pack_sentences(block, max, &mut chunks);
            // All of the block's chunks inherit the source message's role.
            out.extend(chunks.into_iter().map(|c| (*role, c)));
        }
    }
    out
}

/// Packs a block's sentences into chunks up to `max` characters.
fn pack_sentences(block: &str, max: usize, out: &mut Vec<String>) {
    let mut cur = String::new();
    for sentence in crate::features::tools::rag::split_sentences(block) {
        append_pieces(&sentence, max, &mut cur, out);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
}

/// Appends one sentence's pieces to the chunk being built (`cur`), flushing it
/// into `out` whenever the next piece would push it past `max` characters.
fn append_pieces(sentence: &str, max: usize, cur: &mut String, out: &mut Vec<String>) {
    for piece in split_long(sentence, max) {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        let extra = if cur.is_empty() { 0 } else { 1 };
        if !cur.is_empty() && cur.chars().count() + extra + piece.chars().count() > max {
            out.push(std::mem::take(cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(piece);
    }
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
struct TtsTask {
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
    /// The audio device, opened by the handler and shared with the orchestrator
    /// (`Arc`, for `/tts pause`/`resume`). Its `Arc` is dropped when the task
    /// finishes.
    playback: std::sync::Arc<Playback>,
    /// The interface language (axis B) — human-facing error text.
    loc: &'static Locale,
    evt_tx: tokio::sync::mpsc::UnboundedSender<AppEvent>,
    done_tx: tokio::sync::mpsc::UnboundedSender<Uuid>,
}

/// Starts the background speech run: synthesizes chunks into the given
/// playback queue, holding back synthesis while the queue is full. The device
/// is already opened by the handler (`Playback::open` — on the `/tts` command,
/// shared via `Arc`).
fn spawn_tts(task: TtsTask) {
    tokio::spawn(async move {
        for (role, chunk) in &task.chunks {
            if task.cancel.is_cancelled() {
                break;
            }
            if !synth_chunk(&task, *role, chunk).await {
                break;
            }
        }

        // Wait for the queue to finish playing (or for cancellation).
        while !task.playback.is_drained() && !task.cancel.is_cancelled() {
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        if task.cancel.is_cancelled() {
            task.playback.stop();
        } else {
            // The queue reports "empty" when the source is **exhausted by the
            // mixer**, not when the sound card has finished playing out its
            // buffer (measured by a smoke test: a 400 ms clip reports
            // `is_drained` after ~370 ms). Dropping `Playback` closes the device,
            // so without this pause the last chunk's tail would be cut off by a
            // few dozen milliseconds.
            tokio::time::sleep(DRAIN_TAIL).await;
        }
        let _ = task.done_tx.send(task.task_id);
    });
}

/// Synthesizes one chunk and enqueues it, first waiting for a queue slot (the
/// synthesis-ahead-of-playback pipeline — see [`QUEUE_AHEAD`]). Returns `false`
/// when the run must stop: cancellation, or an error already reported to the
/// feed.
async fn synth_chunk(task: &TtsTask, role: MessageRole, chunk: &str) -> bool {
    let fail = |msg: String| {
        let _ = task.evt_tx.send(AppEvent::Error(msg));
    };

    // Pipeline: don't synthesize further ahead than the queue needs.
    while task.playback.queued() >= QUEUE_AHEAD && !task.cancel.is_cancelled() {
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    if task.cancel.is_cancelled() {
        return false;
    }
    // User utterances get their own voice, when set; otherwise (and for
    // the assistant) — the main engine.
    let active: &dyn TtsEngine = match role {
        MessageRole::User => task.user_engine.as_deref().unwrap_or(task.engine.as_ref()),
        _ => task.engine.as_ref(),
    };
    match active.synthesize(chunk, &task.cancel).await {
        Ok(clip) if !clip.is_empty() => {
            if let Err(err) = task.playback.enqueue(clip) {
                fail(
                    task.loc
                        .tf("ui.err.tts_playback", &[("err", &err.to_string())]),
                );
                return false;
            }
            true
        }
        // An empty clip — simply nothing to play (not an error).
        Ok(_) => true,
        Err(err) => {
            // Cancellation isn't an error: the user stopped it themselves.
            if !task.cancel.is_cancelled() {
                fail(
                    task.loc
                        .tf("ui.err.tts_synth", &[("err", &err.to_string())]),
                );
            }
            false
        }
    }
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

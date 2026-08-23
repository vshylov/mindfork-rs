//! Chat screen — projecting AppEvent into the feed (messages, generation, tool blocks, tokens). Part of the [`super`] module; split out of the
//! chat.rs monolith (see docs/history/refactoring-god-objects.md, stage 2).

use super::*;

// Imported here rather than through `super`: `screens` may not depend on `app`
// (FSD), so the jump descriptor comes from `features`, where both layers can
// see it — the `RagProgress` precedent.
use crate::features::chat_search::FeedFocus;

impl ChatScreen {
    /// Marks that the feed content changed (streaming, a new message, a note,
    /// an edit): if the feed contains risk-group glyphs, the next frame is drawn as a **full
    /// redraw** — otherwise legacy terminals leave artifacts from changed
    /// emoji lines. See [`is_risky_glyph`], [`crate::shared::ui::prime_full_redraw`].
    ///
    /// Called by **mutators**, not by render fact: the artifact must not be shown even
    /// for a single frame. Detecting "there's a risk" from a render fact would lag a frame
    /// behind (a cache miss is only visible there), and the artifact would flash —
    /// hiding this behind synchronized output isn't possible, conhost ignores mode 2026.
    ///
    /// The risk flag is cached and **only accumulates**: edits always affect the
    /// last block, so we only check it, while a full feed rebuild
    /// ([`Self::activate_chat`]) recomputes it from scratch. Over-estimating is
    /// safe — an extra redraw isn't visible, a missed one leaves an artifact.
    pub(super) fn mark_feed_changed(&mut self) {
        if !self.feed_has_risky
            && let Some(last) = self.feed.last()
        {
            self.feed_has_risky = feed_msg_has_risky_glyph(last);
        }
        if self.feed_has_risky {
            self.full_redraw = true;
        }
    }

    /// Updates the chat title in the projection (after a manual/auto rename).
    /// Changes the title in the feed header if this is the active chat. The list and the
    /// overlay are additionally updated by the `ChatList` event (`set_chat_list`).
    pub fn rename_chat(&mut self, id: Uuid, title: String) {
        if self.active_chat == Some(id) {
            self.title = title.clone();
        }
        if let Some(c) = self.chats.iter_mut().find(|c| c.id == id) {
            c.title = title;
        }
    }

    /// Rebuilds the feed for a chat. `focus` — a domain message to put the view
    /// on together with the query to highlight inside it
    /// (`AppCommand::OpenChatAt`); `None` — the usual "show the tail", with
    /// nothing highlighted. `feed_view` — the chat's stored collapse state
    /// ("thoughts"/tool calls, spec §11.3). `compaction` — the history-compaction
    /// boundary `(the id of the first message still sent verbatim, the rolling
    /// summary)`, or `None` when nothing is folded (spec §6.7).
    #[allow(clippy::too_many_arguments)]
    pub fn activate_chat(
        &mut self,
        id: Uuid,
        title: String,
        messages: &[Message],
        draft: &str,
        feed_view: FeedView,
        focus: Option<FeedFocus>,
        compaction: Option<(Uuid, String)>,
    ) {
        // Switching chats resets the generation state: "orphaned" chunks of the
        // previous generation must not land in the new chat's feed.
        self.active_chat = Some(id);
        self.title = title;
        self.current_gen = None;
        self.generating = false;
        // The token counter belongs to the previous chat — clear it so it doesn't linger
        // in the status line after switching (the status bar hides the counter when
        // `tokens == 0 && context == None`).
        self.gen_tokens = 0;
        self.gen_context = None;
        self.gen_context_exact = false;
        self.gen_reasoning = 0;
        // The collapse state belongs to the chat, so it is applied **before**
        // the feed is built: the block cache is keyed on it (spec §11.3). So is
        // the compaction boundary — both are per-chat rendering inputs.
        self.feed_view.set_view(feed_view);
        self.feed_view.set_compaction(compaction);
        // Which `chat://` references resolve depends on the profile, which the
        // new chat may have changed — a third per-chat rendering input, applied
        // before the feed is built for the same reason (spec §11.3).
        self.refresh_known_chats();
        // The reference picker lists the *previous* chat's links.
        self.chat_links = None;
        // Stitch agentic-loop rounds into one "Assistant:" block with inline tool blocks.
        self.feed = FeedMessage::from_messages(messages);
        // A sub-agent transcript opens with its persona (spec §11.3) — the one
        // thing a reader of one wants first, and a chat never shows.
        if let Some(child) = &self.child {
            self.feed
                .insert(0, FeedMessage::system(child.system_message.clone()));
        }
        // The feed was replaced wholesale — recompute "is there a risk" from scratch (from
        // here on the flag only accumulates based on the last block).
        self.feed_has_risky = self.feed.iter().any(feed_msg_has_risky_glyph);
        self.mark_feed_changed();
        // The anchor and the marker index a feed that no longer exists — drop
        // both, so neither can survive into the wrong chat. (The marker outlives
        // a manual scroll on purpose, so a chat switch is the one place that has
        // to clear it explicitly.)
        // The feed is renumbered under any in-feed search, and `Ctrl+E`/`Ctrl+R`/
        // a rewrite round/a cross-chat jump all arrive here (§1.5) — so close it
        // rather than leave it pointing at messages that moved.
        self.search = None;
        self.search_last.clear();
        self.feed_view.clear_search();
        self.feed_view.clear_focus();
        // A jump asked for by the user wins over the tail; anything else — the
        // usual bottom. An id this chat doesn't contain isn't found, so it falls
        // through to the tail (and `clear_focus` above already dropped the
        // previous highlight). See docs/history/chat-search-stage2.md §4.
        match &focus {
            Some(f)
                if self
                    .feed_view
                    .focus_message(&self.feed, f.message, Some(&f.query)) => {}
            _ => self.feed_view.scroll_to_bottom(),
        }
        // Load the chat's saved draft into the input box (empty for a new chat).
        // Do NOT mark `draft_dirty` — otherwise we'd immediately send it back via the same
        // `SetDraft`; trigger the spellcheck recheck directly instead.
        self.input.set_text(draft);
        self.spell_dirty = true;
        self.last_edit = None;
    }

    /// Updates the history-compaction boundary after a live compaction
    /// (`AppEvent::Compacted`) — `(the id of the first message still sent
    /// verbatim, the rolling summary)`; `None` clears it.
    ///
    /// A feed mutator like any other: the boundary changes the rendered lines,
    /// so a legacy terminal needs the same full-redraw insurance every other
    /// content change takes out (see [`Self::mark_feed_changed`]).
    pub fn set_compaction(&mut self, compaction: Option<(Uuid, String)>) {
        self.feed_view.set_compaction(compaction);
        self.mark_feed_changed();
    }

    /// Returns the text for the input box after deleting the last exchange. If the field
    /// isn't empty, the text is prepended to its start (existing input isn't lost).
    /// See spec §11.7.
    pub fn restore_input(&mut self, text: String) {
        let existing = self.input.text();
        let combined = if existing.is_empty() {
            text
        } else {
            format!("{text}{existing}")
        };
        self.input.set_text(&combined);
        self.mark_input_changed();
    }

    pub fn push_user_message(&mut self, text: String) {
        self.feed.push(FeedMessage {
            role: FeedRole::User,
            text,
            thoughts: String::new(),
            tools: Vec::new(),
            streaming: false,
            // The echo carries no id — the domain message is the orchestrator's;
            // the feed picks the ids up on the next activation.
            message_ids: Vec::new(),
            // A user message has no model behind it, in the feed or on disk.
            model: None,
        });
        self.mark_feed_changed();
        // User-initiated: you sent it, you want to see it (§4).
        self.feed_view.scroll_to_bottom();
    }

    /// `model` — the model this turn goes to (`AppEvent::GenerationStarted`),
    /// shown in the streaming bubble's header when `interface.show_model_name`
    /// is on. See spec §11.3.
    pub fn begin_generation(&mut self, generation_id: Uuid, model: Option<String>) {
        self.current_gen = Some(generation_id);
        self.generating = true;
        self.gen_tokens = 0;
        self.gen_context = None;
        self.gen_context_exact = false;
        self.gen_reasoning = 0;
        self.gen_model = model;
        self.pending_text_sep = false;
        self.pending_thoughts_sep = false;
        self.feed.push(FeedMessage {
            role: FeedRole::Assistant,
            text: String::new(),
            thoughts: String::new(),
            tools: Vec::new(),
            streaming: true,
            message_ids: Vec::new(),
            model: self.gen_model.clone(),
        });
        self.mark_feed_changed();
        // User-initiated (you pressed send/regenerate): show the new reply (§4).
        self.feed_view.scroll_to_bottom();
    }

    /// Appends a tool block to the current assistant message (live, during the turn).
    pub fn push_tool_call(
        &mut self,
        generation_id: Uuid,
        name: String,
        arguments: String,
        result: String,
        images: usize,
    ) {
        if self.current_gen == Some(generation_id)
            && let Some(last) = self.feed.last_mut()
        {
            // The call happened after response text had already accumulated — record the
            // position, so the tool block lands at the call site, not in the "header".
            let text_offset = last.text.len();
            last.tools.push(crate::widgets::message_feed::FeedToolCall {
                name,
                arguments,
                result,
                text_offset,
                images,
            });
            // Separate the next round's text/thoughts with a separator (matching a reload).
            self.pending_text_sep = true;
            self.pending_thoughts_sep = true;
            self.mark_feed_changed();
            // Arrives on its own mid-turn — must not yank a reader away (§4).
            self.feed_view.scroll_to_bottom_if_following();
        }
    }

    /// The assistant wrote a message and is continuing with a second one (the
    /// `send_followup_message` tool): finish the current bubble and add a new
    /// streaming assistant bubble — the next round's text will go into it.
    /// This way the live feed matches a reload (`from_messages` doesn't merge a
    /// message with `new_bubble`). See spec §9.3.
    pub fn continue_assistant(&mut self, generation_id: Uuid) {
        if self.current_gen != Some(generation_id) {
            return;
        }
        if let Some(last) = self.feed.last_mut() {
            last.streaming = false;
        }
        self.pending_text_sep = false;
        self.pending_thoughts_sep = false;
        self.feed.push(FeedMessage {
            role: FeedRole::Assistant,
            text: String::new(),
            thoughts: String::new(),
            tools: Vec::new(),
            streaming: true,
            message_ids: Vec::new(),
            // Same turn, same model — and the same answer the second message's
            // own metadata will carry once it is stored.
            model: self.gen_model.clone(),
        });
        self.mark_feed_changed();
        // Arrives on its own (the model chose to write another message) — §4.
        self.feed_view.scroll_to_bottom_if_following();
    }

    /// The assistant decided to rewrite the current message (the
    /// `rewrite_current_message` tool): discard the current bubble's already-accumulated
    /// text/thoughts/calls — the rewritten reply will go into the same bubble. See spec §9.3.
    pub fn rewrite_assistant(&mut self, generation_id: Uuid) {
        if self.current_gen != Some(generation_id) {
            return;
        }
        if let Some(last) = self.feed.last_mut() {
            last.text.clear();
            last.thoughts.clear();
            last.tools.clear();
            last.streaming = true;
        }
        self.pending_text_sep = false;
        self.pending_thoughts_sep = false;
        self.mark_feed_changed();
        // Arrives on its own (the model chose to rewrite) — §4.
        self.feed_view.scroll_to_bottom_if_following();
    }

    /// Guarantees that `last` is a streaming assistant bubble (the target for chunks).
    /// If a note slipped into the feed mid-generation (e.g. an `AppEvent::Error`
    /// about hitting the round limit before the final synthesis), `last` ends up being
    /// a note — in that case open a new assistant bubble, otherwise the stream would go
    /// into the note and render as plain text with no markdown.
    fn ensure_streaming_bubble(&mut self) {
        let ok = matches!(
            self.feed.last(),
            Some(m) if m.role == FeedRole::Assistant && m.streaming
        );
        if !ok {
            self.feed.push(FeedMessage {
                role: FeedRole::Assistant,
                text: String::new(),
                thoughts: String::new(),
                tools: Vec::new(),
                streaming: true,
                message_ids: Vec::new(),
                model: self.gen_model.clone(),
            });
        }
    }

    /// Shows the "retrying" chip, or clears it when a retry produced content.
    ///
    /// The chip is transient by construction: every path that ends the wait —
    /// content ([`Self::push_chunk`]/[`Self::push_thoughts`]) or the turn finishing
    /// ([`Self::finish_generation`]) — clears it, so it cannot outlive what it
    /// describes. Stale generations are dropped, as everywhere (spec §4.4).
    pub fn set_retrying(&mut self, generation_id: Uuid, attempt: u32, max: u32, delay_secs: u64) {
        if self.current_gen != Some(generation_id) {
            return;
        }
        self.retrying = Some(self.loc.tf(
            "ui.chat.bg.retry",
            &[
                ("attempt", &attempt.to_string()),
                ("max", &max.to_string()),
                ("secs", &delay_secs.to_string()),
            ],
        ));
    }

    /// Clears the retry chip: whatever it was waiting for has happened.
    fn clear_retrying(&mut self) {
        self.retrying = None;
    }

    pub fn push_chunk(&mut self, generation_id: Uuid, text: &str) {
        if self.current_gen != Some(generation_id) {
            return;
        }
        self.clear_retrying();
        self.ensure_streaming_bubble();
        if let Some(last) = self.feed.last_mut() {
            // The round's first text after a tool call gets an empty-line
            // separator (matching `FeedMessage::from_messages`).
            if self.pending_text_sep {
                self.pending_text_sep = false;
                if !last.text.is_empty() {
                    last.text.push_str("\n\n");
                }
            }
            last.text.push_str(text);
        }
        self.mark_feed_changed();
    }

    pub fn push_thoughts(&mut self, generation_id: Uuid, text: &str) {
        if self.current_gen != Some(generation_id) {
            return;
        }
        self.clear_retrying();
        self.ensure_streaming_bubble();
        if let Some(last) = self.feed.last_mut() {
            if self.pending_thoughts_sep {
                self.pending_thoughts_sep = false;
                if !last.thoughts.is_empty() {
                    last.thoughts.push('\n');
                }
            }
            last.thoughts.push_str(text);
        }
        self.mark_feed_changed();
    }

    /// Updates the token counter of the current generation (live). Ignores stale
    /// events (by `generation_id`). Updates the context (the conversation) only when it's
    /// set (`Some`), remembering whether it's an exact number or an estimate.
    pub fn set_token_usage(
        &mut self,
        generation_id: Uuid,
        completion: u64,
        context: Option<u64>,
        context_exact: bool,
        reasoning: Option<u32>,
    ) {
        if self.current_gen == Some(generation_id) {
            self.gen_tokens = completion;
            if let Some(c) = context {
                self.gen_context = Some(c);
                self.gen_context_exact = context_exact;
            }
            // Reasoning tokens are only known from `usage` (Some) — otherwise leave them be.
            if let Some(r) = reasoning {
                self.gen_reasoning = r;
            }
        }
    }

    pub fn finish_generation(&mut self, generation_id: Uuid, reason: FinishReason) {
        if self.current_gen != Some(generation_id) {
            return;
        }
        if let Some(last) = self.feed.last_mut() {
            last.streaming = false;
        }
        self.generating = false;
        self.current_gen = None;
        self.clear_retrying();
        if reason == FinishReason::Cancelled {
            self.push_note(self.loc.t("ui.chat.gen_cancelled"));
        }
    }

    pub fn push_error(&mut self, message: &str) {
        let warn = self.palette.glyphs().warn;
        self.push_note(&format!("{warn} {message}"));
    }

    /// Appends a neutral note to the feed (e.g. a confirmation of a chat-list
    /// operation once the list screen is already closed — a late auto-title/copy reply).
    pub fn push_note(&mut self, text: &str) {
        self.feed.push(FeedMessage::note(text));
        self.mark_feed_changed();
        // Arrives on its own (an error, a late list-operation reply) — §4.
        self.feed_view.scroll_to_bottom_if_following();
    }

    /// The agentic loop is asking whether to run a dangerous tool call
    /// (spec §9.8). Opens the modal popup; the answer leaves as
    /// [`ChatIntent::ConfirmTool`].
    pub fn request_tool_confirm(
        &mut self,
        generation_id: Uuid,
        call_id: String,
        name: String,
        arguments: String,
    ) {
        self.tool_confirm = Some(ToolConfirm {
            generation_id,
            call_id,
            name,
            arguments,
        });
    }
}

/// A character whose rendering on legacy terminals (conhost/Command Prompt) diverges
/// from `ratatui`'s model enough that changing the content leaves "hanging"
/// artifacts — halves of wide glyphs, pieces of the backdrop, drifted rows.
///
/// Risk classes (all about emoji, not CJK: terminals render width-2 ideographs
/// consistently, so there's no point triggering a full redraw over them):
/// - **VS16** (U+FE0F) — `🗂️`: `ratatui` sends this cluster's trailing cell
///   separately, see [`crate::shared::ui::prime_full_redraw`];
/// - **ZWJ** (U+200D) — `👨‍👩‍👧`: a composite cluster, the model's and the terminal's widths
///   diverge the most (a deliberate boundary, see the `shared/wrap.rs` doc);
/// - **skin-tone modifiers** (U+1F3FB..=U+1F3FF) — `👍🏽`;
/// - **supplementary-plane pictographs** (≥ U+1F000) — `😀`, `🔥`;
/// - **width-2 BMP emoji symbols** (`✅`, `⭐`, `✨`) — ordinary arrows/typography
///   from the same width-1 blocks don't fall into this.
pub(super) fn is_risky_glyph(c: char) -> bool {
    use crate::shared::wrap::char_width;
    matches!(c, '\u{FE0F}' | '\u{200D}')
        || ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
        || c >= '\u{1F000}'
        || (('\u{2190}'..='\u{2BFF}').contains(&c) && char_width(c) == 2)
}

/// Whether a feed item contains a risk-group glyph (in the text, "thoughts", or
/// tool-call arguments/result) — in that case a content change or
/// scroll requires a full redraw. See [`is_risky_glyph`].
pub(super) fn feed_msg_has_risky_glyph(m: &FeedMessage) -> bool {
    let has = |s: &str| s.chars().any(is_risky_glyph);
    has(&m.text) || has(&m.thoughts) || m.tools.iter().any(|t| has(&t.arguments) || has(&t.result))
}

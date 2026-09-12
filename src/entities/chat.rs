//! A chat: the message list, a profile binding, the active system message.
//! See spec §5.1.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entities::attachment::Attachment;
use crate::entities::chat_file::ChatFile;
use crate::entities::message::{Message, MessageRole};
use crate::entities::profile::{CharacterNames, Profile};
use crate::entities::sampling::SamplingConfig;
use crate::entities::subagent::{RunOutcome, SubagentRun};

/// A chat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chat {
    /// The schema version of this file's shape (ADR 0006; `CHAT_SCHEMA`).
    /// Absent in files written before the first chat-file step — those read as
    /// 1 and are migrated at startup before any load; every save since writes
    /// it, so a migrated file is never mistaken for an old one. A value loaded
    /// from a file is kept as loaded: the field states what the content
    /// conforms to, not which binary last touched it.
    #[serde(default = "chat_schema_v1")]
    pub v: u32,
    pub id: Uuid,
    pub profile_id: Uuid,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    /// The chat's active system message (the assistant can change it via a tool).
    pub system_message: String,
    /// A snapshot of the profile's role names at creation time (spec §10). **Not
    /// used for display**: the feed and the `F5` export resolve names from the
    /// profile, so editing them in settings applies to existing chats as well.
    /// Kept for the import format and a possible future per-chat override.
    #[serde(default)]
    pub character_names: CharacterNames,
    #[serde(default)]
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling_override: Option<SamplingConfig>,
    /// The unsaved input-box draft (text the user typed but hasn't sent yet).
    /// Stored in the chat file and restored into the input box on switching to the
    /// chat; empty for a new chat. See spec §11.7.
    #[serde(default)]
    pub draft: String,
    /// Which of the feed's foldable blocks are expanded in this chat
    /// (`Ctrl+T` — "thoughts", `Ctrl+O` — tool calls). Per chat, like
    /// [`Chat::draft`]: additive field, old chat files read without migration,
    /// and a chat nobody expanded writes no new key. See spec §11.3.
    #[serde(default, skip_serializing_if = "FeedView::is_default")]
    pub feed_view: FeedView,
    /// Whether the chat list shows this chat's sub-agent transcripts as rows
    /// nested under it (`Ctrl+O` in the list, `/subagents` in the chat).
    /// `false` = **collapsed**, the default: like [`Chat::feed_view`]'s blocks,
    /// the transcripts are detail you ask for. Additive field — old chat files
    /// read without migration, and a chat nobody expanded writes no new key
    /// (ADR 0006 F12). See spec §11.2.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub children_expanded: bool,
    /// Files attached to the chat (`/file attach`). Their text is injected into
    /// the request's system prompt on every turn, so `/file remove` genuinely
    /// removes them from what the model sees. Additive field — old chat files
    /// read without migration. See docs/file-attachments.md, spec §9.7.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
    /// Files stored with the chat — what `python_exec` saved to `/w/out` — whose bytes
    /// live in `data/files/<chat-id>/`. Not part of any request: the chat lists them for
    /// the user. Additive field — old chat files read without migration, and a chat with
    /// no files writes no new key (ADR 0006 F12). See docs/history/sandbox-file-exchange.md,
    /// spec §9.7.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<ChatFile>,
    /// Deleted exchanges (`Ctrl+E`/`Ctrl+R`). Stored in the chat file only for
    /// **manual** recovery (editing JSON) in rare cases where something important
    /// was deleted; not used by the UI and not restored automatically.
    /// See spec §11.7.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deleted: Vec<DeletedExchange>,
    /// The background auto-reflection watermark index: how many leading `messages`
    /// have already been covered by reflection. The digest is built only from the
    /// "tail" `messages[wm..]` — so each cycle doesn't re-read the same early
    /// material (otherwise duplicate insights would pile up). Lives with the chat →
    /// survives a restart; history truncation (`Ctrl+R`/`Ctrl+E`) is handled by
    /// clamping on read. `None`/old files — from the start.
    /// See docs/history/refinements.md (stage 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflected_upto: Option<usize>,
    /// When background auto-reflection last ran (a reference point; future work —
    /// filtering behavioral signals by window). `None`/old files — never ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflected_at: Option<DateTime<Utc>>,
    /// The rolling summary of the older part of the conversation, when one has
    /// been made. `messages` is **never** edited by compression — this only
    /// changes what a request carries (see [`Chat::compaction_view`]), so the
    /// feed, search, export and every other consumer keep seeing the whole
    /// history. Additive field — old chat files read without migration
    /// (ADR 0006 F12). See docs/research/history-compression.md, spec §6.7.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<Compaction>,
    /// The title was chosen by the user (the list's `F2` editor or
    /// `/rename <title>`). The automatic titling trigger (spec §11.2) never
    /// touches such a chat — a person's choice outranks a model's. Model-written
    /// titles, requested or automatic, do **not** set this. Additive field —
    /// old chat files read without migration, and an untouched chat writes no
    /// new key (ADR 0006 F12).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub renamed_manually: bool,
    /// A background run's **task notification** landed here while the chat
    /// was not the open one (spec §9.3.2, §11.2): the list marks the row
    /// *unread* until the chat is opened, which clears it. Only the
    /// orchestrator sets and clears it — the delivery and the activation.
    /// Additive field — old chat files read without migration, and a chat
    /// with nothing unread writes no new key (ADR 0006 F12).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unread: bool,
    /// The code project attached to this chat (`/project attach`, spec §9.12).
    /// While it is `None` the `code_*` tools are not offered to the model at
    /// all — attaching a directory *is* the consent, so there is no second
    /// switch to forget. Additive field: old chat files read without migration,
    /// and a chat with no project writes no new key (ADR 0006 F12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<crate::entities::workspace::Workspace>,
    /// Soft delete.
    #[serde(default)]
    pub is_hidden: bool,
}

/// The version a chat file without a `v` field is: the shape before the first
/// chat-file migration step.
fn chat_schema_v1() -> u32 {
    1
}

/// A rolling summary covering `messages[..upto]`.
///
/// `upto` is a fast path, not the source of truth: the boundary is identified by
/// `boundary_id`, so an edit that shifts indices cannot silently make the
/// summary cover the wrong span. See [`Chat::compaction_view`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Compaction {
    /// The summary text, as the model wrote it.
    pub summary: String,
    /// How many leading messages the summary covers. Always an exchange
    /// boundary — the message at this index is a `User` one — so a request never
    /// splits an assistant turn from its tool results.
    pub upto: usize,
    /// The id of `messages[upto]` when the summary was written. The boundary is
    /// re-found by this id on every read; if the message is gone the summary is
    /// treated as stale and ignored.
    pub boundary_id: Uuid,
    pub compacted_at: DateTime<Utc>,
    /// How many times the summary has been rolled forward (1 = first
    /// compaction). Diagnostic: summary-of-summary degrades with depth.
    pub rolls: u32,
}

/// Which of the feed's foldable blocks are expanded — the view state of one chat
/// (see [`Chat::feed_view`], spec §11.3).
///
/// Both default to `false` = **collapsed**: the feed is scanned for the reply,
/// and reasoning/tool plumbing is detail you ask for. A named type rather than
/// two loose bools because it travels through a command, an event and the feed's
/// render-cache key, where a bare `(bool, bool)` is exactly the pair that gets
/// swapped by accident.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FeedView {
    /// Show the "thoughts" (CoT) block expanded (`Ctrl+T`).
    #[serde(default)]
    pub thoughts: bool,
    /// Show tool-call arguments and results (`Ctrl+O`). The card's header
    /// (`⚒ name(args)`) is drawn either way — collapsing hides the detail, not
    /// the fact that a tool ran.
    #[serde(default)]
    pub tools: bool,
}

impl FeedView {
    /// Everything collapsed — the default. Lets `serde` skip the field entirely
    /// for a chat the user never expanded anything in.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// A snapshot of a deleted exchange (`Ctrl+E`/`Ctrl+R`). This is **not** a message
/// but a container: the deleted messages + the input-box draft at the time of
/// deletion. There's no UI restore — the object exists only for manual JSON edits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeletedExchange {
    /// The moment of deletion (a reference point when manually searching for a
    /// record).
    pub deleted_at: DateTime<Utc>,
    /// The deleted messages. For `Ctrl+E` (deleting an exchange) — the user
    /// message and the assistant's reply; for `Ctrl+R` (regeneration) — the
    /// assistant's reply (and the round's related tool messages).
    pub messages: Vec<Message>,
    /// The input box's content at the time of deletion — before the deleted user
    /// message's text was restored into it (`Ctrl+E`) or a new turn began (`Ctrl+R`).
    pub draft: String,
    /// What triggered the deletion — a behavioral signal about the interlocutor (or
    /// the agent itself). Auto-reflection reads it as indirect evidence ("regenerated
    /// = the reply probably wasn't good enough"). `None` for old records (no migration).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<DeletedCause>,
}

/// The reason an exchange was deleted — a behavioral signal for auto-reflection (§stage 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeletedCause {
    /// `Ctrl+E` — the interlocutor deleted the exchange (didn't like the reply /
    /// changed their mind about asking).
    DeleteExchange,
    /// `Ctrl+R` — the interlocutor regenerated the reply (the reply probably wasn't
    /// good enough).
    Regenerate,
    /// `rewrite_current_message` — the assistant itself rewrote its reply (a signal
    /// about its own behavior, not attributed to the interlocutor).
    Rewrite,
}

impl Chat {
    /// Creates a chat bound to a profile, copying its system message and role
    /// names from it (the chat has its own copy — profile edits don't change
    /// them, spec §10). The greeting (`greeting`) is added by the caller as the
    /// first assistant message (a use-case, M4).
    ///
    /// The profile's sampling **is not copied** into `sampling_override`:
    /// resolution is three-tiered at request time (`Chat → Profile → global`,
    /// spec §8.3), so `sampling_override` stays `None` until explicitly
    /// overridden by the user or the assistant (`set_sampling`, M5).
    pub fn from_profile(profile: &Profile, title: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            v: crate::shared::storage::schema::CHAT_SCHEMA,
            id: Uuid::new_v4(),
            profile_id: profile.id,
            title: title.into(),
            created_at: now,
            modified_at: now,
            system_message: profile.default_system_message.clone(),
            character_names: profile.character_names.clone(),
            messages: Vec::new(),
            sampling_override: None,
            draft: String::new(),
            feed_view: FeedView::default(),
            children_expanded: false,
            attachments: Vec::new(),
            files: Vec::new(),
            deleted: Vec::new(),
            compaction: None,
            reflected_upto: None,
            reflected_at: None,
            renamed_manually: false,
            unread: false,
            workspace: None,
            is_hidden: false,
        }
    }

    /// Appends a message and updates `modified_at`.
    pub fn push_message(&mut self, message: Message) {
        self.messages.push(message);
        self.modified_at = Utc::now();
    }

    /// Lists a stored file, unless the chat already lists one of that name — compared
    /// case-insensitively, as the folder may be restored onto Windows. Returns whether it
    /// was listed: a landing that repeats is a no-op (docs/history/sandbox-file-exchange.md §11 S7).
    pub fn list_file(&mut self, file: ChatFile) -> bool {
        if self
            .files
            .iter()
            .any(|f| crate::entities::chat_file::same_name(&f.name, &file.name))
        {
            return false;
        }
        self.files.push(file);
        true
    }

    /// Whether the chat holds no conversation content at all: no messages, no
    /// deleted-exchange tombstones (restorable content), no compaction summary.
    /// Such a chat — the first-launch default one included — does not bind its
    /// profile to the scaffold language (axis A, spec §10): everything
    /// language-bearing it carries (default title, the system-message copy) is
    /// a re-derivable localized default. The draft, attachments, workspace and
    /// sampling override deliberately don't count: they are the user's, not
    /// the agent's — the same rule that keeps RAG documents out of the lock.
    pub fn is_pristine(&self) -> bool {
        self.messages.is_empty() && self.deleted.is_empty() && self.compaction.is_none()
    }

    /// Records a deleted exchange (`Ctrl+E`/`Ctrl+R`) into the `deleted` collection
    /// for manual recovery. An empty message set is ignored. The new entry is
    /// inserted at the **front** of the collection (recent deletions are faster to
    /// find). `modified_at` is **not** touched here — the truncation operations
    /// themselves update it.
    pub fn record_deleted(&mut self, messages: Vec<Message>, draft: String, cause: DeletedCause) {
        if messages.is_empty() {
            return;
        }
        self.deleted.insert(
            0,
            DeletedExchange {
                deleted_at: Utc::now(),
                messages,
                draft,
                cause: Some(cause),
            },
        );
    }

    /// What a request should carry: the rolling summary and the index the
    /// verbatim messages start at, or `None` to send the whole history.
    ///
    /// `enabled` is the master switch (`config.compaction.enabled`, fork F10):
    /// when off this returns `None` unconditionally, so the feature is inert and
    /// the stored summary is merely dormant — nothing is discarded.
    ///
    /// The boundary is re-found **by id**, not trusted from `upto`. `Ctrl+E` and
    /// `Ctrl+R` truncate at the tail, so the boundary normally survives and the
    /// stored index is still right; the id lookup is what keeps the rare case
    /// honest. If the boundary message is gone the summary can no longer be
    /// placed, so it is ignored (the full history is sent) and the next
    /// compaction rebuilds it — a temporarily longer prompt, never a summary
    /// silently covering the wrong span.
    pub fn compaction_view(&self, enabled: bool) -> Option<(&str, usize)> {
        if !enabled {
            return None;
        }
        let c = self.compaction.as_ref()?;
        // The fast path: the stored index still points at the same message.
        let upto = if self
            .messages
            .get(c.upto)
            .is_some_and(|m| m.id == c.boundary_id)
        {
            c.upto
        } else {
            self.messages.iter().position(|m| m.id == c.boundary_id)?
        };
        // A summary covering nothing is not worth a block in the prompt.
        (upto > 0).then_some((c.summary.as_str(), upto))
    }

    /// A short chat card (for the list/overlay, without copying messages),
    /// with its sub-agent transcripts as child cards in the order the calls
    /// were made (spec §11.2).
    pub fn summary(&self) -> ChatSummary {
        ChatSummary {
            id: self.id,
            profile_id: self.profile_id,
            title: self.title.clone(),
            created_at: self.created_at,
            modified_at: self.modified_at,
            message_count: visible_message_count(&self.messages),
            children: self.children().map(ChildSummary::of).collect(),
            children_expanded: self.children_expanded,
            unread: self.unread,
        }
    }

    /// The sub-agent transcripts this chat holds, in the order the calls were
    /// made — the live messages only: a transcript in the `deleted` archive is
    /// as gone as the exchange it belonged to (spec §9.3.2).
    pub fn children(&self) -> impl Iterator<Item = &SubagentRun> {
        self.messages
            .iter()
            .flat_map(|m| m.tool_calls.iter())
            .filter_map(|r| r.subagent.as_deref())
    }

    /// The sub-agent transcript with this id, if it is one of this chat's.
    pub fn child(&self, id: Uuid) -> Option<&SubagentRun> {
        self.children().find(|r| r.id == id)
    }

    /// Mutable access to the sub-agent transcript with this id.
    pub fn child_mut(&mut self, id: Uuid) -> Option<&mut SubagentRun> {
        self.messages
            .iter_mut()
            .flat_map(|m| m.tool_calls.iter_mut())
            .filter_map(|r| r.subagent.as_deref_mut())
            .find(|r| r.id == id)
    }

    /// [`Self::child_mut`] over the live messages **and** the deleted
    /// archive: where a background run lands when the exchange that started
    /// it was taken back while it was out (spec §9.3.2,
    /// docs/research/background-subagents.md fork F8).
    pub fn child_mut_including_deleted(&mut self, id: Uuid) -> Option<&mut SubagentRun> {
        self.messages
            .iter_mut()
            .chain(self.deleted.iter_mut().flat_map(|d| d.messages.iter_mut()))
            .flat_map(|m| m.tool_calls.iter_mut())
            .filter_map(|r| r.subagent.as_deref_mut())
            .find(|r| r.id == id)
    }

    /// Gives every sub-agent transcript a fresh id — what a **clone** must do,
    /// since the copied messages carry the transcripts along and two chats
    /// answering to one `chat://` prefix would make the reference ambiguous
    /// (docs/research/subagent-chats.md §3.6). The archive's are re-id'd too;
    /// they are copied with the rest.
    pub fn reid_children(&mut self) {
        let runs = self
            .messages
            .iter_mut()
            .chain(self.deleted.iter_mut().flat_map(|d| d.messages.iter_mut()))
            .flat_map(|m| m.tool_calls.iter_mut())
            .filter_map(|r| r.subagent.as_deref_mut());
        for run in runs {
            run.id = Uuid::new_v4();
        }
    }
}

/// How many messages a chat card says the conversation has: the bubbles the
/// feed draws, not the raw storage rows (spec §11.2). One question answered
/// through an agentic loop stores dozens of rows — an assistant message per
/// round plus a `Tool` message per result — while the feed shows two: the
/// question, and one stitched reply. A list row saying "34 msg" over that
/// conversation is noise, so the card counts what the user perceives.
///
/// The rule mirrors the feed's projection (`FeedMessage::from_messages`,
/// spec §11.3/§9.3), which FSD keeps out of reach here — dependencies point
/// strictly downward, and `entities` cannot import `widgets`. Two statements
/// of one rule is the drift the lessons warn about, so the
/// `bubble_count_agrees_with_the_list_counter` test next to the feed pins
/// them together: tool messages draw no bubble of their own, a `System`
/// entry draws a note row (a dialogue transcript's director intervention,
/// spec §9.13) that also ends any assistant stitching, and consecutive
/// assistant rounds are one bubble unless a round opts out via
/// [`Message::new_bubble`].
pub fn visible_message_count(messages: &[Message]) -> usize {
    let mut count = 0;
    let mut in_assistant_bubble = false;
    for m in messages {
        match m.role {
            MessageRole::Tool => {}
            MessageRole::User | MessageRole::System => {
                count += 1;
                in_assistant_bubble = false;
            }
            MessageRole::Assistant => {
                if !in_assistant_bubble || m.new_bubble {
                    count += 1;
                }
                in_assistant_bubble = true;
            }
        }
    }
    count
}

/// A short chat card for the list/overlay (no messages). See spec §11.2.
/// A domain view-projection, lives in `entities` so it can be used by both `app`
/// (events) and `widgets` (rendering) — dependency strictly downward.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatSummary {
    pub id: Uuid,
    /// Which companion this conversation belongs to. The list itself is
    /// deliberately cross-profile (spec §11.2), but the profile boundary
    /// (spec §9.5) has to be drawable from a snapshot — a `chat://` reference
    /// only resolves inside the current profile (spec §11.3).
    #[serde(default)]
    pub profile_id: Uuid,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    pub message_count: usize,
    /// The chat's sub-agent transcripts, in call order — the list draws them
    /// nested under this row (spec §11.2). Additive: a snapshot without the
    /// key reads as a chat with none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ChildSummary>,
    /// Whether the list shows the `children` rows — the stored per-chat fold,
    /// [`Chat::children_expanded`]. A search that matches a transcript outranks
    /// it (the widget's tree rule, spec §11.2).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub children_expanded: bool,
    /// A background run's result landed while the chat was not open
    /// ([`Chat::unread`]): the row is marked until the chat is opened.
    /// Additive: a snapshot without the key reads as read.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unread: bool,
}

impl ChatSummary {
    /// A bare card for tests: fresh id, nil profile, now-timestamps, no
    /// messages, no transcripts, collapsed. Shared by the list, picker and
    /// screen tests — each keeping a copy of this literal is the sliding
    /// self-duplication the gate measures (docs/lessons.md §2); tests mutate
    /// the fields they are about.
    #[cfg(test)]
    pub fn fixture(title: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            profile_id: Uuid::nil(),
            title: title.to_string(),
            created_at: Utc::now(),
            modified_at: Utc::now(),
            message_count: 0,
            children: Vec::new(),
            children_expanded: false,
            unread: false,
        }
    }

    /// A card for one of this chat's transcripts, shaped like a chat card so
    /// the surfaces that take one — the reference picker, the list's rows —
    /// need no second type. It belongs to the same profile; it has no children.
    pub fn child_card(&self, child: &ChildSummary) -> ChatSummary {
        ChatSummary {
            id: child.id,
            profile_id: self.profile_id,
            title: child.title.clone(),
            created_at: child.created_at,
            modified_at: child.finished_at.unwrap_or(child.created_at),
            message_count: child.message_count,
            children: Vec::new(),
            children_expanded: false,
            unread: false,
        }
    }

    /// This chat's transcripts plus itself, as cards (the `chat://` address
    /// book is built from it).
    pub fn child_ids(&self) -> impl Iterator<Item = Uuid> + '_ {
        self.children.iter().map(|c| c.id)
    }
}

/// A sub-agent transcript's card under its parent (spec §11.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChildSummary {
    pub id: Uuid,
    pub title: String,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    pub message_count: usize,
    /// How the run ended; `None` — interrupted before it could say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RunOutcome>,
    /// The run is in progress right now (docs/subagent-live.md §3.3): the
    /// card comes from the orchestrator's in-flight mirror, not from a chat
    /// file — `Chat::summary()` never sets it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub running: bool,
    /// The run was started in the background (spec §9.3.2): with no outcome
    /// and not running, its row reads *unfinished* — the app was closed while
    /// it was out — rather than *interrupted*, which is a turn's word.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub background: bool,
}

impl ChildSummary {
    pub fn of(run: &SubagentRun) -> Self {
        Self {
            id: run.id,
            title: run.title.clone(),
            created_at: run.created_at,
            finished_at: run.finished_at,
            message_count: visible_message_count(&run.messages),
            outcome: run.outcome,
            running: false,
            background: run.background,
        }
    }
}

#[cfg(test)]
mod tests {
    /// The additive contract for `workspace`, mirroring `renamed_manually`'s:
    /// a chat file written before the field existed still loads, a chat with no
    /// project writes no new key, and a set value survives the round trip. All
    /// three are what let ADR 0006 keep `CHAT_SCHEMA` at 1.
    #[test]
    fn workspace_is_additive_and_round_trips() {
        let profile = Profile::new("P", "sys");
        let chat = Chat::from_profile(&profile, "t");
        let json = serde_json::to_string(&chat).unwrap();
        assert!(
            !json.contains("workspace"),
            "a chat with no project must not write the key: {json}"
        );

        // A file from before the field existed.
        let old: Chat = serde_json::from_str(&json).unwrap();
        assert_eq!(old.workspace, None);

        let mut attached = chat.clone();
        attached.workspace = Some(crate::entities::workspace::Workspace::new("D:/proj"));
        let json = serde_json::to_string(&attached).unwrap();
        assert!(json.contains("workspace"), "{json}");
        let back: Chat = serde_json::from_str(&json).unwrap();
        assert_eq!(back.workspace.unwrap().root, "D:/proj");
    }

    /// The additive contract for `children_expanded` (spec §11.2), mirroring
    /// `feed_view`'s: a chat file from before the field loads collapsed, a chat
    /// nobody expanded writes no new key, and the expanded state round-trips —
    /// on the chat and on its summary alike.
    #[test]
    fn children_expanded_is_additive_and_round_trips() {
        let profile = Profile::new("P", "sys");
        let chat = Chat::from_profile(&profile, "t");
        let json = serde_json::to_value(&chat).unwrap();
        assert!(
            json.get("children_expanded").is_none(),
            "a chat nobody expanded writes no new key: {json}"
        );
        let old: Chat = serde_json::from_value(json).unwrap();
        assert!(!old.children_expanded, "old files load collapsed");

        let mut expanded = chat;
        expanded.children_expanded = true;
        let json = serde_json::to_value(&expanded).unwrap();
        assert_eq!(json.get("children_expanded"), Some(&true.into()));
        let back: Chat = serde_json::from_value(json).unwrap();
        assert!(back.children_expanded);
        assert!(
            back.summary().children_expanded,
            "the card carries the fold"
        );
        let summary_json = serde_json::to_value(back.summary()).unwrap();
        let summary: ChatSummary = serde_json::from_value(summary_json).unwrap();
        assert!(summary.children_expanded);
    }

    /// The additive contract for `unread` (spec §9.3.2, §11.2), the same
    /// shape as the fold's: an old file loads read, a chat with nothing
    /// unread writes no key, and the mark round-trips onto the card.
    #[test]
    fn unread_is_additive_and_round_trips() {
        let profile = Profile::new("P", "sys");
        let chat = Chat::from_profile(&profile, "t");
        let json = serde_json::to_value(&chat).unwrap();
        assert!(
            json.get("unread").is_none(),
            "a chat with nothing unread writes no new key: {json}"
        );
        let old: Chat = serde_json::from_value(json).unwrap();
        assert!(!old.unread, "old files load read");

        let mut marked = chat;
        marked.unread = true;
        let json = serde_json::to_value(&marked).unwrap();
        assert_eq!(json.get("unread"), Some(&true.into()));
        let back: Chat = serde_json::from_value(json).unwrap();
        assert!(back.unread);
        assert!(back.summary().unread, "the card carries the mark");
        let summary_json = serde_json::to_value(back.summary()).unwrap();
        let summary: ChatSummary = serde_json::from_value(summary_json).unwrap();
        assert!(summary.unread);
    }

    use super::*;
    use crate::entities::message::Message;

    /// A chat of `n` user/assistant pairs with a summary covering `[..upto]`.
    fn compacted(n: usize, upto: usize) -> Chat {
        let p = Profile::new("P", "sys");
        let mut chat = Chat::from_profile(&p, "c");
        for i in 0..n {
            chat.push_message(Message::user(format!("u{i}")));
            chat.push_message(Message::assistant(format!("a{i}")));
        }
        chat.compaction = Some(Compaction {
            summary: "ранее обсудили X".into(),
            upto,
            boundary_id: chat.messages[upto].id,
            compacted_at: Utc::now(),
            rolls: 1,
        });
        chat
    }

    #[test]
    fn compaction_view_is_inert_when_the_switch_is_off() {
        // Fork F10: off must not merely stop compacting — it must stop *using*
        // what is stored, so the request is what it was before the feature.
        let chat = compacted(4, 4);
        assert_eq!(chat.compaction_view(false), None);
        assert_eq!(chat.compaction_view(true), Some(("ранее обсудили X", 4)));
    }

    #[test]
    fn compaction_view_refinds_the_boundary_after_an_index_shift() {
        // `upto` is a fast path; the id is the truth. Deleting a message ahead of
        // the boundary shifts every later index, and trusting the stored number
        // would silently move the summary's coverage.
        let mut chat = compacted(4, 4);
        chat.messages.remove(0);
        assert_eq!(chat.compaction_view(true), Some(("ранее обсудили X", 3)));
    }

    #[test]
    fn compaction_view_drops_a_summary_whose_boundary_is_gone() {
        let mut chat = compacted(4, 4);
        chat.messages.remove(4);
        assert_eq!(chat.compaction_view(true), None);
    }

    #[test]
    fn compaction_view_ignores_a_summary_that_covers_nothing() {
        let mut chat = compacted(4, 4);
        let first = chat.messages[0].id;
        chat.compaction.as_mut().unwrap().upto = 0;
        chat.compaction.as_mut().unwrap().boundary_id = first;
        assert_eq!(chat.compaction_view(true), None);
    }

    #[test]
    fn compaction_reads_old_chat_files_and_stays_out_of_new_ones() {
        // Additive field, no migration (ADR 0006 F12).
        let p = Profile::new("P", "sys");
        let chat = Chat::from_profile(&p, "c");
        let json = serde_json::to_value(&chat).unwrap();
        assert!(
            json.get("compaction").is_none(),
            "a chat that was never compacted must write no new key"
        );
        let back: Chat = serde_json::from_value(json).unwrap();
        assert!(back.compaction.is_none());
    }

    #[test]
    fn feed_view_reads_old_chat_files_and_stays_out_of_new_ones() {
        // Additive field, no migration (ADR 0006 F12): a chat file written
        // before it existed loads with everything collapsed...
        let p = Profile::new("P", "sys");
        let chat = Chat::from_profile(&p, "Чат");
        let mut json: serde_json::Value = serde_json::to_value(&chat).unwrap();
        assert!(
            json.get("feed_view").is_none(),
            "a chat nobody expanded anything in writes no new key: {json}"
        );
        let back: Chat = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(back.feed_view, FeedView::default());

        // ...and an expanded one round-trips.
        let mut expanded = chat;
        expanded.feed_view = FeedView {
            thoughts: true,
            tools: false,
        };
        json = serde_json::to_value(&expanded).unwrap();
        assert!(json.get("feed_view").is_some());
        let back: Chat = serde_json::from_value(json).unwrap();
        assert_eq!(back.feed_view, expanded.feed_view);
    }

    #[test]
    fn from_profile_copies_fields_but_not_sampling() {
        let mut p = Profile::new("Carlos", "Ты — Карлос.");
        p.default_sampling = Some(SamplingConfig {
            temperature: Some(0.9),
            ..Default::default()
        });
        let chat = Chat::from_profile(&p, "Новый чат");
        assert_eq!(chat.profile_id, p.id);
        assert_eq!(chat.system_message, "Ты — Карлос.");
        assert_eq!(chat.character_names, p.character_names);
        // The profile's sampling is NOT copied into the override — it's resolved
        // three-tiered at request time (spec §8.3).
        assert_eq!(chat.sampling_override, None);
        assert!(chat.messages.is_empty());
    }

    #[test]
    fn push_message_updates_modified() {
        let p = Profile::new("X", "s");
        let mut chat = Chat::from_profile(&p, "t");
        let before = chat.modified_at;
        chat.push_message(Message::user("hi"));
        assert_eq!(chat.messages.len(), 1);
        assert!(chat.modified_at >= before);
    }

    /// spec §11.2: the card counts the feed's bubbles, not storage rows. An
    /// agentic exchange — the question, a pure tool-call round, its result,
    /// the answering round — reads as two messages, not four; a followup
    /// flagged `new_bubble` (spec §9.3) opens a third; a system row is a note
    /// row (a dialogue's director intervention, spec §9.13) and counts, also
    /// ending the assistant's stitching; tool rows draw nothing; the next
    /// question starts a new bubble again.
    #[test]
    fn visible_message_count_folds_rounds_and_tool_results() {
        let mut messages = vec![
            Message::user("q"),
            Message::assistant(""),
            Message::new(MessageRole::Tool, "result"),
            Message::assistant("the answer"),
        ];
        assert_eq!(visible_message_count(&messages), 2);

        let mut followup = Message::assistant("more");
        followup.new_bubble = true;
        messages.push(followup);
        assert_eq!(visible_message_count(&messages), 3);

        messages.push(Message::new(MessageRole::System, "sys"));
        assert_eq!(visible_message_count(&messages), 4);

        // The note row broke the stitching: another assistant round after it
        // is a new bubble even without `new_bubble`.
        messages.push(Message::assistant("post-note"));
        assert_eq!(visible_message_count(&messages), 5);

        messages.push(Message::user("q2"));
        messages.push(Message::assistant("a2"));
        assert_eq!(visible_message_count(&messages), 7);
    }

    /// Both cards go through [`visible_message_count`]: the chat's own and
    /// its transcripts' — a sub-agent run stores its rounds exactly like a
    /// chat (spec §9.3.2), so its row inflates the same way without this.
    #[test]
    fn summaries_count_visible_messages_not_rows() {
        let p = Profile::new("P", "sys");
        let mut chat = Chat::from_profile(&p, "t");
        assert_eq!(chat.summary().message_count, 0);
        chat.push_message(Message::user("q"));
        chat.push_message(Message::assistant(""));
        chat.push_message(Message::new(MessageRole::Tool, "result"));
        chat.push_message(Message::assistant("a"));
        assert_eq!(chat.summary().message_count, 2);

        let mut run = SubagentRun::fixture("R", &["instruction"]);
        run.messages.push(Message::assistant(""));
        run.messages.push(Message::new(MessageRole::Tool, "result"));
        run.messages.push(Message::assistant("verdict"));
        assert_eq!(ChildSummary::of(&run).message_count, 2);
    }

    #[test]
    fn record_deleted_appends_with_draft_and_skips_empty() {
        let p = Profile::new("X", "s");
        let mut chat = Chat::from_profile(&p, "t");
        chat.record_deleted(vec![], "ignored".into(), DeletedCause::DeleteExchange);
        assert!(chat.deleted.is_empty()); // an empty set isn't recorded

        chat.record_deleted(
            vec![Message::user("hi"), Message::assistant("hello")],
            "набранный, но не отправленный текст".into(),
            DeletedCause::DeleteExchange,
        );
        assert_eq!(chat.deleted.len(), 1);
        assert_eq!(chat.deleted[0].messages.len(), 2);
        assert_eq!(chat.deleted[0].draft, "набранный, но не отправленный текст");
        assert_eq!(chat.deleted[0].cause, Some(DeletedCause::DeleteExchange));
    }

    /// What binds a profile to its scaffold language (spec §10): conversation
    /// content only. The user's own state on an untouched chat doesn't count.
    #[test]
    fn is_pristine_tracks_conversation_content_only() {
        let p = Profile::new("X", "s");
        let mut chat = Chat::from_profile(&p, "t");
        chat.draft = "typed but unsent".into();
        chat.sampling_override = Some(SamplingConfig::default());
        chat.renamed_manually = true;
        assert!(chat.is_pristine(), "user-side state is not content");

        // Any message counts — a greeting copy included.
        let mut with_message = Chat::from_profile(&p, "t");
        with_message.push_message(Message::assistant("Здравствуйте!"));
        assert!(!with_message.is_pristine());

        // A deleted-exchange tombstone is restorable conversation content.
        let mut with_tombstone = Chat::from_profile(&p, "t");
        with_tombstone.record_deleted(
            vec![Message::user("hi")],
            String::new(),
            DeletedCause::DeleteExchange,
        );
        assert!(!with_tombstone.is_pristine());

        // So is a compaction summary.
        let mut with_summary = Chat::from_profile(&p, "t");
        with_summary.compaction = Some(Compaction {
            summary: "сводка".into(),
            upto: 0,
            boundary_id: Uuid::new_v4(),
            compacted_at: Utc::now(),
            rolls: 1,
        });
        assert!(!with_summary.is_pristine());
    }

    #[test]
    fn renamed_manually_is_additive_and_round_trips() {
        // Additive field, no migration (ADR 0006 F12): an old chat file has no
        // key and reads as `false`, an untouched chat writes none...
        let old = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "profile_id": "00000000-0000-0000-0000-000000000002",
            "title": "old chat",
            "created_at": "2026-01-01T00:00:00Z",
            "modified_at": "2026-01-01T00:00:00Z",
            "system_message": "s",
            "messages": []
        }"#;
        let loaded: Chat = serde_json::from_str(old).unwrap();
        assert!(!loaded.renamed_manually);
        let p = Profile::new("X", "s");
        let chat = Chat::from_profile(&p, "t");
        let json = serde_json::to_string(&chat).unwrap();
        assert!(
            !json.contains("renamed_manually"),
            "an untouched chat must write no new key: {json}"
        );

        // ...and a renamed one round-trips.
        let mut renamed = chat;
        renamed.renamed_manually = true;
        let json = serde_json::to_string(&renamed).unwrap();
        let back: Chat = serde_json::from_str(&json).unwrap();
        assert!(back.renamed_manually);
    }

    #[test]
    fn deleted_empty_is_not_serialized() {
        let p = Profile::new("X", "s");
        let chat = Chat::from_profile(&p, "t");
        let json = serde_json::to_string(&chat).unwrap();
        assert!(!json.contains("deleted"));
    }

    #[test]
    fn files_are_additive_and_not_serialized_when_empty() {
        use crate::entities::chat_file::{ChatFile, FileOrigin};
        let p = Profile::new("X", "s");
        let chat = Chat::from_profile(&p, "t");
        let json = serde_json::to_string(&chat).unwrap();
        assert!(!json.contains("\"files\""), "no files, no key: {json}");

        // A chat file from before stored files reads without migration.
        let old = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "profile_id": "00000000-0000-0000-0000-000000000002",
            "title": "old chat",
            "created_at": "2026-01-01T00:00:00Z",
            "modified_at": "2026-01-01T00:00:00Z",
            "system_message": "s",
            "messages": []
        }"#;
        let loaded: Chat = serde_json::from_str(old).unwrap();
        assert!(loaded.files.is_empty());

        let mut with = chat.clone();
        with.files.push(ChatFile::new(
            "chart.png",
            FileOrigin::Sandbox,
            b"\x89PNG\r\n\x1a\n",
        ));
        let json = serde_json::to_string(&with).unwrap();
        let back: Chat = serde_json::from_str(&json).unwrap();
        assert_eq!(back.files, with.files);
    }

    #[test]
    fn attachments_are_additive_and_not_serialized_when_empty() {
        use crate::entities::attachment::{AttachMode, Attachment};
        let p = Profile::new("X", "s");
        let chat = Chat::from_profile(&p, "t");
        assert!(chat.attachments.is_empty());
        let json = serde_json::to_string(&chat).unwrap();
        assert!(
            !json.contains("attachments"),
            "empty list doesn't clutter the file"
        );

        // An old chat file (no `attachments` key) reads without migration.
        let old = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "profile_id": "00000000-0000-0000-0000-000000000002",
            "title": "old chat",
            "created_at": "2026-01-01T00:00:00Z",
            "modified_at": "2026-01-01T00:00:00Z",
            "system_message": "s",
            "messages": []
        }"#;
        let loaded: Chat = serde_json::from_str(old).unwrap();
        assert!(loaded.attachments.is_empty());

        // A non-empty list round-trips.
        let mut with = chat.clone();
        with.attachments.push(Attachment::new(
            "notes.md",
            "/tmp/notes.md",
            "содержимое".into(),
            20,
            AttachMode::Inline,
        ));
        let json = serde_json::to_string(&with).unwrap();
        let back: Chat = serde_json::from_str(&json).unwrap();
        assert_eq!(with, back);
    }

    #[test]
    fn serde_roundtrip() {
        let p = Profile::new("X", "s");
        let mut chat = Chat::from_profile(&p, "t");
        chat.push_message(Message::user("hi"));
        chat.push_message(Message::assistant("hello"));
        chat.record_deleted(
            vec![Message::user("удалённое")],
            "черновик".into(),
            DeletedCause::Regenerate,
        );
        let json = serde_json::to_string(&chat).unwrap();
        let back: Chat = serde_json::from_str(&json).unwrap();
        assert_eq!(chat, back);
    }

    #[test]
    fn deserializes_old_json_without_reflection_watermark() {
        // An old chat file (pre-stage-3) has no reflection watermark fields — reads
        // without migration, the fields default to `None`. And aren't serialized when empty.
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "profile_id": "00000000-0000-0000-0000-000000000002",
            "title": "старый чат",
            "created_at": "2026-01-01T00:00:00Z",
            "modified_at": "2026-01-01T00:00:00Z",
            "system_message": "s",
            "messages": []
        }"#;
        let chat: Chat = serde_json::from_str(json).unwrap();
        assert_eq!(chat.reflected_upto, None);
        assert_eq!(chat.reflected_at, None);
        // Empty watermark fields don't clutter the JSON (skip_serializing_if).
        let out = serde_json::to_string(&chat).unwrap();
        assert!(!out.contains("reflected_upto"));
        assert!(!out.contains("reflected_at"));
    }
}

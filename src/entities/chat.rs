//! A chat: the message list, a profile binding, the active system message.
//! See spec §5.1.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entities::attachment::Attachment;
use crate::entities::message::Message;
use crate::entities::profile::{CharacterNames, Profile};
use crate::entities::sampling::SamplingConfig;

/// A chat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chat {
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
    /// Files attached to the chat (`/file attach`). Their text is injected into
    /// the request's system prompt on every turn, so `/file remove` genuinely
    /// removes them from what the model sees. Additive field — old chat files
    /// read without migration. See docs/file-attachments.md, spec §9.7.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
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
    /// Soft delete.
    #[serde(default)]
    pub is_hidden: bool,
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
            attachments: Vec::new(),
            deleted: Vec::new(),
            compaction: None,
            reflected_upto: None,
            reflected_at: None,
            renamed_manually: false,
            is_hidden: false,
        }
    }

    /// Appends a message and updates `modified_at`.
    pub fn push_message(&mut self, message: Message) {
        self.messages.push(message);
        self.modified_at = Utc::now();
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

    /// A short chat card (for the list/overlay, without copying messages).
    pub fn summary(&self) -> ChatSummary {
        ChatSummary {
            id: self.id,
            profile_id: self.profile_id,
            title: self.title.clone(),
            created_at: self.created_at,
            modified_at: self.modified_at,
            message_count: self.messages.len(),
        }
    }
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
}

#[cfg(test)]
mod tests {
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

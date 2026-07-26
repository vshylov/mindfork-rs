//! A chat: the message list, a profile binding, the active system message.
//! See spec §5.1.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
    /// Soft delete.
    #[serde(default)]
    pub is_hidden: bool,
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
            deleted: Vec::new(),
            reflected_upto: None,
            reflected_at: None,
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

    /// A short chat card (for the list/overlay, without copying messages).
    pub fn summary(&self) -> ChatSummary {
        ChatSummary {
            id: self.id,
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
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    pub message_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::message::Message;

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
    fn deleted_empty_is_not_serialized() {
        let p = Profile::new("X", "s");
        let chat = Chat::from_profile(&p, "t");
        let json = serde_json::to_string(&chat).unwrap();
        assert!(!json.contains("deleted"));
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

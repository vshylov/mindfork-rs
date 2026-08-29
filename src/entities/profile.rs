//! The AI interlocutor's profile and related types. See spec §5.1, §10.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entities::sampling::SamplingConfig;
use crate::shared::i18n::Lang;

/// A tool id (may become an enum at M5).
pub type ToolId = String;

/// Displayed role names: what the chat feed and the `F5` conversation export call
/// the interlocutors. **Empty = not set** — the surface then falls back to its own
/// localized default (`YOU`/`ASSISTANT` in the feed, `User:`/`Assistant:` in the
/// export), so the names follow the interface language until the user overrides
/// them. See spec §5.1, §11.3.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CharacterNames {
    pub user: String,
    pub assistant: String,
    /// Heads the system bubble a sub-agent transcript opens with (spec §11.3);
    /// ordinary chats still never draw their system message.
    pub system: String,
}

impl CharacterNames {
    /// The user's display name, or `None` when not set (the surface falls back to
    /// its localized default).
    pub fn user_name(&self) -> Option<&str> {
        non_empty(&self.user)
    }

    /// The assistant's display name, or `None` when not set.
    pub fn assistant_name(&self) -> Option<&str> {
        non_empty(&self.assistant)
    }

    /// The system role's display name, or `None` when not set. Drawn on the
    /// system bubble a sub-agent transcript opens with (spec §11.3).
    pub fn system_name(&self) -> Option<&str> {
        non_empty(&self.system)
    }
}

/// A trimmed non-empty view of a name field (`""`/whitespace — "not set").
fn non_empty(s: &str) -> Option<&str> {
    let s = s.trim();
    (!s.is_empty()).then_some(s)
}

/// One record of the profile's language-model history (spec §9.14): from
/// `changed_at` on, the profile's exchanges were answered by `model` in
/// engine mode `mode`. Appended by the orchestrator after a completed
/// exchange whose model name is known and differs — by name *or* mode —
/// from the newest record (a record whose stored mode went stale would lie
/// about it). Lives in `data.db` (`llm_history`), not in `profiles.json`;
/// deliberately named `Llm*`, never `*Model*` alone — "model" by itself is
/// already claimed by the self-model (spec §17).
#[derive(Debug, Clone, PartialEq)]
pub struct LlmChange {
    pub changed_at: chrono::DateTime<chrono::Utc>,
    pub model: String,
    pub mode: crate::shared::config::ServerMode,
}

/// The AI interlocutor's profile: system message, names, tools, defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub id: Uuid,
    pub name: String,
    pub default_system_message: String,
    /// The agent's **scaffold** language (background-task prompts, the
    /// "self-model" scaffold, tool results — text that the *model* reads; axis A,
    /// see docs/history/i18n.md). Does NOT govern the model's reply language (that's
    /// `default_system_message`'s territory) and is NOT tied to the interface
    /// language (axis B). Chosen at profile creation and **locked** once the profile
    /// has data (chats / "self-model" / notes) — so the whole profile's memory stays
    /// in one language. Old `profiles.json` files read as `Ru` (their data is Russian).
    #[serde(default)]
    pub language: Lang,
    /// The impersonation profile (the user persona the model writes a reply on behalf
    /// of, `Ctrl+U`) used by this profile's chats. `None`/a dangling id — the shared
    /// default text. The profiles themselves live in
    /// [`AppConfig.impersonation_profiles`](crate::shared::config::AppConfig). See spec §11.8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impersonation_profile_id: Option<Uuid>,
    /// **Legacy** (superseded by [`Self::impersonation_profile_id`]): the impersonation
    /// system message stored on the assistant profile itself. On startup a non-empty
    /// value is migrated once into a named impersonation profile
    /// (`Orchestrator::migrate_impersonation_profiles`); the field is kept so the
    /// migration is idempotent across app versions and no data is lost on a downgrade.
    /// Nothing reads it for prompt building any more.
    #[serde(default)]
    pub impersonation_system_message: String,
    /// Role names shown in this profile's chats (feed headers, `F5` export).
    /// **The source of truth for display** — resolved at render time, so editing
    /// them applies to existing chats too (unlike [`crate::entities::chat::Chat`]'s
    /// own copy, which is created-at-the-time state). Empty fields fall back to the
    /// interface language's defaults. See spec §5.1, §11.3.
    #[serde(default)]
    pub character_names: CharacterNames,
    /// The assistant's greeting message (the first message in a new chat).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub greeting: Option<String>,
    #[serde(default)]
    pub enabled_tools: Vec<ToolId>,
    /// Tools already "offered" to the profile (the known-tools registry). Needed so
    /// that when new tools are added to the application, they can be enabled for
    /// existing profiles **without** re-enabling ones the user deliberately turned
    /// off. Reconciliation — [`crate::features::profiles::reconcile_tools`]. See spec §9.4.
    #[serde(default)]
    pub known_tools: Vec<ToolId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_sampling: Option<SamplingConfig>,
    /// Soft delete.
    #[serde(default)]
    pub is_hidden: bool,
}

impl Profile {
    /// A new profile with the system message and default values.
    pub fn new(name: impl Into<String>, system_message: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            default_system_message: system_message.into(),
            language: Lang::default(),
            impersonation_profile_id: None,
            impersonation_system_message: String::new(),
            character_names: CharacterNames::default(),
            greeting: None,
            enabled_tools: Vec::new(),
            known_tools: Vec::new(),
            default_sampling: None,
            is_hidden: false,
        }
    }

    /// A short profile card (for the picker overlay, without copying the system
    /// message or the tool set).
    pub fn summary(&self) -> ProfileSummary {
        ProfileSummary {
            id: self.id,
            name: self.name.clone(),
        }
    }
}

/// A short profile card for the picker overlay at chat creation. See spec §11.2.
/// A domain view-projection living in `entities` so it can be used by both `app`
/// (events) and `widgets` (rendering) — dependency strictly downward.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileSummary {
    pub id: Uuid,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_roundtrip() {
        let p = Profile::new("Joyce", "Ты — Джойс.");
        let json = serde_json::to_string(&p).unwrap();
        let back: Profile = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn defaults_applied() {
        let raw = format!(
            r#"{{"id":"{}","name":"X","default_system_message":"s"}}"#,
            Uuid::nil()
        );
        let p: Profile = serde_json::from_str(&raw).unwrap();
        assert_eq!(p.character_names, CharacterNames::default());
        assert!(!p.is_hidden);
        assert!(p.enabled_tools.is_empty());
    }

    /// Role names start out unset — the feed/export then use the interface
    /// language's labels rather than a hardcoded name.
    #[test]
    fn character_names_default_to_unset() {
        let names = CharacterNames::default();
        assert_eq!(names.user_name(), None);
        assert_eq!(names.assistant_name(), None);
        assert_eq!(Profile::new("X", "s").character_names, names);
    }

    #[test]
    fn character_names_trim_and_treat_blank_as_unset() {
        let names = CharacterNames {
            user: "  Гайя  ".into(),
            assistant: "   ".into(),
            system: String::new(),
        };
        assert_eq!(names.user_name(), Some("Гайя"));
        assert_eq!(names.assistant_name(), None);
    }
}

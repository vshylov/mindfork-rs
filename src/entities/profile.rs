//! The AI interlocutor's profile and related types. See spec §5.1, §10.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entities::sampling::SamplingConfig;
use crate::shared::i18n::Lang;

/// A tool id (may become an enum at M5).
pub type ToolId = String;

/// Displayed role names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CharacterNames {
    pub user: String,
    pub assistant: String,
    pub system: String,
}

impl Default for CharacterNames {
    fn default() -> Self {
        Self {
            user: "Вы".to_string(),
            assistant: "Ассистент".to_string(),
            system: "Система".to_string(),
        }
    }
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
    /// The system message for impersonation mode: describes the user persona the
    /// model writes a reply on behalf of (`Ctrl+U`). Empty — the shared default is
    /// used. No tools in this mode. See spec §11.8.
    #[serde(default)]
    pub impersonation_system_message: String,
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
}

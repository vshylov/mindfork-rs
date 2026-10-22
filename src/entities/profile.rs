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

/// The companion features of a profile (spec §10,
/// docs/research/companion-platform.md): each a switch of its own, every one off
/// by default — a profile is an assistant until it opts in, one feature at a time
/// (the owner's decision of 2026-10-21, §8 there). `profiles.json` omits the whole
/// group while it is at its default, so a profile that never opts in is written
/// exactly as before.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompanionFeatures {
    /// A dated row in front of every user message the model reads — when it was
    /// sent, and how long after the previous message (C1 there). Derived from the
    /// messages' own timestamps when the request is built, never stored.
    pub time_rows: bool,
    /// A block of the companion's notes — those about the person and their world,
    /// not its `@self` observations — in the system prompt, picked by the
    /// conversation's opening (C2 (a) there, spec §6.6).
    pub notes_in_prompt: bool,
    /// The titles and ages of the profile's few conversations before this one in
    /// the system prompt, with their `chat://` addresses when the turn offers
    /// `chat_read` (C2 (b) there).
    pub earlier_chats: bool,
    /// Follow-ups the companion plans — `followup_plan`/`followup_cancel` offered,
    /// and a due one delivered as a notice row and a woken turn (C4 (a) there,
    /// spec §9.3.6).
    pub followups: bool,
    /// The quiet hours no follow-up is delivered in, from this local hour…
    pub quiet_from: u8,
    /// …up to this one; equal hours mean none. On by default (§7, rule 3 there).
    pub quiet_until: u8,
    /// The most follow-ups delivered in one local day; the next waits for the
    /// next day (§7, rules 3 and 5 there).
    pub followups_per_day: u8,
    /// A notification when the companion writes unasked and the person is not
    /// looking — the terminal's, and a ntfy push (C5 (a) there, spec §11.12).
    pub notify: bool,
}

/// The default quiet hours, 22:00 to 08:00.
pub const DEFAULT_QUIET_FROM: u8 = 22;
pub const DEFAULT_QUIET_UNTIL: u8 = 8;
/// The default cap of follow-ups delivered a day.
pub const DEFAULT_FOLLOWUPS_PER_DAY: u8 = 3;

impl Default for CompanionFeatures {
    fn default() -> Self {
        Self {
            time_rows: false,
            notes_in_prompt: false,
            earlier_chats: false,
            followups: false,
            quiet_from: DEFAULT_QUIET_FROM,
            quiet_until: DEFAULT_QUIET_UNTIL,
            followups_per_day: DEFAULT_FOLLOWUPS_PER_DAY,
            notify: false,
        }
    }
}

impl CompanionFeatures {
    /// The quiet hours as the follow-up rules read them.
    pub fn quiet_hours(&self) -> crate::entities::followup::QuietHours {
        crate::entities::followup::QuietHours {
            from: self.quiet_from.min(23),
            until: self.quiet_until.min(23),
        }
    }

    /// Whether every feature is at its default (off) — the serializer's skip test.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
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
    /// Other openings a new chat can begin with instead of [`Self::greeting`]
    /// — a character card's `alternate_greetings` (spec §10). `/greeting` in a
    /// chat that holds nothing but its greeting moves to the next. Written only
    /// when there are some, so a profile without them is written as before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternate_greetings: Vec<String>,
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
    /// The companion features this profile has switched on (all off by default).
    #[serde(default, skip_serializing_if = "CompanionFeatures::is_default")]
    pub companion: CompanionFeatures,
    /// Soft delete.
    #[serde(default)]
    pub is_hidden: bool,
    /// Where this profile was forked from (`/profile fork`, spec §10):
    /// `None` for a profile made any other way, and then `profiles.json`
    /// omits it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<ForkOrigin>,
}

/// The profile a fork was made from, as it was at that moment (spec §10,
/// docs/research/companion-platform.md C8): the two share their past up to
/// here — the self-model's versions up to `self_model_version`, the notes, the
/// knowledge base — and diverge after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkOrigin {
    pub profile_id: Uuid,
    /// The parent's name then — it may be renamed or deleted since.
    pub name: String,
    pub at: chrono::DateTime<chrono::Utc>,
    /// The parent's self-model version the fork began from; `None` — the
    /// parent had no self-model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_model_version: Option<u64>,
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
            alternate_greetings: Vec::new(),
            enabled_tools: Vec::new(),
            known_tools: Vec::new(),
            default_sampling: None,
            companion: CompanionFeatures::default(),
            is_hidden: false,
            forked_from: None,
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

    /// The companion features start off, and a profile that never switched one on
    /// is written without the group — byte-for-byte what an older build writes, so
    /// a downgrade reads it untouched. A switched-on feature survives the round trip.
    #[test]
    fn companion_features_default_off_and_omitted() {
        let p = Profile::new("X", "s");
        assert!(!p.companion.time_rows);
        assert!(!p.companion.notes_in_prompt);
        assert!(!p.companion.earlier_chats);
        assert!(!p.companion.followups);
        // Quiet hours and the day's cap are on from the start (§7, rule 3).
        assert_eq!((p.companion.quiet_from, p.companion.quiet_until), (22, 8));
        assert_eq!(p.companion.followups_per_day, 3);
        let json = serde_json::to_value(&p).unwrap();
        assert!(json.get("companion").is_none(), "{json}");

        let mut on = p.clone();
        on.companion.time_rows = true;
        let json = serde_json::to_string(&on).unwrap();
        assert!(json.contains("\"time_rows\":true"), "{json}");
        let back: Profile = serde_json::from_str(&json).unwrap();
        assert!(back.companion.time_rows);

        // Each feature is a switch of its own: one on is enough to write the group,
        // and it comes back alone.
        for flip in [
            (|c: &mut CompanionFeatures| c.notes_in_prompt = true) as fn(&mut CompanionFeatures),
            |c| c.earlier_chats = true,
            |c| c.followups = true,
            |c| c.notify = true,
            |c| c.quiet_from = 23,
        ] {
            let mut on = p.clone();
            flip(&mut on.companion);
            let back: Profile = serde_json::from_str(&serde_json::to_string(&on).unwrap()).unwrap();
            assert_eq!(back.companion, on.companion);
            assert!(!back.companion.time_rows);
        }
    }

    /// An older `profiles.json` (no group) reads with every feature off, and a
    /// group naming a feature this build does not know still reads.
    #[test]
    fn companion_features_tolerate_absence_and_unknown_keys() {
        let raw = format!(
            r#"{{"id":"{}","name":"X","default_system_message":"s"}}"#,
            Uuid::nil()
        );
        let p: Profile = serde_json::from_str(&raw).unwrap();
        assert_eq!(p.companion, CompanionFeatures::default());
        let raw = format!(
            r#"{{"id":"{}","name":"X","default_system_message":"s","companion":{{"time_rows":true,"later_feature":true}}}}"#,
            Uuid::nil()
        );
        let p: Profile = serde_json::from_str(&raw).unwrap();
        assert!(p.companion.time_rows);
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

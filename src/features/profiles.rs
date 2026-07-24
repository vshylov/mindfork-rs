//! Operations on AI-interlocutor profiles: creation, editing, name
//! validation (pure logic, no I/O). Storage and cascading soft delete — in
//! `shared/storage`; the profiles UI section — at M8. See spec §10.

use crate::entities::profile::ToolId;
use crate::entities::profile::{CharacterNames, Profile};
use crate::entities::sampling::SamplingConfig;
use crate::features::tools::default_tool_ids;

/// Maximum profile name length (in characters). Anything past it is trimmed.
pub const MAX_NAME_LEN: usize = 80;

/// Normalizes a profile name: collapses whitespace, trims the edges and the length.
/// Returns `None` if empty after normalization (the operation is rejected).
pub fn sanitize_name(input: &str) -> Option<String> {
    let collapsed: String = input
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    let mut name = String::with_capacity(collapsed.len());
    let mut prev_space = false;
    for ch in collapsed.trim().chars() {
        let is_space = ch == ' ';
        if is_space && prev_space {
            continue;
        }
        name.push(ch);
        prev_space = is_space;
    }
    if name.is_empty() {
        return None;
    }
    Some(name.chars().take(MAX_NAME_LEN).collect())
}

/// Creates a new profile with a valid name. `None` if the name is empty. A new profile
/// gets the current default tool set (via [`reconcile_tools`]).
pub fn create(name: &str, system_message: impl Into<String>) -> Option<Profile> {
    let name = sanitize_name(name)?;
    let mut profile = Profile::new(name, system_message);
    reconcile_tools(&mut profile);
    Some(profile)
}

/// Reconciles a profile's tools against the current default set: tools previously
/// unknown to the profile (new in the app) **are enabled** and recorded in the
/// "known" registry (`known_tools`). Tools the user deliberately disabled
/// (already in `known_tools`) are NOT re-enabled. Returns `true` if the profile
/// changed (a save is needed). See spec §9.4.
///
/// For a profile whose `known_tools` is empty (created before this registry existed),
/// "new" means only tools not in `enabled_tools` — i.e. the missing ones (new in the app)
/// are added to the already-enabled set, and the set itself is
/// recorded as known.
pub fn reconcile_tools(profile: &mut Profile) -> bool {
    // The "known" baseline on the first migration run is what's already enabled
    // (the old defaults snapshot). This prevents re-enabling a tool that
    // was previously enabled, while not breaking anything if known_tools is already filled in.
    if profile.known_tools.is_empty() {
        profile.known_tools = profile.enabled_tools.clone();
    }
    let mut changed = false;
    for id in default_tool_ids() {
        if profile.known_tools.iter().any(|t| t == &id) {
            continue; // the profile already knew the tool — respect the user's choice
        }
        profile.known_tools.push(id.clone());
        changed = true;
        if !profile.enabled_tools.iter().any(|t| t == &id) {
            profile.enabled_tools.push(id);
        }
    }
    changed
}

/// A set of profile edits (any field — optional). Applied to an existing
/// profile; doesn't affect already-created chats (they hold their own copies, spec §10).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProfileEdit {
    pub name: Option<String>,
    pub system_message: Option<String>,
    /// The impersonation mode's system message (see [`Profile::impersonation_system_message`]).
    pub impersonation_system_message: Option<String>,
    /// `Some(None)` — clear the greeting; `Some(Some(..))` — set it.
    pub greeting: Option<Option<String>>,
    pub character_names: Option<CharacterNames>,
    /// `Some(None)` — remove the profile's sampling defaults.
    pub default_sampling: Option<Option<SamplingConfig>>,
    pub enabled_tools: Option<Vec<ToolId>>,
    /// Agent-scaffold language (axis A, docs/history/i18n.md). Editing is allowed only while the
    /// profile has no data yet — the **authoritative** gate check is in the orchestrator, before
    /// applying (`handle_update_profile`), and the UI additionally renders the field
    /// as locked.
    pub language: Option<crate::shared::i18n::Lang>,
}

/// Applies edits to a profile. Returns `false` if a name is given but empty
/// (the edits aren't applied — the profile stays as it was).
pub fn apply_edit(profile: &mut Profile, edit: ProfileEdit) -> bool {
    // Validate the name before any mutations (edit atomicity).
    let new_name = match &edit.name {
        Some(raw) => match sanitize_name(raw) {
            Some(n) => Some(n),
            None => return false,
        },
        None => None,
    };
    if let Some(name) = new_name {
        profile.name = name;
    }
    if let Some(system) = edit.system_message {
        profile.default_system_message = system;
    }
    if let Some(system) = edit.impersonation_system_message {
        profile.impersonation_system_message = system;
    }
    if let Some(greeting) = edit.greeting {
        profile.greeting = greeting.filter(|g| !g.is_empty());
    }
    if let Some(names) = edit.character_names {
        profile.character_names = names;
    }
    if let Some(sampling) = edit.default_sampling {
        profile.default_sampling = sampling;
    }
    if let Some(tools) = edit.enabled_tools {
        profile.enabled_tools = tools;
    }
    if let Some(lang) = edit.language {
        profile.language = lang;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_collapses_and_trims() {
        assert_eq!(
            sanitize_name("  Джойс   Кэрол ").as_deref(),
            Some("Джойс Кэрол")
        );
        assert_eq!(sanitize_name("   \t\n "), None);
    }

    #[test]
    fn sanitize_truncates() {
        let long = "я".repeat(MAX_NAME_LEN + 20);
        assert_eq!(sanitize_name(&long).unwrap().chars().count(), MAX_NAME_LEN);
    }

    #[test]
    fn create_rejects_empty_name() {
        assert!(create("   ", "sys").is_none());
        let p = create("  Карлос ", "Ты — Карлос.").unwrap();
        assert_eq!(p.name, "Карлос");
        assert_eq!(p.default_system_message, "Ты — Карлос.");
    }

    #[test]
    fn apply_edit_updates_selected_fields() {
        let mut p = Profile::new("X", "sys");
        let ok = apply_edit(
            &mut p,
            ProfileEdit {
                name: Some("  Новый  ".into()),
                system_message: Some("новое sys".into()),
                greeting: Some(Some("Привет!".into())),
                ..Default::default()
            },
        );
        assert!(ok);
        assert_eq!(p.name, "Новый");
        assert_eq!(p.default_system_message, "новое sys");
        assert_eq!(p.greeting.as_deref(), Some("Привет!"));
    }

    #[test]
    fn apply_edit_empty_greeting_clears_it() {
        let mut p = Profile::new("X", "sys");
        p.greeting = Some("было".into());
        apply_edit(
            &mut p,
            ProfileEdit {
                greeting: Some(Some(String::new())),
                ..Default::default()
            },
        );
        assert_eq!(p.greeting, None);
    }

    #[test]
    fn reconcile_adds_new_tools_to_existing_profile() {
        // A profile with an "old" defaults snapshot (no new tools).
        let mut p = Profile::new("X", "sys");
        p.enabled_tools = vec!["note_save".into(), "web_search".into()];
        // known_tools is empty (created before the registry existed) — migration should add new ones.
        let changed = reconcile_tools(&mut p);
        assert!(changed);
        // New safe tools are enabled.
        assert!(p.enabled_tools.iter().any(|t| t == "calculate"));
        assert!(p.enabled_tools.iter().any(|t| t == "current_time"));
        // Old ones are preserved.
        assert!(p.enabled_tools.iter().any(|t| t == "note_save"));
        // All current defaults are now "known".
        for id in default_tool_ids() {
            assert!(p.known_tools.iter().any(|t| t == &id), "not recorded: {id}");
        }
    }

    #[test]
    fn reconcile_does_not_reenable_user_disabled_tool() {
        // The user deliberately disabled calculate: it's not in enabled, but it is in known.
        let mut p = Profile::new("X", "sys");
        p.known_tools = default_tool_ids();
        p.enabled_tools = default_tool_ids()
            .into_iter()
            .filter(|t| t != "calculate")
            .collect();
        let changed = reconcile_tools(&mut p);
        assert!(
            !changed,
            "nothing new — a known disabled tool is left alone"
        );
        assert!(
            !p.enabled_tools.iter().any(|t| t == "calculate"),
            "a disabled tool shouldn't be re-enabled"
        );
    }

    #[test]
    fn reconcile_is_idempotent() {
        let mut p = Profile::new("X", "sys");
        assert!(reconcile_tools(&mut p)); // the first run enables the defaults
        let after_first = p.clone();
        assert!(!reconcile_tools(&mut p)); // a repeat — no changes
        assert_eq!(p, after_first);
    }

    #[test]
    fn create_gives_default_tools() {
        let p = create("Имя", "sys").unwrap();
        assert!(p.enabled_tools.iter().any(|t| t == "calculate"));
        assert_eq!(p.enabled_tools, default_tool_ids());
    }

    #[test]
    fn apply_edit_rejects_empty_name_without_mutating() {
        let mut p = Profile::new("Имя", "sys");
        let ok = apply_edit(
            &mut p,
            ProfileEdit {
                name: Some("   ".into()),
                system_message: Some("не должно примениться".into()),
                ..Default::default()
            },
        );
        assert!(!ok);
        assert_eq!(p.name, "Имя");
        assert_eq!(p.default_system_message, "sys");
    }
}

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

/// Seed values `CharacterNames::default()` used to carry before role names became
/// user-visible — the Russian set and the English one it was migrated to
/// (`b83ae37`). They were never displayed anywhere, so they're pure leftovers.
const SEED_CHARACTER_NAMES: [(&str, &str, &str); 2] = [
    ("Вы", "Ассистент", "Система"), // the pre-migration seed: data, not prose; cyrillic-ok
    ("You", "Assistant", "System"),
];

/// Clears role names that still hold a legacy seed value (see
/// [`SEED_CHARACTER_NAMES`]), so the feed and the `F5` export fall back to the
/// **interface language's** labels instead of showing a stale English seed. A
/// user-typed name is kept (the field had no UI until now, so the only sources of
/// non-seed values are imports and hand edits). Returns `true` if the profile
/// changed (a save is needed). Idempotent: on a second run there's nothing to clear.
pub fn clear_seed_character_names(profile: &mut Profile) -> bool {
    let names = &mut profile.character_names;
    let mut changed = false;
    for (user, assistant, system) in SEED_CHARACTER_NAMES {
        for (field, seed) in [
            (&mut names.user, user),
            (&mut names.assistant, assistant),
            (&mut names.system, system),
        ] {
            if field == seed {
                field.clear();
                changed = true;
            }
        }
    }
    changed
}

/// Re-seeds the localized defaults after a scaffold-language switch (axis A,
/// spec §10) on a data-free profile: a name still byte-equal to the old
/// locale's `defaults.profile_name` and a system message still byte-equal to
/// the old locale's `defaults.system_message` follow the new language
/// (`profile.language`, already switched by `apply_edit`). User-edited text
/// never matches the old default and is left alone — the rule the i18n plan
/// fixed for this switch (docs/history/i18n.md §4 p.6). Returns `true` if the
/// profile changed.
pub fn reseed_language_defaults(
    profile: &mut Profile,
    old_lang: crate::shared::i18n::Lang,
) -> bool {
    let old = crate::shared::i18n::locale(old_lang);
    let new = crate::shared::i18n::locale(profile.language);
    let mut changed = false;
    if profile.name == old.t("defaults.profile_name") {
        profile.name = new.t("defaults.profile_name").to_string();
        changed = true;
    }
    if profile.default_system_message == old.t("defaults.system_message") {
        profile.default_system_message = new.t("defaults.system_message").to_string();
        changed = true;
    }
    changed
}

/// A set of profile edits (any field — optional). Applied to an existing
/// profile; doesn't affect already-created chats (they hold their own copies, spec §10).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProfileEdit {
    pub name: Option<String>,
    pub system_message: Option<String>,
    /// The impersonation profile this profile's chats use (`Some(None)` — unlink,
    /// falling back to the default text). See [`Profile::impersonation_profile_id`].
    pub impersonation_profile_id: Option<Option<uuid::Uuid>>,
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
    if let Some(imp) = edit.impersonation_profile_id {
        profile.impersonation_profile_id = imp;
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

    /// Legacy seed names (both the Russian set and the English one it was migrated
    /// to) are cleared, so the labels follow the interface language; a name the user
    /// actually chose survives. Idempotent — a second run changes nothing.
    #[test]
    fn clear_seed_character_names_drops_only_seeds() {
        let mut p = Profile::new("X", "sys");
        p.character_names = CharacterNames {
            user: "Вы".into(),
            assistant: "Assistant".into(),
            system: "Система".into(),
        };
        assert!(clear_seed_character_names(&mut p));
        assert_eq!(p.character_names, CharacterNames::default());
        assert!(!clear_seed_character_names(&mut p));

        let mut custom = Profile::new("X", "sys");
        custom.character_names = CharacterNames {
            user: "Гайя".into(),
            assistant: "Анна".into(),
            system: String::new(),
        };
        let before = custom.character_names.clone();
        assert!(!clear_seed_character_names(&mut custom));
        assert_eq!(custom.character_names, before);
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

    /// After a language switch (`profile.language` already holds the new value,
    /// as `apply_edit` leaves it) the untouched localized defaults follow it.
    #[test]
    fn reseed_replaces_untouched_defaults() {
        use crate::shared::i18n::{Lang, locale};
        let ru = locale(Lang::Ru);
        let mut p = Profile::new(
            ru.t("defaults.profile_name"),
            ru.t("defaults.system_message"),
        );
        p.language = Lang::En;
        assert!(reseed_language_defaults(&mut p, Lang::Ru));
        let en = locale(Lang::En);
        assert_eq!(p.name, en.t("defaults.profile_name"));
        assert_eq!(p.default_system_message, en.t("defaults.system_message"));
    }

    /// User-edited text never matches the old locale's default byte-for-byte —
    /// the switch leaves it alone.
    #[test]
    fn reseed_keeps_user_edited_texts() {
        use crate::shared::i18n::Lang;
        let mut p = Profile::new("Гея", "Ты — Гея.");
        p.language = Lang::En;
        assert!(!reseed_language_defaults(&mut p, Lang::Ru));
        assert_eq!(p.name, "Гея");
        assert_eq!(p.default_system_message, "Ты — Гея.");
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

//! Managing profiles: creation, soft deletion with cascade, editing.

use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::features::profiles::ProfileEdit;
use crate::shared::config::ImpersonationProfile;

use super::Orchestrator;

impl Orchestrator {
    /// One-time migration of the legacy `Profile.impersonation_system_message` into a
    /// named impersonation profile (spec §11.8): for every profile that still carries a
    /// non-empty message and no reference, an impersonation profile "«name»
    /// (impersonation)" is created in `config.impersonation_profiles` and linked.
    ///
    /// Idempotent: a profile with a reference is skipped, so a repeat run (or an app
    /// downgrade/upgrade) doesn't duplicate anything. The legacy field itself is kept
    /// on disk — nothing reads it for prompt building any more.
    pub(super) fn migrate_impersonation_profiles(&mut self) {
        let loc = self.ui_locale();
        let mut created = Vec::new();
        let mut linked = Vec::new();
        for profile in &mut self.profiles {
            let legacy = profile.impersonation_system_message.trim();
            if legacy.is_empty() || profile.impersonation_profile_id.is_some() {
                continue;
            }
            let imp = ImpersonationProfile::new(
                loc.tf(
                    "ui.settings.imp_profile_migrated_name",
                    &[("name", &profile.name)],
                ),
                legacy,
            );
            profile.impersonation_profile_id = Some(imp.id);
            created.push(imp);
            linked.push(profile.clone());
        }
        if created.is_empty() {
            return;
        }
        let count = created.len();
        self.config.impersonation_profiles.extend(created);
        // The config first: a reference persisted without its target would dangle.
        // On failure the in-memory state stays consistent and the next launch retries.
        if let Err(err) = self.storage.json().save_config(&self.config) {
            tracing::warn!(error = %err, "failed to save migrated impersonation profiles");
            return;
        }
        for profile in &linked {
            if let Err(err) = self.storage.json().upsert_profile(profile) {
                tracing::warn!(error = %err, profile = %profile.id,
                    "failed to link the migrated impersonation profile");
            }
        }
        tracing::info!(count, "migrated legacy impersonation system messages");
    }

    /// Creates a new profile (validates the name), saves it, and refreshes the list.
    pub(super) fn handle_create_profile(&mut self, name: String, system_message: String) {
        let Some(mut profile) = crate::features::profiles::create(&name, system_message) else {
            let _ = self.evt_tx.send(AppEvent::Error(
                self.ui_locale().t("ui.err.profile_name_empty").into(),
            ));
            return;
        };
        // A new profile is created in the default scaffold language (defaults.json); while
        // it has no data yet, the language can be changed in settings. See docs/history/i18n.md.
        profile.language = self.default_language;
        if let Err(err) = self.storage.json().upsert_profile(&profile) {
            let _ = self.evt_tx.send(AppEvent::Error(
                self.ui_locale()
                    .tf("ui.err.profile_create_failed", &[("err", &err.to_string())]),
            ));
            return;
        }
        self.profiles.push(profile);
        self.emit_profile_list();
        // The settings screen keeps its own copy of the profile list (from the
        // `Settings` snapshot), so without this re-emit a freshly created profile
        // would be invisible there until a restart — impossible to select, let alone
        // edit. The screen also auto-selects the newly appeared profile.
        self.emit_settings();
    }

    /// Soft-deletes a profile with a cascade: its chats are hidden, and notes/RAG
    /// become unreachable (the profile is hidden). See spec §10, §12.3.
    pub(super) fn handle_delete_profile(&mut self, id: Uuid) {
        // Can't delete the last profile — otherwise there's nothing to create chats from.
        if self.profiles.len() <= 1 {
            let _ = self.evt_tx.send(AppEvent::Error(
                self.ui_locale().t("ui.err.profile_delete_last").into(),
            ));
            return;
        }
        match self.storage.hide_profile_cascade(id) {
            Ok(false) => return,
            Err(err) => {
                let _ = self.evt_tx.send(AppEvent::Error(
                    self.ui_locale()
                        .tf("ui.err.profile_delete_failed", &[("err", &err.to_string())]),
                ));
                return;
            }
            Ok(true) => {}
        }
        self.profiles.retain(|p| p.id != id);
        // Remove the deleted profile's chats from memory.
        let removed: Vec<Uuid> = self
            .chats
            .iter()
            .filter(|c| c.profile_id == id)
            .map(|c| c.id)
            .collect();
        self.chats.retain(|c| c.profile_id != id);
        for cid in &removed {
            self.saves.forget(*cid);
        }
        self.emit_profile_list();
        // Same as in `handle_create_profile`: refresh the settings screen's copy,
        // otherwise it would keep showing (and editing) a deleted profile.
        self.emit_settings();

        // If the active chat belonged to the deleted profile — switch away.
        let active_removed = self.active_id.is_some_and(|a| removed.contains(&a));
        if active_removed {
            self.active_id = None;
            if let Some(next) = self.chats.first().map(|c| c.id) {
                self.emit_chat_list();
                self.activate(next);
            } else {
                self.handle_new_chat(None);
            }
        } else {
            self.emit_chat_list();
        }
    }

    /// Applies profile edits (doesn't touch already-created chats — they have their own
    /// copies, spec §10). Saves and re-emits the profile list/settings.
    pub(super) fn handle_update_profile(&mut self, id: Uuid, mut edit: ProfileEdit) {
        // The authoritative gate for changing the scaffold language (axis A, docs/history/i18n.md): if the profile
        // already has data, an actual language change is rejected (a safety net on top
        // of the UI's field lock). The language edit is dropped in that case, the rest are applied.
        if let Some(new_lang) = edit.language {
            let current = self
                .profiles
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.language);
            if current == Some(new_lang) {
                edit.language = None; // the language isn't changing — the gate doesn't apply
            } else if self.profile_has_data(id) {
                edit.language = None;
                let _ = self.evt_tx.send(AppEvent::Error(
                    self.ui_locale().t("ui.err.profile_language_locked").into(),
                ));
            }
        }
        let Some(profile) = self.profiles.iter_mut().find(|p| p.id == id) else {
            return;
        };
        if !crate::features::profiles::apply_edit(profile, edit) {
            let _ = self.evt_tx.send(AppEvent::Error(
                self.ui_locale().t("ui.err.profile_name_empty").into(),
            ));
            return;
        }
        let profile = profile.clone();
        if let Err(err) = self.storage.json().upsert_profile(&profile) {
            let _ = self.evt_tx.send(AppEvent::Error(
                self.ui_locale()
                    .tf("ui.err.profile_save_failed", &[("err", &err.to_string())]),
            ));
            return;
        }
        self.emit_profile_list();
        self.emit_settings();
        // Role names live on the profile and are resolved at render time — refresh
        // the feed's copy so a rename applies to the open chat immediately (spec §11.3).
        self.emit_character_names();
    }
}

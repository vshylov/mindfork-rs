//! Settings screen — key handling and working-copy mutations: the input dispatcher,
//! the field editor, toggles, value cycles, applying text, saving.
//! Part of the [`super`] module; split out of settings.rs.

use super::helpers::*;
use super::spec::{Access, FieldSpec, field_spec};
use super::*;

impl SettingsScreen {
    // ---------- key handling ----------

    /// Handles a keypress, returning an intent for `app` (or `None`).
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<SettingsIntent> {
        if key.kind != KeyEventKind::Press {
            return None;
        }
        // Quit the app (`Ctrl+Q`/`F10`), from anywhere on the settings screen
        // (including a field editor). `Ctrl+Q` is matched by the "physical" Latin key —
        // works under any layout (see shared::keys). Quit moved off `Ctrl+C`
        // (freed up), see docs/history/input-selection-undo-mouse.md §B.
        if key.code == KeyCode::F(10)
            || (key.modifiers.contains(KeyModifiers::CONTROL)
                && keys::hotkey_char(&key) == Some('q'))
        {
            return Some(SettingsIntent::Quit);
        }
        // The search overlay intercepts input (except for quit above).
        if self.search.is_some() {
            return self.handle_search_key(key);
        }
        // The Choice-field picker popup.
        if self.choice.is_some() {
            return self.handle_choice_key(key);
        }
        if self.editor.is_some() {
            return self.handle_editor_key(key);
        }
        // `/` opens field search (inside the editor `/` is a plain character, handled
        // above). Matched by the "physical" `/` key — under a Russian layout the same
        // key sends `.` (see shared::keys::is_slash_key).
        if let KeyCode::Char(c) = key.code
            && keys::is_slash_key(c)
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT)
        {
            self.open_search();
            return None;
        }
        // Create/delete a profile (in the "Profiles" section). Matched by the
        // "physical" Latin key — shortcuts work under any layout (see shared::keys).
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && self.section() == Section::Profiles
            && let Some(physical) = keys::hotkey_char(&key)
        {
            // The subsection decides *which* list is edited: "Assistant" — the
            // AI-interlocutor profiles (owned by the orchestrator → an intent),
            // "Impersonation" — the user personas in `config.impersonation_profiles`
            // (a plain config edit). See spec §11.8.
            let imp = self.profile_sub == Subsection::Impersonation;
            match (physical, imp) {
                ('n', false) => {
                    // The orchestrator owns the profile list: the new profile arrives
                    // with the next `Settings` snapshot, and `refresh` selects it.
                    self.pending_profile_select = true;
                    return Some(SettingsIntent::CreateProfile {
                        name: self.loc().t("ui.settings.new_profile_name").into(),
                        system_message: String::new(),
                    });
                }
                ('d', false) => {
                    return self
                        .profiles
                        .get(self.profile_idx)
                        .map(|p| SettingsIntent::DeleteProfile(p.id));
                }
                ('n', true) => return Some(self.create_impersonation_profile()),
                ('d', true) => return self.delete_impersonation_profile(),
                _ => {}
            }
        }
        match (key.code, key.modifiers) {
            // Esc — one level up. From the field pane it returns to the sections; from
            // the sections it closes the screen. The editor/Choice/search popups
            // handled above already close *into* the pane, so this completes a ladder
            // that was previously only half built. See docs/history/settings-navigation.md §2.
            (KeyCode::Esc, _) => match self.focus {
                Focus::Fields => {
                    self.focus = Focus::Menu;
                    None
                }
                Focus::Menu => Some(SettingsIntent::Close),
            },
            (KeyCode::Tab, _) => {
                self.move_section(1);
                None
            }
            (KeyCode::BackTab, _) => {
                self.move_section(-1);
                None
            }
            _ => match self.focus {
                Focus::Menu => self.handle_menu_key(key),
                Focus::Fields => self.handle_fields_key(key),
            },
        }
    }

    pub(super) fn handle_menu_key(&mut self, key: KeyEvent) -> Option<SettingsIntent> {
        match key.code {
            KeyCode::Up => {
                self.move_section(-1);
            }
            KeyCode::Down => {
                self.move_section(1);
            }
            // Enter is the ONLY way into the field pane. `→` deliberately doesn't
            // enter: while it did, users built the model "`←` leaves the pane" — but
            // `←` has to cycle a Choice value, and the first fields of most sections
            // are Choice (the server mode, the theme), so the return keystroke
            // silently changed a setting. See docs/history/settings-navigation.md §1.2.
            // The guard: a section with no fields would leave the focus pointing at
            // nothing. No current section produces an empty set — this is defensive.
            KeyCode::Enter if !self.fields().is_empty() => {
                self.focus = Focus::Fields;
                self.field_idx = 0;
            }
            _ => {}
        }
        None
    }

    pub(super) fn handle_fields_key(&mut self, key: KeyEvent) -> Option<SettingsIntent> {
        let fields = self.fields();
        match key.code {
            // ←/→ inside the pane mean exactly one thing: change the value (or switch
            // the subsection tab). They never move focus — leaving is `Esc`. The two
            // arms are deliberately symmetric: `←` used to fall through to "back to the
            // menu" for non-Choice fields, which is what made the key context-dependent
            // and cost users an accidental edit. See docs/history/settings-navigation.md R2.
            KeyCode::Left => {
                if let Some(f) = fields.get(self.field_idx)
                    && matches!(f.kind, FieldKind::Choice(_))
                {
                    return self.cycle_field(f.id, -1);
                }
                None
            }
            KeyCode::Right => {
                if let Some(f) = fields.get(self.field_idx)
                    && matches!(f.kind, FieldKind::Choice(_))
                {
                    return self.cycle_field(f.id, 1);
                }
                None
            }
            KeyCode::Up => {
                self.field_idx = self.field_idx.saturating_sub(1);
                None
            }
            KeyCode::Down => {
                if self.field_idx + 1 < fields.len() {
                    self.field_idx += 1;
                }
                None
            }
            KeyCode::Char(' ') => {
                if let Some(f) = fields.get(self.field_idx)
                    && matches!(f.kind, FieldKind::Toggle(_))
                {
                    return self.toggle_field(f.id);
                }
                None
            }
            // Del — reset the field to its default value (config fields; profile fields — no-op).
            KeyCode::Delete => {
                let id = fields.get(self.field_idx)?.id;
                self.reset_field(id)
            }
            KeyCode::Enter => {
                let f = fields.get(self.field_idx)?;
                // The MCP-server row — a read-only status; Enter when "catalog
                // changed" confirms the new catalog (TOFU, spec §9.6).
                if let FieldId::TMcpServer(idx) = f.id {
                    return self.confirm_mcp_catalog(idx);
                }
                match &f.kind {
                    FieldKind::Toggle(_) => self.toggle_field(f.id),
                    // Choice (incl. profile selection PSelect) — an option-list popup.
                    FieldKind::Choice(_) => {
                        self.open_choice(f.id);
                        None
                    }
                    FieldKind::Text(value) => {
                        // The system message and greeting are multiline (wrapping +
                        // line breaks); other fields are single-line (horizontal
                        // scroll, no wrap onto an invisible row). See spec §11.6.
                        let multiline = matches!(
                            f.id,
                            FieldId::PSystem | FieldId::PGreeting | FieldId::IpSystem
                        );
                        let mut input = InputBox::new();
                        input.set_single_line(!multiline);
                        // Secret field: masking (`•`) + an **empty** seed — a stored
                        // key can't be shown (not even the screen has it); editing =
                        // entering it again. See docs/research/api-key-storage.md.
                        if is_api_key_field(f.id) {
                            input.set_mask(true);
                        }
                        // Don't show the "(all)"/"—" placeholders as a value.
                        let seed = self.field_seed(f.id, value);
                        input.set_text(&seed);
                        self.editor = Some(Editor {
                            field: f.id,
                            input,
                            multiline,
                            error: None,
                        });
                        None
                    }
                }
            }
            _ => None,
        }
    }

    pub(super) fn handle_editor_key(&mut self, key: KeyEvent) -> Option<SettingsIntent> {
        let loc = self.loc();
        let editor = self.editor.as_mut()?;
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                self.editor = None;
                None
            }
            // The multiline editor (system message/greeting): `Shift+Enter` (or
            // `Alt+Enter` — a fallback for terminals without the kitty protocol,
            // see item 11) — a line break, Enter — commit (as in chat input, spec §11.7).
            (KeyCode::Enter, m)
                if editor.multiline && m.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) =>
            {
                editor.input.insert_newline();
                None
            }
            (KeyCode::Enter, _) => {
                let text = editor.input.text();
                // Validation without closing: an invalid numeric field leaves the editor
                // open, the title turns red; fixing it or Esc closes it.
                if let Some(err_key) = field_validation_error(editor.field, &text) {
                    editor.error = Some(loc.t(err_key));
                    return None;
                }
                let editor = self.editor.take().unwrap();
                self.apply_text(editor.field, &text)
            }
            // All Ctrl combos on the field (`Ctrl+K` clear with `Ctrl+Z` to restore,
            // word-wise navigation/deletion, undo/redo) are handled by `InputBox` itself
            // in `on_key` (layout-independent); we clear `editor.error` below.
            _ => {
                editor.input.on_key(key);
                editor.error = None; // an edit clears the previous error
                None
            }
        }
    }

    /// Clipboard paste (bracketed paste): meaningful only when a field's text editor
    /// is open (e.g. a model path) — otherwise a no-op. See spec §11.5.
    pub fn handle_paste(&mut self, text: &str) {
        if let Some(search) = self.search.as_mut() {
            search.input.insert_str(text);
            self.search_filter();
        } else if let Some(editor) = self.editor.as_mut() {
            editor.input.insert_str(text);
        }
    }

    /// Switches the section — and nothing else. The focus is **preserved**
    /// (docs/history/settings-navigation.md R4): Tab used to reset it to the menu, which made
    /// the key do two things at once. `field_idx` is still reset, since field sets
    /// differ per section.
    pub(super) fn move_section(&mut self, delta: i32) {
        let n = SECTIONS.len() as i32;
        self.section_idx = (((self.section_idx as i32 + delta) % n + n) % n) as usize;
        self.field_idx = 0;
    }

    // ---------- applying edits ----------

    /// The editor's seed value (no placeholders).
    pub(super) fn field_seed(&self, id: FieldId, shown: &str) -> String {
        match id {
            // Secret field: the row's value is a status ("configured"), not the key;
            // the editor always opens empty (a stored key can't be shown).
            _ if is_api_key_field(id) => String::new(),
            FieldId::IDicts => self.config.interface.selected_dictionaries.join(", "),
            // "—" for empty numbers — the seed is empty.
            _ if shown == "—" => String::new(),
            _ => shown.to_string(),
        }
    }

    /// Flips a boolean toggle and returns the corresponding intent.
    pub(super) fn toggle_field(&mut self, id: FieldId) -> Option<SettingsIntent> {
        // Profile-tool toggles — over `profiles[idx]`, not over `AppConfig`.
        if let FieldId::PTool(idx) = id {
            return self.toggle_profile_tool(idx);
        }
        // Config toggles — via the access table (single source, see spec.rs).
        if let Some(FieldSpec {
            access: Access::Toggle(flip),
            ..
        }) = field_spec(id)
        {
            flip(&mut self.config);
            return Some(self.save_config());
        }
        None
    }

    /// Confirming a changed MCP-server catalog (Enter on its row): the intent goes to
    /// the orchestrator only when the server is actually awaiting confirmation
    /// (`pending_catalog`); otherwise — a no-op (the row is read-only).
    pub(super) fn confirm_mcp_catalog(&self, idx: usize) -> Option<SettingsIntent> {
        let srv = self.mcp.servers.get(idx)?;
        srv.pending_catalog
            .then(|| SettingsIntent::ConfirmMcpCatalog(srv.id.clone()))
    }

    pub(super) fn toggle_profile_tool(&mut self, idx: usize) -> Option<SettingsIntent> {
        let catalog = self.tool_catalog();
        let tool = catalog.get(idx)?.id.clone();
        let p = self.profiles.get_mut(self.profile_idx)?;
        if let Some(pos) = p.enabled_tools.iter().position(|t| t == &tool) {
            p.enabled_tools.remove(pos);
        } else {
            p.enabled_tools.push(tool);
        }
        Some(self.save_profile())
    }

    /// Cyclically changes a Choice field's value.
    pub(super) fn cycle_field(&mut self, id: FieldId, dir: i32) -> Option<SettingsIntent> {
        // Config Choice fields (mode/flash-attn/spec-type/theme) — via the access table.
        if let Some(FieldSpec {
            access: Access::Choice { cycle, .. },
            ..
        }) = field_spec(id)
        {
            cycle(&mut self.config, dir);
            return Some(self.save_config());
        }
        match id {
            // Switching subsections (tab strip) — pure navigation, no save.
            // Model — three tabs, direction-aware; Sampling/Profiles — two.
            FieldId::ModelSub => {
                self.model_sub = self.model_sub.cycle(dir);
                None
            }
            FieldId::SamplingSub => {
                self.sampling_sub = self.sampling_sub.toggled();
                None
            }
            FieldId::ProfileSub => {
                self.profile_sub = self.profile_sub.toggled();
                None
            }
            // Sampling parameters — their own descriptor (`SamplingParam`), not in the table.
            FieldId::S(p) => self.cycle_sampling_field(false, p),
            FieldId::IS(p) => self.cycle_sampling_field(true, p),
            // Profile selection — navigation over `profiles`, no config save.
            FieldId::PSelect => {
                if !self.profiles.is_empty() {
                    let n = self.profiles.len() as i32;
                    self.profile_idx = (((self.profile_idx as i32 + dir) % n + n) % n) as usize;
                }
                None
            }
            // Impersonation-profile selection — navigation, no save.
            FieldId::IpSelect => {
                let n = self.config.impersonation_profiles.len() as i32;
                if n > 0 {
                    self.imp_profile_idx =
                        (((self.imp_profile_idx as i32 + dir) % n + n) % n) as usize;
                }
                None
            }
            // The assistant profile's reference to an impersonation profile: option 0 —
            // "not set" (the shared default text), then the profiles themselves.
            FieldId::PImpProfile => {
                let n = self.config.impersonation_profiles.len() as i32 + 1;
                let cur = self.imp_profile_choice_index()? as i32;
                let next = (((cur + dir) % n + n) % n) as usize;
                let id = (next > 0).then(|| self.config.impersonation_profiles[next - 1].id);
                self.profiles
                    .get_mut(self.profile_idx)?
                    .impersonation_profile_id = id;
                Some(self.save_profile())
            }
            // Profile scaffold language (axis A): cycles over built-in languages; locked
            // if the profile has data (a safety net on top of the orchestrator gate).
            // See docs/history/i18n.md.
            FieldId::PLanguage => {
                let p = self.profiles.get(self.profile_idx)?;
                if self.language_locked.contains(&p.id) {
                    return None; // the language is locked
                }
                let cur = p.language;
                let all = crate::shared::i18n::Lang::all();
                let i = all.iter().position(|l| *l == cur).unwrap_or(0) as i32;
                let n = all.len() as i32;
                let next = all[(((i + dir) % n + n) % n) as usize];
                self.profiles[self.profile_idx].language = next;
                Some(self.save_profile())
            }
            _ => None,
        }
    }

    /// Cyclically changes a Choice sampling parameter (`Thinking`/`Reasoning`) in the
    /// right subsection. For numeric parameters — a no-op (`None`), so ←/→ over a text
    /// field don't cause a redundant save.
    pub(super) fn cycle_sampling_field(
        &mut self,
        imp: bool,
        p: SamplingParam,
    ) -> Option<SettingsIntent> {
        {
            let s = if imp {
                &mut self.config.impersonation_sampling
            } else {
                &mut self.config.default_sampling
            };
            match p {
                SamplingParam::Thinking => s.thinking = cycle_opt_bool(s.thinking),
                SamplingParam::Reasoning => {
                    s.reasoning_effort = cycle_reasoning(s.reasoning_effort)
                }
                SamplingParam::Verbosity => s.verbosity = cycle_verbosity(s.verbosity),
                _ => return None,
            }
        }
        Some(self.save_config())
    }

    /// Applies text from the editor to a field and returns a save intent.
    pub(super) fn apply_text(&mut self, id: FieldId, text: &str) -> Option<SettingsIntent> {
        let trimmed = text.trim();
        // The secret isn't stored in the config working copy — it goes as a separate
        // intent (the orchestrator encrypts it with the machine key). Empty input = delete the key.
        if let Some(provider) = self.api_key_field_provider(id) {
            return Some(SettingsIntent::SetApiKey {
                provider,
                key: trimmed.to_string(),
            });
        }
        match id {
            // Sampling parameters — their own descriptor (`SamplingParam`), not in the table.
            FieldId::S(p) => apply_sampling_text(&mut self.config.default_sampling, p, trimmed),
            FieldId::IS(p) => {
                apply_sampling_text(&mut self.config.impersonation_sampling, p, trimmed)
            }
            // Profile fields — over `profiles[idx]`, not over `AppConfig`.
            FieldId::PName
            | FieldId::PSystem
            | FieldId::PGreeting
            | FieldId::PUserName
            | FieldId::PAssistantName => {
                return self.apply_profile_text(id, trimmed);
            }
            // Impersonation-profile fields — over `config.impersonation_profiles`.
            FieldId::IpName | FieldId::IpSystem => {
                let ip = self
                    .config
                    .impersonation_profiles
                    .get_mut(self.imp_profile_idx)?;
                match id {
                    FieldId::IpName if trimmed.is_empty() => return None, // no empty names
                    FieldId::IpName => ip.name = trimmed.to_string(),
                    _ => ip.system_message = trimmed.to_string(),
                }
            }
            // Config fields — via the access table (the setter itself parses and routes
            // by external/cloud mode; see spec.rs).
            _ => {
                let Some(FieldSpec {
                    access: Access::Text(set),
                    ..
                }) = field_spec(id)
                else {
                    return None;
                };
                set(&mut self.config, trimmed);
            }
        }
        Some(self.save_config())
    }

    pub(super) fn apply_profile_text(&mut self, id: FieldId, text: &str) -> Option<SettingsIntent> {
        let p = self.profiles.get_mut(self.profile_idx)?;
        match id {
            FieldId::PName => {
                if text.is_empty() {
                    return None; // we don't apply an empty name
                }
                p.name = text.to_string();
            }
            FieldId::PSystem => p.default_system_message = text.to_string(),
            FieldId::PGreeting => {
                p.greeting = (!text.is_empty()).then(|| text.to_string());
            }
            // Role names: empty is a legitimate value — "not set", the feed and the
            // export fall back to their localized labels (spec §5.1).
            FieldId::PUserName => p.character_names.user = text.to_string(),
            FieldId::PAssistantName => p.character_names.assistant = text.to_string(),
            _ => return None,
        }
        Some(self.save_profile())
    }

    /// The index of the current [`FieldId::PImpProfile`] option: `0` — "not set"
    /// (including a dangling reference), otherwise the profile's position + 1.
    pub(super) fn imp_profile_choice_index(&self) -> Option<usize> {
        let p = self.profiles.get(self.profile_idx)?;
        Some(
            p.impersonation_profile_id
                .and_then(|id| {
                    self.config
                        .impersonation_profiles
                        .iter()
                        .position(|ip| ip.id == id)
                })
                .map_or(0, |i| i + 1),
        )
    }

    /// `Ctrl+N` in the "Impersonation" subsection: appends a persona to
    /// `config.impersonation_profiles` and selects it. Unlike assistant profiles, the
    /// list lives in the config — the screen edits its own working copy directly, so
    /// no round-trip through the orchestrator is needed to see the new entry.
    pub(super) fn create_impersonation_profile(&mut self) -> SettingsIntent {
        let name = self.loc().t("ui.settings.new_profile_name");
        self.config
            .impersonation_profiles
            .push(crate::shared::config::ImpersonationProfile::new(
                name,
                String::new(),
            ));
        self.imp_profile_idx = self.config.impersonation_profiles.len() - 1;
        self.save_config()
    }

    /// `Ctrl+D` in the "Impersonation" subsection: removes the selected persona.
    /// Assistant profiles referencing it are left alone — a dangling reference reads
    /// as "not set" and falls back to the shared default text (spec §11.8).
    pub(super) fn delete_impersonation_profile(&mut self) -> Option<SettingsIntent> {
        if self.imp_profile_idx >= self.config.impersonation_profiles.len() {
            return None;
        }
        self.config
            .impersonation_profiles
            .remove(self.imp_profile_idx);
        self.imp_profile_idx = self.imp_profile_idx.saturating_sub(1);
        Some(self.save_config())
    }

    /// The intent to save the current working configuration.
    pub(super) fn save_config(&self) -> SettingsIntent {
        SettingsIntent::SaveConfig(Box::new(self.config.clone()))
    }

    /// The intent to save the selected profile (a full snapshot of its fields).
    pub(super) fn save_profile(&self) -> SettingsIntent {
        let p = &self.profiles[self.profile_idx];
        SettingsIntent::SaveProfile {
            id: p.id,
            edit: Box::new(ProfileEdit {
                name: Some(p.name.clone()),
                system_message: Some(p.default_system_message.clone()),
                impersonation_profile_id: Some(p.impersonation_profile_id),
                greeting: Some(p.greeting.clone()),
                character_names: Some(p.character_names.clone()),
                default_sampling: Some(p.default_sampling.clone()),
                enabled_tools: Some(p.enabled_tools.clone()),
                // The scaffold language is always sent; the orchestrator authoritatively
                // vetoes a switch if the profile has data (matching the current
                // value is also a no-op). docs/history/i18n.md.
                language: Some(p.language),
            }),
        }
    }
}

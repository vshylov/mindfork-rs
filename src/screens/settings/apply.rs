//! Settings screen — key handling and working-copy mutations: the input dispatcher,
//! the field editor, toggles, value cycles, applying text, saving.
//! Part of the [`super`] module; split out of settings.rs.

use super::helpers::*;
use super::spec::{Access, FieldSpec, field_spec};
use super::*;

impl SettingsScreen {
    // ---------- key handling ----------

    /// Handles a keypress, returning an intent for `app` (or `None`).
    ///
    /// Wraps the real dispatcher to record an undo step (`Ctrl+Z`, see
    /// docs/history/settings-undo.md). Config and profile mutations happen at ~9
    /// places inside; hooking them all would be shotgun surgery and easy to forget
    /// when a field type is added later, so the single entry point takes the
    /// snapshot instead — and keeps it only if an edit actually came back.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<SettingsIntent> {
        let pending = self.pre_edit_snapshot(&key);
        let intent = self.handle_key_inner(key);
        self.record_edit(pending, &intent);
        intent
    }

    /// A snapshot of both stores, taken before a key that **could** commit an edit.
    /// `None` for every other key — so typing inside a text editor doesn't clone the
    /// config on each keystroke; only the committing `Enter` does.
    fn pre_edit_snapshot(&self, key: &KeyEvent) -> Option<PendingEdit> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // `Ctrl+N`/`Ctrl+D` create/delete an impersonation persona, which lives in the
        // config and is therefore undoable — but with no field to coalesce on.
        let persona_key = ctrl && matches!(keys::hotkey_char(key), Some('n' | 'd'));
        let field_key = !ctrl
            && matches!(
                key.code,
                KeyCode::Enter
                    | KeyCode::Char(' ')
                    | KeyCode::Delete
                    | KeyCode::Left
                    | KeyCode::Right
            );
        if !field_key && !persona_key {
            return None;
        }
        // What the key acts on: an open editor/popup targets its own field, otherwise
        // it's the focused row.
        let field = (!persona_key)
            .then(|| {
                self.editor
                    .as_ref()
                    .map(|e| e.field)
                    .or_else(|| self.choice.as_ref().map(|c| c.field))
                    .or_else(|| self.fields().get(self.field_idx).map(|f| f.id))
            })
            .flatten();
        Some(PendingEdit {
            config: self.config.clone(),
            profiles: self.profiles.clone(),
            field,
        })
    }

    /// Keeps the half of the snapshot the returned intent names, and drops the rest.
    ///
    /// Anything that isn't a value edit — an API key (the screen never holds it),
    /// creating/deleting an assistant profile (owned by the orchestrator; deletion
    /// cascades over the profile's chats), confirming an MCP catalog (a trust
    /// decision) — records nothing. That exclusion falls out of this match rather
    /// than needing a guard of its own. See docs/history/settings-undo.md §2.1.
    fn record_edit(&mut self, pending: Option<PendingEdit>, intent: &Option<SettingsIntent>) {
        let Some(p) = pending else { return };
        let before = match intent {
            Some(SettingsIntent::SaveConfig(_)) => EditValue::Config(Box::new(p.config)),
            Some(SettingsIntent::SaveProfile { id, .. }) => {
                match p.profiles.into_iter().find(|pr| pr.id == *id) {
                    Some(prev) => EditValue::Profile(Box::new(prev)),
                    None => return,
                }
            }
            _ => return,
        };
        self.push_undo(EditStep {
            before,
            field: p.field,
        });
    }

    /// Pushes a step, coalescing a run of edits to the **same** field into one (U2):
    /// cycling `managed → external → openai` undoes to `managed` in a single press.
    /// Coalescing keeps the *oldest* "before" value, i.e. drops the newer step.
    fn push_undo(&mut self, step: EditStep) {
        self.redo.clear(); // a fresh edit invalidates the redo branch
        if let Some(last) = self.undo.last()
            && step.field.is_some()
            && last.field == step.field
        {
            return;
        }
        if self.undo.len() >= UNDO_CAP {
            self.undo.remove(0);
        }
        self.undo.push(step);
    }

    /// `Ctrl+Z`: restores the previous value and re-emits the same intent the edit
    /// produced. Returns `None` on an empty stack (a plain no-op).
    pub(super) fn undo_edit(&mut self) -> Option<SettingsIntent> {
        let step = self.undo.pop()?;
        let before = self.build_search_index();
        let (intent, mirror) = self.apply_step(step)?;
        self.redo.push(mirror);
        self.jump_to_changed(&before);
        Some(intent)
    }

    /// `Ctrl+Y`: the mirror of [`Self::undo_edit`].
    pub(super) fn redo_edit(&mut self) -> Option<SettingsIntent> {
        let step = self.redo.pop()?;
        let before = self.build_search_index();
        let (intent, mirror) = self.apply_step(step)?;
        self.undo.push(mirror);
        self.jump_to_changed(&before);
        Some(intent)
    }

    /// Restores a step into the working copy and returns (the intent to persist it,
    /// the mirror step for the opposite stack). `None` if the step's profile is gone
    /// — the step is then dropped rather than resurrecting a deleted profile.
    fn apply_step(&mut self, step: EditStep) -> Option<(SettingsIntent, EditStep)> {
        match step.before {
            EditValue::Config(prev) => {
                let mirror = EditStep {
                    before: EditValue::Config(Box::new(self.config.clone())),
                    field: step.field,
                };
                self.config = *prev;
                Some((self.save_config(), mirror))
            }
            EditValue::Profile(prev) => {
                let idx = self.profiles.iter().position(|p| p.id == prev.id)?;
                let mirror = EditStep {
                    before: EditValue::Profile(Box::new(self.profiles[idx].clone())),
                    field: step.field,
                };
                self.profiles[idx] = *prev;
                // Make the restored profile the selected one, so the jump below lands
                // on a row that shows what was actually restored.
                self.profile_idx = idx;
                Some((self.save_profile(), mirror))
            }
        }
    }

    /// Moves the cursor onto the field the undo/redo changed (U4) — otherwise
    /// reverting something from another section happens invisibly.
    ///
    /// Compares the field-search index (which already enumerates every field of every
    /// section **with its rendered value**) before and after the restore, and takes
    /// the first field present in **both** whose value differs. Fields that only
    /// appear or disappear are skipped: they are the *consequence* of a mode change,
    /// not the change itself, which leaves the mode field as the match.
    fn jump_to_changed(&mut self, before: &[SearchHit]) {
        let target = self
            .build_search_index()
            .into_iter()
            .find(|after| {
                before
                    .iter()
                    .any(|b| b.id == after.id && b.value != after.value)
            })
            .map(|h| (h.section_idx, h.subsection, h.field_idx));
        if let Some((section_idx, subsection, field_idx)) = target {
            self.jump_to(section_idx, subsection, field_idx);
        }
    }

    fn handle_key_inner(&mut self, key: KeyEvent) -> Option<SettingsIntent> {
        if key.kind != KeyEventKind::Press {
            return None;
        }
        if is_quit_key(&key) {
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
        // The model picker — a filter line and a list, so it takes every key it
        // is given, exactly as the search overlay above does.
        if self.picker.is_some() {
            return self.handle_picker_key(key);
        }
        if self.editor.is_some() {
            return self.handle_editor_key(key);
        }
        // Undo/redo of a *setting* — deliberately below the editor/search/choice
        // branches above: while one of those is open the same two keys belong to its
        // `InputBox` (text undo), which is existing behaviour and must not change.
        // Matched by the "physical" Latin key, so they work under any layout.
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match keys::hotkey_char(&key) {
                Some('z') => return self.undo_edit(),
                Some('y') => return self.redo_edit(),
                _ => {}
            }
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
        if let Some(done) = self.plugins_list_key(&key) {
            return done;
        }
        if let Some(done) = self.profiles_list_key(&key) {
            return done;
        }
        self.navigation_key(key)
    }

    /// `Ctrl+N`/`Ctrl+D` in the "Plugins" section: create/delete an MCP server.
    /// `Some(..)` — the key was consumed; `None` lets the dispatcher continue.
    fn plugins_list_key(&mut self, key: &KeyEvent) -> Option<Option<SettingsIntent>> {
        // Create/delete an MCP server (in the "Plugins" section) — the same two
        // keys as the profile lists below, over `config.mcp.servers`.
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && self.section() == Section::Plugins
            && let Some(physical) = keys::hotkey_char(key)
        {
            match physical {
                'n' => return Some(Some(self.create_mcp_server())),
                'd' => return Some(self.delete_mcp_server()),
                _ => {}
            }
        }
        None
    }

    /// `Ctrl+N`/`Ctrl+D` in the "Profiles" section: create/delete a profile or
    /// persona. `Some(..)` — the key was consumed; `None` lets the dispatcher continue.
    fn profiles_list_key(&mut self, key: &KeyEvent) -> Option<Option<SettingsIntent>> {
        // Create/delete a profile (in the "Profiles" section). Matched by the
        // "physical" Latin key — shortcuts work under any layout (see shared::keys).
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && self.section() == Section::Profiles
            && let Some(physical) = keys::hotkey_char(key)
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
                    return Some(Some(SettingsIntent::CreateProfile {
                        name: self.loc().t("ui.settings.new_profile_name").into(),
                        system_message: String::new(),
                    }));
                }
                ('d', false) => {
                    return Some(
                        self.profiles
                            .get(self.profile_idx)
                            .map(|p| SettingsIntent::DeleteProfile(p.id)),
                    );
                }
                ('n', true) => return Some(Some(self.create_impersonation_profile())),
                ('d', true) => return Some(self.delete_impersonation_profile()),
                _ => {}
            }
        }
        None
    }

    /// `Esc`/`Tab` navigation and the focus-routed key fallback (the dispatcher's tail).
    fn navigation_key(&mut self, key: KeyEvent) -> Option<SettingsIntent> {
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
                self.field_enter(f)
            }
            _ => None,
        }
    }

    /// `Enter` on a field row: the row-specific action (the MCP status row), a toggle
    /// flip, the Choice popup, or opening the text editor.
    fn field_enter(&mut self, f: &FieldRow) -> Option<SettingsIntent> {
        // The MCP-server row — a read-only status; Enter does what the
        // row needs: confirm a changed catalog (TOFU) or reconnect.
        if let FieldId::TMcpServer(idx) = f.id {
            return self.mcp_server_action(idx);
        }
        // A variable that names a source: the row only reports whether
        // that OS variable is there — there is nothing here to edit (the
        // source itself is edited in the "Variables" row above).
        if matches!(f.id, FieldId::McpEnvSource(_)) {
            return None;
        }
        match &f.kind {
            FieldKind::Toggle(_) => self.toggle_field(f.id),
            // Choice (incl. profile selection PSelect) — an option-list popup.
            FieldKind::Choice(_) => {
                self.open_choice(f.id);
                None
            }
            FieldKind::Text(value) => {
                // A model row offers the provider's catalogue first (fork F1(a)
                // of docs/research/model-picker.md); it falls through to the
                // editor below when there is no catalogue to offer — a slot
                // whose provider already refused, and every other text field.
                if crate::screens::settings::picker::model_slot(f.id).is_some()
                    && !self.catalogue_refused(f.id)
                {
                    return self.open_model_picker(f.id);
                }
                self.open_text_editor(f.id, value);
                None
            }
        }
    }

    /// Opens the text editor on a field — what `Enter` has always done, now also
    /// reachable from the model picker's "type a name by hand" row.
    pub(super) fn open_text_editor(&mut self, id: FieldId, value: &str) {
        // The system message and greeting are multiline (wrapping + line
        // breaks); other fields are single-line (horizontal scroll, no wrap
        // onto an invisible row). See spec §11.6.
        let multiline = matches!(
            id,
            FieldId::PSystem | FieldId::PGreeting | FieldId::IpSystem
        );
        let mut input = InputBox::new();
        input.set_single_line(!multiline);
        // Secret field: masking (`•`) + an **empty** seed — a stored key can't
        // be shown (not even the screen has it); editing = entering it again.
        // See docs/research/api-key-storage.md.
        if is_secret_field(id) {
            input.set_mask(true);
        }
        // Don't show the "(all)"/"—" placeholders as a value.
        let seed = self.field_seed(id, value);
        input.set_text(&seed);
        self.editor = Some(Editor {
            field: id,
            input,
            multiline,
            error: None,
        });
    }

    pub(super) fn handle_editor_key(&mut self, key: KeyEvent) -> Option<SettingsIntent> {
        let loc = self.loc();
        // The commit is handled before taking the `&mut` borrow: some rules need
        // the rest of the configuration to judge (an MCP server id has to be
        // unique among the others), and a validator holding the editor borrow
        // could not look at it.
        let ed = self.editor.as_ref()?;
        let multiline_break = ed.multiline
            && key
                .modifiers
                .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT);
        if key.code == KeyCode::Enter && !multiline_break {
            let (field, text) = (ed.field, ed.input.text());
            // Validation without closing: an invalid value leaves the editor
            // open, the title turns red; fixing it or Esc closes it.
            let error = self
                .mcp_field_error(field, &text)
                .or_else(|| field_validation_error(field, &text))
                .map(|key| loc.t(key).to_string())
                .or_else(|| extra_args_error(field, &text, loc));
            if let Some(error) = error {
                self.editor.as_mut()?.error = Some(error);
                return None;
            }
            self.editor = None;
            return self.apply_text(field, &text);
        }
        let editor = self.editor.as_mut()?;
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                self.editor = None;
                None
            }
            // The multiline editor (system message/greeting): `Shift+Enter` (or
            // `Alt+Enter` — a fallback for terminals without the kitty protocol,
            // see item 11) — a line break, Enter — commit (as in chat input, spec §11.7).
            (KeyCode::Enter, _) => {
                editor.input.insert_newline();
                None
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
            _ if is_secret_field(id) => String::new(),
            FieldId::IDicts => self.config.interface.selected_dictionaries.join(", "),
            // The import row shows the last outcome; what it edits is a path.
            FieldId::McpImport => String::new(),
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
        // The selected MCP server's switch — indexed, so not in the access table.
        if id == FieldId::McpEnabled {
            let srv = self.config.mcp.servers.get_mut(self.mcp_server_idx)?;
            srv.enabled = !srv.enabled;
            return Some(self.save_config());
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

    /// Enter on an MCP server's status row: it does whatever that row needs —
    /// confirms a changed catalog when one is pending (TOFU, spec §9.6),
    /// otherwise reconnects. Reconnect is the only way back for a server that
    /// exhausted its restart budget: since `McpManager::is_current` landed, an
    /// identical config is not re-applied, so "wiggle a setting" no longer
    /// restarts anything (docs/history/mcp-server-editor.md F7).
    pub(super) fn mcp_server_action(&self, idx: usize) -> Option<SettingsIntent> {
        let srv = self.mcp.servers.get(idx)?;
        Some(if srv.pending_catalog {
            SettingsIntent::ConfirmMcpCatalog(srv.id.clone())
        } else {
            SettingsIntent::ReconnectMcpServer(srv.id.clone())
        })
    }

    /// `Ctrl+N` in the "Plugins" section: appends a server and selects it. It is
    /// created **disabled** with a unique generated id and no command — nothing
    /// is spawned while the command is still half-typed, and flipping "Enabled"
    /// becomes the deliberate "start it" moment that a per-field debounce cannot
    /// express (docs/history/mcp-server-editor.md F5).
    pub(super) fn create_mcp_server(&mut self) -> SettingsIntent {
        let id = (1..)
            .map(|n| format!("server-{n}"))
            .find(|id| !self.config.mcp.servers.iter().any(|s| &s.id == id))
            .expect("an unused server id always exists");
        self.config.mcp.servers.push(McpServerConfig {
            id,
            enabled: false,
            ..Default::default()
        });
        self.mcp_server_idx = self.config.mcp.servers.len() - 1;
        self.save_config()
    }

    /// `Ctrl+D` in the "Plugins" section: removes the selected server. Its
    /// per-profile tool toggles are left alone — they name tools that no longer
    /// exist and are simply not offered to the model (the dangling-reference
    /// rule the persona list already uses).
    pub(super) fn delete_mcp_server(&mut self) -> Option<SettingsIntent> {
        if self.mcp_server_idx >= self.config.mcp.servers.len() {
            return None;
        }
        self.config.mcp.servers.remove(self.mcp_server_idx);
        self.mcp_server_idx = self.mcp_server_idx.saturating_sub(1);
        Some(self.save_config())
    }

    /// Validation of an MCP server field that needs the rest of the inventory to
    /// judge — returns an i18n key for the editor's inline error. The host would
    /// report an invalid id or a batch command in the status row too, but an
    /// invalid **id** creates no slot at all, so without this the server would
    /// simply vanish from the status list (docs/history/mcp-server-editor.md F8).
    pub(super) fn mcp_field_error(&self, id: FieldId, text: &str) -> Option<&'static str> {
        let t = text.trim();
        match id {
            FieldId::McpId => {
                if !valid_mcp_server_id(t) {
                    return Some("ui.settings.err.mcp_id");
                }
                let taken = self
                    .config
                    .mcp
                    .servers
                    .iter()
                    .enumerate()
                    .any(|(i, s)| i != self.mcp_server_idx && s.id == t);
                taken.then_some("ui.settings.err.mcp_id_taken")
            }
            _ => None,
        }
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
            // MCP-server selection — navigation, no save.
            FieldId::McpSelect => {
                let n = self.config.mcp.servers.len() as i32;
                if n > 0 {
                    self.mcp_server_idx =
                        (((self.mcp_server_idx as i32 + dir) % n + n) % n) as usize;
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
        // intent (the orchestrator encrypts it with the machine key). Empty input =
        // delete it.
        if let Some(key) = self.secret_field_key(id) {
            return Some(SettingsIntent::SetSecret {
                key,
                value: trimmed.to_string(),
            });
        }
        // The import is read and parsed by the orchestrator: the file carries
        // literal secrets, which must not travel through `screens` (§9 S6).
        if id == FieldId::McpImport {
            return (!trimmed.is_empty())
                .then(|| SettingsIntent::ImportMcpServers(trimmed.to_string()));
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
            // The selected MCP server's fields — over `config.mcp.servers`.
            FieldId::McpId
            | FieldId::McpCommand
            | FieldId::McpArgs
            | FieldId::McpEnv
            | FieldId::McpTimeout
            | FieldId::McpMaxResult => {
                let srv = self.config.mcp.servers.get_mut(self.mcp_server_idx)?;
                match id {
                    // Shape and uniqueness are checked before the commit
                    // (`mcp_field_error`); an empty id would make the server
                    // invisible to the host, so it is refused there.
                    FieldId::McpId => srv.id = trimmed.to_string(),
                    FieldId::McpCommand => srv.command = trimmed.to_string(),
                    FieldId::McpArgs => srv.args = parse_args(trimmed),
                    FieldId::McpEnv => srv.env = parse_env_map(trimmed),
                    // A zero timeout would mean "give up at once"; the host
                    // clamps it to 1s anyway, so keep the previous value.
                    FieldId::McpTimeout => match trimmed.parse::<u64>() {
                        Ok(v) if v > 0 => srv.tool_timeout_secs = v,
                        _ => return None,
                    },
                    _ => match trimmed.parse::<usize>() {
                        Ok(v) if v > 0 => srv.max_result_chars = v,
                        _ => return None,
                    },
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

/// Quit the app (`Ctrl+Q`/`F10`), from anywhere on the settings screen
/// (including a field editor). `Ctrl+Q` is matched by the "physical" Latin key —
/// works under any layout (see shared::keys). Quit moved off `Ctrl+C`
/// (freed up), see docs/history/input-selection-undo-mouse.md §B.
fn is_quit_key(key: &KeyEvent) -> bool {
    key.code == KeyCode::F(10)
        || (key.modifiers.contains(KeyModifiers::CONTROL) && keys::hotkey_char(key) == Some('q'))
}

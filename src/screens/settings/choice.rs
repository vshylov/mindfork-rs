//! Settings screen — the Choice-field picker popup and resetting a field to its default.
//! Part of the [`super`] module; split out of settings.rs.

use super::helpers::*;
use super::spec::{Access, FieldSpec, field_spec};
use super::*;

impl SettingsScreen {
    // ---------- Choice-field picker popup / reset to default ----------

    /// The Choice field's option list + the index of the current one (`None` — not a
    /// Choice field).
    pub(super) fn choice_menu(&self, id: FieldId) -> Option<(Vec<String>, usize)> {
        // Config Choice fields (mode/flash-attn/spec-type/theme) — from the access table.
        if let Some(FieldSpec {
            access: Access::Choice { options, .. },
            ..
        }) = field_spec(id)
        {
            return Some(options(&self.config, self.loc()));
        }
        match id {
            // Sampling (Thinking/Reasoning/Verbosity) and profile selection — their own sources.
            FieldId::S(
                p @ (SamplingParam::Thinking | SamplingParam::Reasoning | SamplingParam::Verbosity),
            ) => Some(sampling_choice_menu(
                &self.config.default_sampling,
                p,
                self.loc(),
            )),
            FieldId::IS(
                p @ (SamplingParam::Thinking | SamplingParam::Reasoning | SamplingParam::Verbosity),
            ) => Some(sampling_choice_menu(
                &self.config.impersonation_sampling,
                p,
                self.loc(),
            )),
            FieldId::PSelect => {
                let opts: Vec<String> = self.profiles.iter().map(|p| p.name.clone()).collect();
                (!opts.is_empty()).then_some((opts, self.profile_idx))
            }
            // Profile scaffold language (axis A): options — all known languages (built-in +
            // external from data/locales/).
            FieldId::PLanguage => {
                let p = self.profiles.get(self.profile_idx)?;
                let all = crate::shared::i18n::Lang::all();
                let opts: Vec<String> = all.iter().map(|l| l.label().to_string()).collect();
                let cur = all.iter().position(|l| *l == p.language).unwrap_or(0);
                Some((opts, cur))
            }
            _ => None,
        }
    }

    pub(super) fn open_choice(&mut self, id: FieldId) {
        if let Some((options, selected)) = self.choice_menu(id)
            && !options.is_empty()
        {
            self.choice = Some(ChoiceState {
                field: id,
                options,
                selected,
            });
        }
    }

    /// Applies picking an option by index via the existing cycle (`cycle_field`):
    /// steps forward as many times as needed from the current one to the target.
    pub(super) fn apply_choice(&mut self, id: FieldId, target: usize) -> Option<SettingsIntent> {
        let (opts, cur) = self.choice_menu(id)?;
        let n = opts.len();
        if n == 0 {
            return None;
        }
        let steps = (target + n - cur) % n;
        let mut intent = None;
        for _ in 0..steps {
            if let Some(i) = self.cycle_field(id, 1) {
                intent = Some(i);
            }
        }
        intent
    }

    pub(super) fn handle_choice_key(&mut self, key: KeyEvent) -> Option<SettingsIntent> {
        let st = self.choice.as_mut()?;
        match key.code {
            KeyCode::Esc => {
                self.choice = None;
                None
            }
            KeyCode::Up | KeyCode::Left => {
                st.selected = st.selected.saturating_sub(1);
                None
            }
            KeyCode::Down | KeyCode::Right => {
                if st.selected + 1 < st.options.len() {
                    st.selected += 1;
                }
                None
            }
            KeyCode::Enter => {
                let (id, target) = (st.field, st.selected);
                self.choice = None;
                self.apply_choice(id, target)
            }
            _ => None,
        }
    }

    /// Fields of the current section/subsection built from the **default** config
    /// (for the "modified" marker and reset). Profiles are the same (they have no
    /// config default).
    pub(super) fn default_fields(&self) -> Vec<FieldRow> {
        let mut tmp = SettingsScreen::new(
            AppConfig::default(),
            self.profiles.clone(),
            self.language_locked.clone(),
        );
        // Copy the MCP snapshot: otherwise `PTool` rows for MCP tools wouldn't find
        // a match in the default field set.
        tmp.mcp = self.mcp.clone();
        tmp.section_idx = self.section_idx;
        tmp.model_sub = self.model_sub;
        tmp.sampling_sub = self.sampling_sub;
        tmp.profile_sub = self.profile_sub;
        tmp.profile_idx = self.profile_idx;
        tmp.fields()
    }

    /// Resets a config field to its default value. Profile fields and values already
    /// at the default — a no-op (no redundant save).
    pub(super) fn reset_field(&mut self, id: FieldId) -> Option<SettingsIntent> {
        if is_profile_field(id) {
            return None;
        }
        // Secret field: "reset" = delete the stored key (an empty value). The comparison
        // against the default below doesn't apply — the row's value is a status, not the value.
        if let Some(provider) = self.api_key_field_provider(id) {
            return self
                .api_key_present(Some(provider))
                .then(|| SettingsIntent::SetApiKey {
                    provider,
                    key: String::new(),
                });
        }
        let cur_kind = self
            .fields()
            .into_iter()
            .find(|f| f.id == id)
            .map(|f| f.kind)?;
        let default_kind = self
            .default_fields()
            .into_iter()
            .find(|d| d.id == id)
            .map(|d| d.kind)?;
        // Already matches the default — do nothing.
        if value_text(&cur_kind, self.loc()) == value_text(&default_kind, self.loc()) {
            return None;
        }
        match default_kind {
            FieldKind::Toggle(_) => self.toggle_field(id),
            FieldKind::Choice(def_label) => {
                let (opts, _) = self.choice_menu(id)?;
                let idx = opts.iter().position(|o| *o == def_label)?;
                self.apply_choice(id, idx)
            }
            FieldKind::Text(def) => {
                // "—" is the empty placeholder (`Option::None`); clear the field.
                let text = if def == "—" { "" } else { &def };
                self.apply_text(id, text)
            }
        }
    }
}

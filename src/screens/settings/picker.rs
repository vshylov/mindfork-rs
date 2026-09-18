//! Settings screen — the model picker: the provider's own catalogue behind the
//! model row ([docs/research/model-picker.md](../../../docs/research/model-picker.md),
//! stage 4b). Part of the [`super`] module.
//!
//! Deliberately **not** the Choice popup next door. That one reaches an option
//! by cycling to it, one config write per step, which is the wrong shape for a
//! list of 132; and it has no filter, which is the wrong shape for a list of 132
//! for the other reason. What it shares is [`ListScroll`], the one implementation
//! of a list's scroll state (`tools/list_scroll_check.py`).

use crate::shared::api::catalogue::{CatalogModel, CatalogueAnswer, CatalogueError, ModelSlot};

use super::*;

/// What the picker knows about the catalogue right now.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum PickerStatus {
    /// The request is out (fork F4: it goes out when the picker opens, never
    /// before).
    Fetching,
    /// The provider answered. An **empty** list is an answer too: Anthropic
    /// publishes no embedding model, and a router with nothing loaded lists
    /// nothing.
    Listed,
    /// There is no list, and this is why. The row keeps the text field it has
    /// always been — "type a name by hand" is one key away, as always (N5).
    Failed(CatalogueError),
}

/// The open picker.
pub(super) struct PickerState {
    /// The row that receives the pick.
    pub(super) field: FieldId,
    /// Which catalogue was asked for, and what it was narrowed to.
    pub(super) slot: ModelSlot,
    /// What that slot pointed at when the picker opened
    /// ([`SettingsScreen::slot_source`]): a late answer about another provider
    /// must not land here.
    pub(super) source: String,
    /// The filter line — a 132-entry list needs one.
    pub(super) input: InputBox,
    /// Everything the answer carried, in the order the provider published (or
    /// newest first, where it publishes a date — `catalogue::parse`).
    pub(super) all: Vec<CatalogModel>,
    /// Indices into [`Self::all`] that match the filter.
    pub(super) results: Vec<usize>,
    /// The highlighted row of the rendered list, where **0 is always "type a
    /// name by hand"** and the catalogue starts at 1.
    pub(super) selected: usize,
    pub(super) scroll: ListScroll,
    pub(super) status: PickerStatus,
}

impl PickerState {
    /// The entry the highlight is on: `None` — the "by hand" row.
    fn picked(&self) -> Option<&CatalogModel> {
        let idx = self.selected.checked_sub(1)?;
        self.all.get(*self.results.get(idx)?)
    }

    /// How many rows the list draws, the "by hand" row included.
    pub(super) fn rows(&self) -> usize {
        self.results.len() + 1
    }
}

/// Which catalogue a model row is filled from (`None` — not a row a catalogue
/// can fill: a managed GGUF path, or anything else).
pub(super) fn model_slot(id: FieldId) -> Option<ModelSlot> {
    match id {
        FieldId::XModelName => Some(ModelSlot::Assistant),
        FieldId::IxModelName => Some(ModelSlot::Impersonation),
        FieldId::EModelName => Some(ModelSlot::Embedder),
        _ => None,
    }
}

impl SettingsScreen {
    /// `Enter` on a model row (fork F1(a)): the catalogue when there is one to
    /// show, and today's text editor when there is not.
    ///
    /// Returns `true` when the picker took the key, `false` to let the caller
    /// open the editor as it always did.
    pub(super) fn open_model_picker(&mut self, id: FieldId) -> Option<SettingsIntent> {
        let slot = model_slot(id)?;
        let source = self.slot_source(slot);
        let mut input = InputBox::new();
        input.set_single_line(true);
        let cached = self
            .catalogues
            .iter()
            .find(|(s, src, _)| *s == slot && *src == source);
        let (all, status, intent) = match cached {
            Some((_, _, Ok(models))) => (models.to_vec(), PickerStatus::Listed, None),
            Some((_, _, Err(err))) => (Vec::new(), PickerStatus::Failed(*err), None),
            // Nothing asked for *this* provider yet — and the asking happens
            // here, on the keypress, never when the screen opens (fork F4).
            None => {
                self.asked.retain(|(s, _)| *s != slot);
                self.asked.push((slot, source.clone()));
                (
                    Vec::new(),
                    PickerStatus::Fetching,
                    Some(SettingsIntent::ListModels(slot)),
                )
            }
        };
        let results = (0..all.len()).collect();
        self.picker = Some(PickerState {
            field: id,
            slot,
            source,
            input,
            all,
            results,
            // On the first catalogue row when there is one: the list is the
            // reason the picker opened.
            selected: 1,
            scroll: ListScroll::default(),
            status,
        });
        self.picker_clamp();
        intent
    }

    /// What a slot points at right now — its mode, the address and where the key
    /// comes from, as one string.
    ///
    /// This is the cache key, and the reason for it: a slot's **provider changes
    /// inside one visit** to this screen. Keyed by the slot alone, the first
    /// answer was shown under every mode cycled to afterwards — OpenAI's 132
    /// models offered for `gemini`, `claude` and `grok` (reported from a live run,
    /// 2026-09-18). Where the key comes from is in here too, so correcting a
    /// mistyped variable name asks again instead of repeating "no key".
    pub(super) fn slot_source(&self, slot: ModelSlot) -> String {
        let cfg = &self.config;
        let (mode, external, cloud, secret) = match slot {
            ModelSlot::Assistant => (
                format!("{:?}", cfg.engine.mode),
                &cfg.engine.external,
                cfg.engine.cloud(),
                cfg.engine.secret_key(),
            ),
            ModelSlot::Impersonation => (
                format!("{:?}", cfg.impersonation_engine.mode),
                &cfg.impersonation_engine.external,
                cfg.impersonation_engine.cloud(),
                cfg.impersonation_engine.secret_key(),
            ),
            ModelSlot::Embedder => (
                format!("{:?}", cfg.embed.mode),
                &cfg.embed.external,
                cfg.embed.cloud(),
                cfg.embed.secret_key(),
            ),
        };
        let (url, env) = match cloud {
            Some(c) => (c.url.as_deref(), c.api_key_env.as_deref()),
            None => (external.url.as_deref(), external.api_key_env.as_deref()),
        };
        let stored = secret
            .as_ref()
            .is_some_and(|k| self.secrets_present.contains(k));
        format!(
            "{mode}|{}|{}|{stored}",
            url.unwrap_or_default(),
            env.unwrap_or_default()
        )
    }

    /// The answer to [`SettingsIntent::ListModels`], from the orchestrator.
    ///
    /// Kept per slot while the screen is open (N6), so re-opening the picker
    /// costs nothing; `Ctrl+R` inside it is what asks again.
    pub fn set_model_catalogue(&mut self, slot: ModelSlot, models: CatalogueAnswer) {
        // Filed under what was **asked**, not under what the slot points at now:
        // the two differ when the mode was cycled while the answer was in flight,
        // and filing it under the new provider is the defect this key exists to
        // prevent.
        let asked = self
            .asked
            .iter()
            .find(|(s, _)| *s == slot)
            .map(|(_, src)| src.clone())
            .unwrap_or_else(|| self.slot_source(slot));
        self.asked.retain(|(s, _)| *s != slot);
        self.catalogues
            .retain(|(s, src, _)| !(*s == slot && *src == asked));
        self.catalogues.push((slot, asked.clone(), models.clone()));
        let Some(st) = self
            .picker
            .as_mut()
            .filter(|st| st.slot == slot && st.source == asked)
        else {
            return;
        };
        match models {
            Ok(list) => {
                st.all = list.to_vec();
                st.status = PickerStatus::Listed;
            }
            Err(err) => {
                st.all.clear();
                st.status = PickerStatus::Failed(err);
            }
        }
        st.selected = 1;
        self.picker_filter();
    }

    /// Recomputes the matching entries (case-insensitive, every word must
    /// appear — the search overlay's rule).
    pub(super) fn picker_filter(&mut self) {
        let Some(st) = &mut self.picker else { return };
        let q = st.input.text().to_lowercase();
        let terms: Vec<&str> = q.split_whitespace().collect();
        st.results = st
            .all
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                let hay = format!(
                    "{} {}",
                    m.id.to_lowercase(),
                    m.display.as_deref().unwrap_or_default().to_lowercase()
                );
                terms.iter().all(|t| hay.contains(t))
            })
            .map(|(i, _)| i)
            .collect();
        self.picker_clamp();
    }

    /// Keeps the highlight on a row that exists.
    fn picker_clamp(&mut self) {
        if let Some(st) = &mut self.picker {
            let last = st.rows().saturating_sub(1);
            st.selected = st.selected.min(last);
        }
    }

    pub(super) fn handle_picker_key(&mut self, key: KeyEvent) -> Option<SettingsIntent> {
        let st = self.picker.as_mut()?;
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                self.picker = None;
                None
            }
            (KeyCode::Up, _) => {
                st.selected = st.selected.saturating_sub(1);
                None
            }
            (KeyCode::Down, _) => {
                if st.selected + 1 < st.rows() {
                    st.selected += 1;
                }
                None
            }
            (KeyCode::Home, _) => {
                st.selected = 0;
                None
            }
            (KeyCode::End, _) => {
                st.selected = st.rows().saturating_sub(1);
                None
            }
            (KeyCode::Enter, _) => {
                let picked = st.picked().map(|m| m.id.clone());
                let field = st.field;
                self.picker = None;
                match picked {
                    // The catalogue's id, verbatim: on a multi-model endpoint
                    // this string is what selects the model (N3).
                    Some(id) => self.apply_text(field, &id),
                    // "Type a name by hand" — the editor the row has always had.
                    None => {
                        let value = self
                            .fields()
                            .into_iter()
                            .find(|f| f.id == field)
                            .and_then(|f| match f.kind {
                                FieldKind::Text(v) => Some(v),
                                _ => None,
                            })
                            .unwrap_or_default();
                        self.open_text_editor(field, &value);
                        None
                    }
                }
            }
            // Ask the provider again — the catalogue is otherwise kept for the
            // whole visit to the screen (N6).
            (KeyCode::Char(_), KeyModifiers::CONTROL) => {
                match keys::hotkey_char(&key) {
                    Some('r') => {
                        let (slot, source) = (st.slot, st.source.clone());
                        st.all.clear();
                        st.results.clear();
                        st.status = PickerStatus::Fetching;
                        self.catalogues
                            .retain(|(s, src, _)| !(*s == slot && *src == source));
                        self.asked.retain(|(s, _)| *s != slot);
                        self.asked.push((slot, source));
                        Some(SettingsIntent::ListModels(slot))
                    }
                    // Every other Ctrl combination belongs to the filter's own
                    // input (Ctrl+K clears it).
                    _ => {
                        st.input.on_key(key);
                        self.picker_filter();
                        None
                    }
                }
            }
            _ => {
                st.input.on_key(key);
                self.picker_filter();
                None
            }
        }
    }

    /// Whether this row's provider has already been asked and had nothing to
    /// give — in which case `Enter` opens the editor directly rather than a
    /// picker that could only repeat the refusal (fork F1(a)).
    pub(super) fn catalogue_refused(&self, id: FieldId) -> bool {
        let Some(slot) = model_slot(id) else {
            return false;
        };
        let source = self.slot_source(slot);
        self.catalogues
            .iter()
            .any(|(s, src, answer)| *s == slot && *src == source && answer.is_err())
    }

    /// One rendered row of the picker: what the user reads.
    ///
    /// The id first, because the id is what the field takes; then the name the
    /// endpoint published for people, and the day it stops serving the model —
    /// both only when it published them.
    pub(super) fn picker_label(&self, m: &CatalogModel) -> String {
        let mut line = m.id.clone();
        if let Some(display) = m.display.as_deref().filter(|d| *d != m.id) {
            line.push_str(" — ");
            line.push_str(display);
        }
        if let Some(day) = &m.retiring {
            line.push_str(&format!(
                " · {}",
                self.loc()
                    .tf("ui.settings.models.retiring", &[("date", day)])
            ));
        }
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::api::catalogue::ModelRole;

    fn model(id: &str, display: Option<&str>) -> CatalogModel {
        CatalogModel {
            id: id.to_string(),
            display: display.map(str::to_string),
            role: ModelRole::Unstated,
            retiring: None,
        }
    }

    #[test]
    fn a_label_carries_the_id_first_and_the_published_extras_after() {
        let s = SettingsScreen::new(AppConfig::default(), vec![], vec![]);
        assert_eq!(s.picker_label(&model("gpt-5.6-sol", None)), "gpt-5.6-sol");
        assert_eq!(
            s.picker_label(&model("claude-opus-5", Some("Claude Opus 5"))),
            "claude-opus-5 — Claude Opus 5"
        );
        assert_eq!(
            s.picker_label(&CatalogModel {
                retiring: Some("2026-10-23".into()),
                ..model("gpt-4", None)
            }),
            format!(
                "gpt-4 · {}",
                s.loc()
                    .tf("ui.settings.models.retiring", &[("date", "2026-10-23")])
            )
        );
    }
}

//! Settings screen — field search overlay (`/`): index, filter, jump.
//! Part of the [`super`] module; split out of settings.rs.

use super::helpers::*;
use super::*;

impl SettingsScreen {
    // ---------- field search (`/`) ----------

    /// Builds the full index of fields across all sections/subsections for search.
    /// Mode-dependent-visibility fields are taken for the current mode (managed/cloud).
    pub(super) fn build_search_index(&self) -> Vec<SearchHit> {
        let loc = self.loc();
        let model_tabs = model_tab_labels(loc);
        let sub_tabs = sub_tab_labels(loc);
        let mut out = Vec::new();
        for (sec_idx, sec) in SECTIONS.iter().enumerate() {
            match sec {
                Section::Model => {
                    for (si, sub) in ModelTab::ALL.iter().enumerate() {
                        collect_hits(
                            &mut out,
                            sec_idx,
                            *sec,
                            Some(si),
                            Some(model_tabs[si]),
                            self.model_fields_for(*sub),
                            loc,
                        );
                    }
                }
                Section::Sampling => {
                    for (si, sub) in Subsection::ALL.iter().enumerate() {
                        collect_hits(
                            &mut out,
                            sec_idx,
                            *sec,
                            Some(si),
                            Some(sub_tabs[si]),
                            self.sampling_fields_for(*sub),
                            loc,
                        );
                    }
                }
                Section::Profiles => {
                    for (si, sub) in Subsection::ALL.iter().enumerate() {
                        collect_hits(
                            &mut out,
                            sec_idx,
                            *sec,
                            Some(si),
                            Some(sub_tabs[si]),
                            self.profile_fields_for(*sub),
                            loc,
                        );
                    }
                }
                Section::Tools => {
                    collect_hits(&mut out, sec_idx, *sec, None, None, self.tool_fields(), loc)
                }
                Section::Memory => collect_hits(
                    &mut out,
                    sec_idx,
                    *sec,
                    None,
                    None,
                    self.memory_fields(),
                    loc,
                ),
                Section::Data => {
                    collect_hits(&mut out, sec_idx, *sec, None, None, self.data_fields(), loc)
                }
                Section::Interface => collect_hits(
                    &mut out,
                    sec_idx,
                    *sec,
                    None,
                    None,
                    self.interface_fields(),
                    loc,
                ),
            }
        }
        out
    }

    pub(super) fn open_search(&mut self) {
        let mut input = InputBox::new();
        input.set_single_line(true);
        let all = self.build_search_index();
        let results = (0..all.len()).collect();
        self.search = Some(SearchState {
            input,
            all,
            results,
            selected: 0,
        });
    }

    /// Recomputes the results for the query (AND over substring words, case-insensitive).
    pub(super) fn search_filter(&mut self) {
        if let Some(st) = &mut self.search {
            let q = st.input.text().to_lowercase();
            let terms: Vec<&str> = q.split_whitespace().collect();
            st.results = st
                .all
                .iter()
                .enumerate()
                .filter(|(_, h)| terms.iter().all(|t| h.haystack.contains(t)))
                .map(|(i, _)| i)
                .collect();
            if st.selected >= st.results.len() {
                st.selected = st.results.len().saturating_sub(1);
            }
        }
    }

    /// Puts the cursor on a field anywhere on the screen: section, subsection, field
    /// index, focus on the pane. Shared by the search jump and by undo/redo landing on
    /// the field it reverted (docs/history/settings-undo.md U4).
    pub(super) fn jump_to(
        &mut self,
        section_idx: usize,
        subsection: Option<usize>,
        field_idx: usize,
    ) {
        self.section_idx = section_idx;
        if let Some(si) = subsection {
            match SECTIONS[section_idx] {
                Section::Model => self.model_sub = ModelTab::from_index(si),
                Section::Sampling => self.sampling_sub = Subsection::from_index(si),
                Section::Profiles => self.profile_sub = Subsection::from_index(si),
                _ => {}
            }
        }
        self.field_idx = field_idx;
        self.focus = Focus::Fields;
    }

    /// Jumps to the selected result: section, subsection, field, focus on fields.
    pub(super) fn jump_to_selected(&mut self) {
        let target = self.search.as_ref().and_then(|st| {
            st.results.get(st.selected).map(|&ai| {
                let h = &st.all[ai];
                (h.section_idx, h.subsection, h.field_idx)
            })
        });
        if let Some((section_idx, subsection, field_idx)) = target {
            self.jump_to(section_idx, subsection, field_idx);
        }
        self.search = None;
    }

    pub(super) fn handle_search_key(&mut self, key: KeyEvent) -> Option<SettingsIntent> {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => self.search = None,
            (KeyCode::Enter, _) => self.jump_to_selected(),
            (KeyCode::Up, _) => {
                if let Some(s) = &mut self.search {
                    s.selected = s.selected.saturating_sub(1);
                }
            }
            (KeyCode::Down, _) => {
                if let Some(s) = &mut self.search
                    && s.selected + 1 < s.results.len()
                {
                    s.selected += 1;
                }
            }
            // `Ctrl+K` (clear the query, restore via `Ctrl+Z`) and other Ctrl combos on
            // the field are handled by `InputBox` itself in `on_key`; we rerun the filter below.
            _ => {
                if let Some(s) = &mut self.search {
                    s.input.on_key(key);
                }
                self.search_filter();
            }
        }
        None
    }
}

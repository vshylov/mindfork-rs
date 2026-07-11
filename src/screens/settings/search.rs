//! Экран настроек — оверлей поиска по полям (`/`): индекс, фильтр, прыжок.
//! Часть модуля [`super`]; разбито из settings.rs.

use super::helpers::*;
use super::*;

impl SettingsScreen {
    // ---------- поиск по полям (`/`) ----------

    /// Строит полный индекс полей всех секций/подсекций для поиска. Поля
    /// mode-зависимой видимости берутся по текущему режиму (managed/облако).
    pub(super) fn build_search_index(&self) -> Vec<SearchHit> {
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
                            Some(MODEL_TABS[si]),
                            self.model_fields_for(*sub),
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
                            Some(SUB_TABS[si]),
                            self.sampling_fields_for(*sub),
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
                            Some(SUB_TABS[si]),
                            self.profile_fields_for(*sub),
                        );
                    }
                }
                Section::Tools => {
                    collect_hits(&mut out, sec_idx, *sec, None, None, self.tool_fields())
                }
                Section::Memory => {
                    collect_hits(&mut out, sec_idx, *sec, None, None, self.memory_fields())
                }
                Section::Interface => {
                    collect_hits(&mut out, sec_idx, *sec, None, None, self.interface_fields())
                }
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

    /// Пересчитывает выдачу по запросу (AND по словам-подстрокам, регистронезав.).
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

    /// Прыжок к выбранному результату: секция, подсекция, поле, фокус на полях.
    pub(super) fn jump_to_selected(&mut self) {
        let target = self.search.as_ref().and_then(|st| {
            st.results.get(st.selected).map(|&ai| {
                let h = &st.all[ai];
                (h.section_idx, h.subsection, h.field_idx)
            })
        });
        if let Some((section_idx, subsection, field_idx)) = target {
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
            // Ctrl+K — очистить/вернуть запрос (как в прочих полях).
            (KeyCode::Char(c), m)
                if m.contains(KeyModifiers::CONTROL) && keys::physical_char(c) == 'k' =>
            {
                if let Some(s) = &mut self.search {
                    s.input.clear_undoable();
                }
                self.search_filter();
            }
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

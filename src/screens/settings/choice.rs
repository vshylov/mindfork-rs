//! Экран настроек — попап выбора Choice-поля и сброс поля к дефолту.
//! Часть модуля [`super`]; разбито из settings.rs.

use super::helpers::*;
use super::spec::{Access, FieldSpec, field_spec};
use super::*;

impl SettingsScreen {
    // ---------- попап выбора Choice-поля / сброс к дефолту ----------

    /// Список вариантов Choice-поля + индекс текущего (`None` — поле не Choice).
    pub(super) fn choice_menu(&self, id: FieldId) -> Option<(Vec<String>, usize)> {
        // Config Choice-поля (режимы/flash-attn/spec-type/тема) — из таблицы доступа.
        if let Some(FieldSpec {
            access: Access::Choice { options, .. },
            ..
        }) = field_spec(id)
        {
            return Some(options(&self.config, self.loc()));
        }
        match id {
            // Семплинг (Thinking/Reasoning/Verbosity) и выбор профиля — свои источники.
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
            // Язык каркаса профиля (ось A): варианты — все известные языки (вшитые +
            // внешние из data/locales/).
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

    /// Применяет выбор варианта по индексу через существующий цикл (`cycle_field`):
    /// делает столько шагов вперёд, сколько нужно от текущего до целевого.
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

    /// Поля текущей секции/подсекции, построенные из **дефолтного** конфига (для
    /// маркера «изменено» и сброса). Профили — те же (у них нет config-дефолта).
    pub(super) fn default_fields(&self) -> Vec<FieldRow> {
        let mut tmp = SettingsScreen::new(
            AppConfig::default(),
            self.profiles.clone(),
            self.language_locked.clone(),
        );
        // Снимок MCP копируем: иначе PTool-строки MCP-инструментов не нашли бы
        // пары в дефолтном наборе полей.
        tmp.mcp = self.mcp.clone();
        tmp.section_idx = self.section_idx;
        tmp.model_sub = self.model_sub;
        tmp.sampling_sub = self.sampling_sub;
        tmp.profile_sub = self.profile_sub;
        tmp.profile_idx = self.profile_idx;
        tmp.fields()
    }

    /// Сбрасывает config-поле к значению по умолчанию. Профильные поля и уже
    /// дефолтные значения — no-op (без лишнего сохранения).
    pub(super) fn reset_field(&mut self, id: FieldId) -> Option<SettingsIntent> {
        if is_profile_field(id) {
            return None;
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
        // Уже совпадает с дефолтом — ничего не делаем.
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
                // «—» — плейсхолдер пустого (Option::None); очищаем поле.
                let text = if def == "—" { "" } else { &def };
                self.apply_text(id, text)
            }
        }
    }
}

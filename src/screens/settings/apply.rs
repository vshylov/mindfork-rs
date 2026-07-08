//! Экран настроек — обработка клавиш и мутации рабочей копии: диспетчер ввода,
//! редактор поля, тумблеры, циклы значений, применение текста, сохранение.
//! Часть модуля [`super`]; разбито из settings.rs.

use super::helpers::*;
use super::*;

impl SettingsScreen {
    // ---------- обработка клавиш ----------

    /// Обрабатывает нажатие, возвращая намерение для `app` (или `None`).
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<SettingsIntent> {
        if key.kind != KeyEventKind::Press {
            return None;
        }
        // Ctrl+C — выход из приложения, откуда угодно на экране настроек (в т.ч. из
        // редактора поля). Матчим по «физической» латинской клавише — работает при
        // любой раскладке (см. shared::keys).
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && let KeyCode::Char(c) = key.code
            && keys::physical_char(c) == 'c'
        {
            return Some(SettingsIntent::Quit);
        }
        // Оверлей поиска перехватывает ввод (кроме Ctrl+C выше).
        if self.search.is_some() {
            return self.handle_search_key(key);
        }
        // Попап выбора Choice-поля.
        if self.choice.is_some() {
            return self.handle_choice_key(key);
        }
        if self.editor.is_some() {
            return self.handle_editor_key(key);
        }
        // `/` открывает поиск по полям (в редакторе `/` — обычный символ, обработан
        // выше). Матчим по «физической» клавише `/` — при русской раскладке та же
        // клавиша отдаёт `.` (см. shared::keys::is_slash_key).
        if let KeyCode::Char(c) = key.code
            && keys::is_slash_key(c)
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT)
        {
            self.open_search();
            return None;
        }
        // Создать/удалить профиль (в секции «Профили»). Матчим по «физической»
        // латинской клавише — шорткаты работают при любой раскладке (см. shared::keys).
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && self.section() == Section::Profiles
            && let KeyCode::Char(c) = key.code
        {
            match keys::physical_char(c) {
                'n' => {
                    return Some(SettingsIntent::CreateProfile {
                        name: "Новый профиль".into(),
                        system_message: String::new(),
                    });
                }
                'd' => {
                    return self
                        .profiles
                        .get(self.profile_idx)
                        .map(|p| SettingsIntent::DeleteProfile(p.id));
                }
                _ => {}
            }
        }
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => Some(SettingsIntent::Close),
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
            KeyCode::Enter | KeyCode::Right => {
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
            KeyCode::Left => {
                // ←: для Choice — переключение значения, иначе уход в меню.
                if let Some(f) = fields.get(self.field_idx)
                    && matches!(f.kind, FieldKind::Choice(_))
                {
                    return self.cycle_field(f.id, -1);
                }
                self.focus = Focus::Menu;
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
            // Del — сброс поля к значению по умолчанию (config-поля; профильные — no-op).
            KeyCode::Delete => {
                let id = fields.get(self.field_idx)?.id;
                self.reset_field(id)
            }
            KeyCode::Enter => {
                let f = fields.get(self.field_idx)?;
                match &f.kind {
                    FieldKind::Toggle(_) => self.toggle_field(f.id),
                    // Choice (в т.ч. выбор профиля PSelect) — попап списка вариантов.
                    FieldKind::Choice(_) => {
                        self.open_choice(f.id);
                        None
                    }
                    FieldKind::Text(value) => {
                        // Системное сообщение и приветствие — многострочные
                        // (перенос + переводы строк); прочие поля — однострочные
                        // (горизонтальный скролл, без переноса на невидимый ряд).
                        // См. spec §11.6.
                        let multiline = matches!(
                            f.id,
                            FieldId::PSystem | FieldId::PGreeting | FieldId::PImpSystem
                        );
                        let mut input = InputBox::new();
                        input.set_single_line(!multiline);
                        // Не показываем плейсхолдеры «(все)»/«—» как значение.
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
        let editor = self.editor.as_mut()?;
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                self.editor = None;
                None
            }
            // Многострочный редактор (системное сообщение/приветствие): Shift+Enter —
            // перевод строки, Enter — коммит (как в чат-вводе, spec §11.7).
            (KeyCode::Enter, KeyModifiers::SHIFT) if editor.multiline => {
                editor.input.insert_newline();
                None
            }
            (KeyCode::Enter, _) => {
                let text = editor.input.text();
                // Валидация без закрытия: невалидное числовое поле оставляет редактор
                // открытым, подпись краснеет; исправление или Esc закрывают.
                if let Some(err) = field_validation_error(editor.field, &text) {
                    editor.error = Some(err);
                    return None;
                }
                let editor = self.editor.take().unwrap();
                self.apply_text(editor.field, &text)
            }
            // Удалить весь текст поля / вернуть удалённое (spec §11.5). Матчим по
            // «физической» клавише — срабатывает при любой раскладке (как в чат-вводе).
            (KeyCode::Char(c), m)
                if m.contains(KeyModifiers::CONTROL) && keys::physical_char(c) == 'k' =>
            {
                editor.input.clear_or_restore();
                editor.error = None;
                None
            }
            _ => {
                editor.input.on_key(key);
                editor.error = None; // правка сбрасывает прежнюю ошибку
                None
            }
        }
    }

    /// Вставка из буфера обмена (bracketed paste): осмысленна только когда открыт
    /// текстовый редактор поля (например, путь к модели) — иначе no-op. См. spec §11.5.
    pub fn handle_paste(&mut self, text: &str) {
        if let Some(search) = self.search.as_mut() {
            search.input.insert_str(text);
            self.search_filter();
        } else if let Some(editor) = self.editor.as_mut() {
            editor.input.insert_str(text);
        }
    }

    pub(super) fn move_section(&mut self, delta: i32) {
        let n = SECTIONS.len() as i32;
        self.section_idx = (((self.section_idx as i32 + delta) % n + n) % n) as usize;
        self.field_idx = 0;
        self.focus = Focus::Menu;
    }

    // ---------- применение правок ----------

    /// Значение для затравки редактора (без плейсхолдеров).
    pub(super) fn field_seed(&self, id: FieldId, shown: &str) -> String {
        match id {
            FieldId::IDicts => self.config.interface.selected_dictionaries.join(", "),
            // «—» для пустых числовых — затравка пустой.
            _ if shown == "—" => String::new(),
            _ => shown.to_string(),
        }
    }

    /// Переключает булев тумблер и возвращает соответствующее намерение.
    pub(super) fn toggle_field(&mut self, id: FieldId) -> Option<SettingsIntent> {
        match id {
            FieldId::XJinja => self.config.engine.managed.jinja = !self.config.engine.managed.jinja,
            FieldId::XNoMmap => {
                self.config.engine.managed.no_mmap = !self.config.engine.managed.no_mmap
            }
            FieldId::IxJinja => {
                self.config.impersonation_engine.managed.jinja =
                    !self.config.impersonation_engine.managed.jinja
            }
            FieldId::IxNoMmap => {
                self.config.impersonation_engine.managed.no_mmap =
                    !self.config.impersonation_engine.managed.no_mmap
            }
            FieldId::TWeb => self.config.tools.web_enabled = !self.config.tools.web_enabled,
            FieldId::TWebFetch => {
                self.config.tools.web_fetch_content = !self.config.tools.web_fetch_content
            }
            FieldId::TPython => {
                self.config.tools.python_enabled = !self.config.tools.python_enabled
            }
            FieldId::TFs => self.config.tools.fs_enabled = !self.config.tools.fs_enabled,
            FieldId::ICompat => {
                self.config.interface.terminal_compat = !self.config.interface.terminal_compat
            }
            FieldId::ISpell => {
                self.config.interface.spellcheck_enabled = !self.config.interface.spellcheck_enabled
            }
            FieldId::IConfirmKeys => {
                self.config.interface.confirm_destructive_keys =
                    !self.config.interface.confirm_destructive_keys
            }
            FieldId::SmProtocol => {
                self.config.self_model.maintenance_protocol =
                    !self.config.self_model.maintenance_protocol
            }
            FieldId::NotesRecallIncludesSelf => {
                self.config.notes.recall_includes_self = !self.config.notes.recall_includes_self
            }
            FieldId::ICopyThoughts => {
                self.config.copy.copy_thoughts = !self.config.copy.copy_thoughts
            }
            FieldId::ICopyToolCalls => {
                self.config.copy.copy_tool_calls = !self.config.copy.copy_tool_calls
            }
            FieldId::ICopyToolResults => {
                self.config.copy.copy_tool_results = !self.config.copy.copy_tool_results
            }
            FieldId::PTool(idx) => return self.toggle_profile_tool(idx),
            _ => return None,
        }
        Some(self.save_config())
    }

    pub(super) fn toggle_profile_tool(&mut self, idx: usize) -> Option<SettingsIntent> {
        let catalog = Self::tool_catalog();
        let tool = catalog.get(idx)?.id.clone();
        let p = self.profiles.get_mut(self.profile_idx)?;
        if let Some(pos) = p.enabled_tools.iter().position(|t| t == &tool) {
            p.enabled_tools.remove(pos);
        } else {
            p.enabled_tools.push(tool);
        }
        Some(self.save_profile())
    }

    /// Циклически меняет значение Choice-поля.
    pub(super) fn cycle_field(&mut self, id: FieldId, dir: i32) -> Option<SettingsIntent> {
        match id {
            // Переключение подсекций (таб-стрип) — чисто навигация, без сохранения.
            // Модель — три вкладки с учётом направления; Семплинг/Профили — две.
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
            FieldId::XMode => {
                self.config.engine.mode = cycle_mode(self.config.engine.mode, dir);
                Some(self.save_config())
            }
            FieldId::IxMode => {
                self.config.impersonation_engine.mode =
                    cycle_imp_mode(self.config.impersonation_engine.mode, dir);
                Some(self.save_config())
            }
            FieldId::XFlashAttn => {
                let m = &mut self.config.engine.managed;
                m.flash_attn = m.flash_attn.cycle(dir);
                Some(self.save_config())
            }
            FieldId::XSpecType => {
                let m = &mut self.config.engine.managed;
                m.spec_type = m.spec_type.cycle(dir);
                Some(self.save_config())
            }
            FieldId::IxFlashAttn => {
                let m = &mut self.config.impersonation_engine.managed;
                m.flash_attn = m.flash_attn.cycle(dir);
                Some(self.save_config())
            }
            FieldId::IxSpecType => {
                let m = &mut self.config.impersonation_engine.managed;
                m.spec_type = m.spec_type.cycle(dir);
                Some(self.save_config())
            }
            FieldId::EMode => {
                self.config.embed.mode = cycle_mode(self.config.embed.mode, dir);
                Some(self.save_config())
            }
            FieldId::ITheme => {
                self.config.interface.theme = cycle_theme(self.config.interface.theme);
                Some(self.save_config())
            }
            FieldId::S(p) => self.cycle_sampling_field(false, p),
            FieldId::IS(p) => self.cycle_sampling_field(true, p),
            FieldId::PSelect => {
                if !self.profiles.is_empty() {
                    let n = self.profiles.len() as i32;
                    self.profile_idx = (((self.profile_idx as i32 + dir) % n + n) % n) as usize;
                }
                None
            }
            _ => None,
        }
    }

    /// Циклически меняет Choice-параметр семплинга (`Thinking`/`Reasoning`) в нужной
    /// подсекции. Для числовых параметров — no-op (`None`), чтобы ←/→ над текстовым
    /// полем не порождали лишнего сохранения.
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
                _ => return None,
            }
        }
        Some(self.save_config())
    }

    /// Применяет текст из редактора к полю и возвращает намерение сохранения.
    pub(super) fn apply_text(&mut self, id: FieldId, text: &str) -> Option<SettingsIntent> {
        let trimmed = text.trim();
        let opt = |s: &str| (!s.is_empty()).then(|| s.to_string());
        let s = &mut self.config;
        match id {
            // URL/модель/ключ маршрутизируются в активную под-секцию по режиму
            // (external → external.*, облако → cloud_mut().*); managed-поля — в managed.
            FieldId::XUrl => {
                if s.engine.mode == ServerMode::External {
                    s.engine.external.url = opt(trimmed);
                } else if let Some(c) = s.engine.cloud_mut() {
                    c.url = opt(trimmed);
                }
            }
            FieldId::XModelName => {
                if s.engine.mode == ServerMode::External {
                    s.engine.external.model_name = opt(trimmed);
                } else if let Some(c) = s.engine.cloud_mut() {
                    c.model_name = opt(trimmed);
                }
            }
            FieldId::XApiKeyEnv => {
                if let Some(c) = s.engine.cloud_mut() {
                    c.api_key_env = opt(trimmed);
                }
            }
            FieldId::XBinary => s.engine.managed.binary = opt(trimmed),
            FieldId::XModel => s.engine.managed.model_path = opt(trimmed),
            FieldId::XDraftModel => s.engine.managed.draft_model = opt(trimmed),
            FieldId::XDraftNgl => {
                s.engine.managed.draft_gpu_layers =
                    parse_opt_num(trimmed, s.engine.managed.draft_gpu_layers)
            }
            FieldId::XDraftNMax => {
                s.engine.managed.draft_n_max = parse_opt_num(trimmed, s.engine.managed.draft_n_max)
            }
            FieldId::XDraftNMin => {
                s.engine.managed.draft_n_min = parse_opt_num(trimmed, s.engine.managed.draft_n_min)
            }
            FieldId::XHost => {
                if !trimmed.is_empty() {
                    s.engine.managed.host = trimmed.to_string();
                }
            }
            FieldId::XNgl => {
                if let Ok(v) = trimmed.parse() {
                    s.engine.managed.gpu_layers = v;
                }
            }
            FieldId::XCtx => {
                if let Ok(v) = trimmed.parse() {
                    s.engine.managed.context_size = v;
                }
            }
            FieldId::XPort => {
                if let Ok(p) = trimmed.parse() {
                    s.engine.managed.port = p;
                }
            }
            // Имперсонация — сервер.
            FieldId::IxUrl => {
                if s.impersonation_engine.mode == ImpersonationMode::External {
                    s.impersonation_engine.external.url = opt(trimmed);
                } else if let Some(c) = s.impersonation_engine.cloud_mut() {
                    c.url = opt(trimmed);
                }
            }
            FieldId::IxModelName => {
                if s.impersonation_engine.mode == ImpersonationMode::External {
                    s.impersonation_engine.external.model_name = opt(trimmed);
                } else if let Some(c) = s.impersonation_engine.cloud_mut() {
                    c.model_name = opt(trimmed);
                }
            }
            FieldId::IxApiKeyEnv => {
                if let Some(c) = s.impersonation_engine.cloud_mut() {
                    c.api_key_env = opt(trimmed);
                }
            }
            FieldId::IxBinary => s.impersonation_engine.managed.binary = opt(trimmed),
            FieldId::IxModel => s.impersonation_engine.managed.model_path = opt(trimmed),
            FieldId::IxDraftModel => s.impersonation_engine.managed.draft_model = opt(trimmed),
            FieldId::IxDraftNgl => {
                s.impersonation_engine.managed.draft_gpu_layers =
                    parse_opt_num(trimmed, s.impersonation_engine.managed.draft_gpu_layers)
            }
            FieldId::IxDraftNMax => {
                s.impersonation_engine.managed.draft_n_max =
                    parse_opt_num(trimmed, s.impersonation_engine.managed.draft_n_max)
            }
            FieldId::IxDraftNMin => {
                s.impersonation_engine.managed.draft_n_min =
                    parse_opt_num(trimmed, s.impersonation_engine.managed.draft_n_min)
            }
            FieldId::IxHost => {
                if !trimmed.is_empty() {
                    s.impersonation_engine.managed.host = trimmed.to_string();
                }
            }
            FieldId::IxNgl => {
                if let Ok(v) = trimmed.parse() {
                    s.impersonation_engine.managed.gpu_layers = v;
                }
            }
            FieldId::IxCtx => {
                if let Ok(v) = trimmed.parse() {
                    s.impersonation_engine.managed.context_size = v;
                }
            }
            FieldId::IxPort => {
                if let Ok(p) = trimmed.parse() {
                    s.impersonation_engine.managed.port = p;
                }
            }
            FieldId::MaxToolRounds => {
                if let Ok(v) = trimmed.parse() {
                    s.max_tool_rounds = v;
                }
            }
            FieldId::S(p) => apply_sampling_text(&mut s.default_sampling, p, trimmed),
            FieldId::IS(p) => apply_sampling_text(&mut s.impersonation_sampling, p, trimmed),
            FieldId::TPythonPath => s.tools.python_path = opt(trimmed),
            FieldId::TFsRoot => s.tools.fs_root = opt(trimmed),
            FieldId::TSubMaxTokens => {
                if let Ok(v) = trimmed.parse() {
                    s.tools.subagent_max_tokens = v;
                }
            }
            FieldId::TSubTimeout => {
                if let Ok(v) = trimmed.parse() {
                    s.tools.subagent_timeout_secs = v;
                }
            }
            FieldId::EUrl => {
                if s.embed.mode == ServerMode::External {
                    s.embed.external.url = opt(trimmed);
                } else if let Some(c) = s.embed.cloud_mut() {
                    c.url = opt(trimmed);
                }
            }
            FieldId::EModelName => {
                if s.embed.mode == ServerMode::External {
                    s.embed.external.model_name = opt(trimmed);
                } else if let Some(c) = s.embed.cloud_mut() {
                    c.model_name = opt(trimmed);
                }
            }
            FieldId::EApiKeyEnv => {
                if let Some(c) = s.embed.cloud_mut() {
                    c.api_key_env = opt(trimmed);
                }
            }
            FieldId::EBinary => s.embed.managed.binary = opt(trimmed),
            FieldId::EModel => s.embed.managed.model_path = opt(trimmed),
            FieldId::EPort => {
                if let Ok(p) = trimmed.parse() {
                    s.embed.managed.port = p;
                }
            }
            FieldId::RagTarget => {
                if let Ok(v) = trimmed.parse() {
                    s.rag.chunk_target_chars = v;
                }
            }
            FieldId::RagOverlap => {
                if let Ok(v) = trimmed.parse() {
                    s.rag.chunk_overlap_chars = v;
                }
            }
            FieldId::RagMax => {
                if let Ok(v) = trimmed.parse() {
                    s.rag.chunk_max_chars = v;
                }
            }
            FieldId::SmMaxNarrative => {
                if let Ok(v) = trimmed.parse() {
                    s.self_model.max_narrative = v;
                }
            }
            FieldId::SmNarrativeInPrompt => {
                if let Ok(v) = trimmed.parse() {
                    s.self_model.narrative_in_prompt = v;
                }
            }
            FieldId::SmPromptCap => {
                if let Ok(v) = trimmed.parse() {
                    s.self_model.prompt_cap = v;
                }
            }
            FieldId::SmSummaryTarget => {
                if let Ok(v) = trimmed.parse() {
                    s.self_model.summary_target_chars = v;
                }
            }
            FieldId::SmAutoReflect => {
                if let Ok(v) = trimmed.parse() {
                    s.self_model.auto_reflect_every = v;
                }
            }
            FieldId::NotesAutoConsolidate => {
                if let Ok(v) = trimmed.parse() {
                    s.notes.auto_consolidate_every = v;
                }
            }
            FieldId::IDicts => {
                s.interface.selected_dictionaries = trimmed
                    .split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect();
            }
            // Поля профиля.
            FieldId::PName | FieldId::PSystem | FieldId::PGreeting | FieldId::PImpSystem => {
                return self.apply_profile_text(id, trimmed);
            }
            _ => return None,
        }
        Some(self.save_config())
    }

    pub(super) fn apply_profile_text(&mut self, id: FieldId, text: &str) -> Option<SettingsIntent> {
        let p = self.profiles.get_mut(self.profile_idx)?;
        match id {
            FieldId::PName => {
                if text.is_empty() {
                    return None; // пустое имя не применяем
                }
                p.name = text.to_string();
            }
            FieldId::PSystem => p.default_system_message = text.to_string(),
            FieldId::PImpSystem => p.impersonation_system_message = text.to_string(),
            FieldId::PGreeting => {
                p.greeting = (!text.is_empty()).then(|| text.to_string());
            }
            _ => return None,
        }
        Some(self.save_profile())
    }

    /// Намерение сохранить текущую рабочую конфигурацию.
    pub(super) fn save_config(&self) -> SettingsIntent {
        SettingsIntent::SaveConfig(Box::new(self.config.clone()))
    }

    /// Намерение сохранить выбранный профиль (полный снимок его полей).
    pub(super) fn save_profile(&self) -> SettingsIntent {
        let p = &self.profiles[self.profile_idx];
        SettingsIntent::SaveProfile {
            id: p.id,
            edit: Box::new(ProfileEdit {
                name: Some(p.name.clone()),
                system_message: Some(p.default_system_message.clone()),
                impersonation_system_message: Some(p.impersonation_system_message.clone()),
                greeting: Some(p.greeting.clone()),
                character_names: Some(p.character_names.clone()),
                default_sampling: Some(p.default_sampling.clone()),
                enabled_tools: Some(p.enabled_tools.clone()),
            }),
        }
    }
}

//! Экран настроек (FSD "page"): секции, навигация и редактирование полей.
//! Вход по `Ctrl+P` из чата. См. spec §11.6.
//!
//! Как и [`super::chat::ChatScreen`], экран не знает про `app`/каналы: на правки
//! он возвращает [`SettingsIntent`], который `app` транслирует в `AppCommand`
//! (`UpdateConfig`/`UpdateProfile`/`CreateProfile`/`DeleteProfile`). Правки
//! применяются **сразу при коммите** поля (оркестратор — единственный писатель и
//! перезапускает сервер при смене модели). Работает на собственной рабочей копии
//! `AppConfig`/профилей, обновляемой теми же правками.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};
use uuid::Uuid;

use crate::entities::profile::Profile;
use crate::entities::sampling::ReasoningEffort;
use crate::features::profiles::ProfileEdit;
use crate::features::tools::default_tool_ids;
use crate::shared::config::{AppConfig, ServerMode, Theme};
use crate::shared::keys;
use crate::shared::theme::Palette;
use crate::widgets::input_box::InputBox;

/// Намерение, которое исполняет `app` (транслирует в `AppCommand`).
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsIntent {
    /// Закрыть экран настроек (вернуться в чат).
    Close,
    /// Сохранить конфигурацию (правка любой секции, кроме профилей).
    SaveConfig(Box<AppConfig>),
    /// Сохранить правки профиля.
    SaveProfile { id: Uuid, edit: Box<ProfileEdit> },
    /// Создать новый профиль.
    CreateProfile {
        name: String,
        system_message: String,
    },
    /// Удалить профиль.
    DeleteProfile(Uuid),
}

/// Секции настроек (левое меню). См. spec §11.6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Model,
    Inference,
    Sampling,
    Profiles,
    Tools,
    Interface,
}

const SECTIONS: [Section; 6] = [
    Section::Model,
    Section::Inference,
    Section::Sampling,
    Section::Profiles,
    Section::Tools,
    Section::Interface,
];

impl Section {
    fn title(self) -> &'static str {
        match self {
            Section::Model => "Модель/сервер",
            Section::Inference => "Инференс",
            Section::Sampling => "Семплинг",
            Section::Profiles => "Профили",
            Section::Tools => "Инструменты",
            Section::Interface => "Интерфейс",
        }
    }
}

/// Идентификатор редактируемого поля (стабильный порядок = порядок в секции).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldId {
    // Модель/сервер (llama-server)
    XMode,
    XUrl,
    XBinary,
    XModel,
    XNgl,
    XCtx,
    XJinja,
    XHost,
    XPort,
    // Инференс
    MaxToolRounds,
    // Семплинг
    STemp,
    STopK,
    STopP,
    SFreqPen,
    SPresPen,
    SMaxTokens,
    SThinking,
    SReasoning,
    // Инструменты
    TWeb,
    TPython,
    TPythonPath,
    TSubMaxTokens,
    TSubTimeout,
    EMode,
    EUrl,
    EBinary,
    EModel,
    EPort,
    // Интерфейс
    ITheme,
    ISpell,
    IDicts,
    // Профили (динамические)
    PSelect,
    PName,
    PSystem,
    PGreeting,
    /// Переключатель инструмента профиля по индексу в каталоге.
    PTool(usize),
}

/// Способ редактирования поля (для отрисовки и обработки клавиш).
enum FieldKind {
    /// Булев тумблер (Space переключает).
    Toggle(bool),
    /// Циклический выбор из вариантов (←/→ переключают).
    Choice(String),
    /// Текст/число (Enter открывает редактор).
    Text(String),
}

/// Строка поля: идентификатор, подпись и текущее представление значения.
struct FieldRow {
    id: FieldId,
    label: String,
    kind: FieldKind,
}

/// Активный редактор текстового поля (попап).
struct Editor {
    field: FieldId,
    input: InputBox,
}

/// Фокус: левое меню секций или список полей справа.
#[derive(PartialEq)]
enum Focus {
    Menu,
    Fields,
}

/// Экран настроек: рабочая копия конфигурации и профилей + состояние навигации.
pub struct SettingsScreen {
    config: AppConfig,
    profiles: Vec<Profile>,
    section_idx: usize,
    field_idx: usize,
    focus: Focus,
    /// Выбранный профиль в секции «Профили».
    profile_idx: usize,
    editor: Option<Editor>,
}

impl SettingsScreen {
    /// Создаёт экран из снимка настроек (конфиг + видимые профили).
    pub fn new(config: AppConfig, profiles: Vec<Profile>) -> Self {
        Self {
            config,
            profiles,
            section_idx: 0,
            field_idx: 0,
            focus: Focus::Menu,
            profile_idx: 0,
            editor: None,
        }
    }

    /// Обновляет рабочую копию из переэмита настроек (после create/delete профиля
    /// или эха правки). Навигация и активный редактор сохраняются.
    pub fn refresh(&mut self, config: AppConfig, profiles: Vec<Profile>) {
        self.config = config;
        self.profiles = profiles;
        if !self.profiles.is_empty() && self.profile_idx >= self.profiles.len() {
            self.profile_idx = self.profiles.len() - 1;
        }
    }

    fn section(&self) -> Section {
        SECTIONS[self.section_idx]
    }

    /// Каталог всех известных инструментов (для тумблеров в профиле).
    fn tool_catalog() -> Vec<String> {
        default_tool_ids()
    }

    // ---------- построение полей текущей секции ----------

    fn fields(&self) -> Vec<FieldRow> {
        match self.section() {
            Section::Model => self.model_fields(),
            Section::Inference => self.inference_fields(),
            Section::Sampling => self.sampling_fields(),
            Section::Profiles => self.profile_fields(),
            Section::Tools => self.tool_fields(),
            Section::Interface => self.interface_fields(),
        }
    }

    fn model_fields(&self) -> Vec<FieldRow> {
        let x = &self.config.engine;
        vec![
            row(
                FieldId::XMode,
                "Режим",
                FieldKind::Choice(mode_label(x.mode)),
            ),
            text_row(FieldId::XUrl, "URL (external)", &x.url),
            text_row(FieldId::XBinary, "Бинарник llama-server", &x.binary),
            text_row(FieldId::XModel, "GGUF-модель (-m)", &x.model_path),
            row(
                FieldId::XNgl,
                "GPU-слои (-ngl)",
                FieldKind::Text(x.gpu_layers.to_string()),
            ),
            row(
                FieldId::XCtx,
                "Контекст (-c)",
                FieldKind::Text(x.context_size.to_string()),
            ),
            row(FieldId::XJinja, "--jinja", FieldKind::Toggle(x.jinja)),
            row(FieldId::XHost, "Host", FieldKind::Text(x.host.clone())),
            row(FieldId::XPort, "Порт", FieldKind::Text(x.port.to_string())),
        ]
    }

    fn inference_fields(&self) -> Vec<FieldRow> {
        vec![row(
            FieldId::MaxToolRounds,
            "Лимит раундов инструментов",
            FieldKind::Text(self.config.max_tool_rounds.to_string()),
        )]
    }

    fn sampling_fields(&self) -> Vec<FieldRow> {
        let s = &self.config.default_sampling;
        vec![
            num_row(FieldId::STemp, "Температура", s.temperature),
            num_row(FieldId::STopK, "top_k", s.top_k),
            num_row(FieldId::STopP, "top_p", s.top_p),
            num_row(FieldId::SFreqPen, "frequency_penalty", s.frequency_penalty),
            num_row(FieldId::SPresPen, "presence_penalty", s.presence_penalty),
            num_row(FieldId::SMaxTokens, "max_tokens", s.max_tokens),
            row(
                FieldId::SThinking,
                "Мысли (thinking)",
                FieldKind::Choice(opt_bool_label(s.thinking)),
            ),
            row(
                FieldId::SReasoning,
                "reasoning_effort",
                FieldKind::Choice(reasoning_label(s.reasoning_effort)),
            ),
        ]
    }

    fn tool_fields(&self) -> Vec<FieldRow> {
        let t = &self.config.tools;
        let e = &self.config.embed;
        vec![
            row(FieldId::TWeb, "Web-поиск", FieldKind::Toggle(t.web_enabled)),
            row(
                FieldId::TPython,
                "Python-исполнение",
                FieldKind::Toggle(t.python_enabled),
            ),
            text_row(FieldId::TPythonPath, "Путь к Python", &t.python_path),
            row(
                FieldId::TSubMaxTokens,
                "call_subagent: max_tokens",
                FieldKind::Text(t.subagent_max_tokens.to_string()),
            ),
            row(
                FieldId::TSubTimeout,
                "call_subagent: таймаут (с)",
                FieldKind::Text(t.subagent_timeout_secs.to_string()),
            ),
            row(
                FieldId::EMode,
                "Эмбеддинги: режим",
                FieldKind::Choice(mode_label(e.mode)),
            ),
            text_row(FieldId::EUrl, "Эмбеддинги: URL", &e.url),
            text_row(FieldId::EBinary, "Эмбеддинги: бинарник", &e.binary),
            text_row(FieldId::EModel, "Эмбеддинги: GGUF (-m)", &e.model_path),
            row(
                FieldId::EPort,
                "Эмбеддинги: порт",
                FieldKind::Text(e.port.to_string()),
            ),
        ]
    }

    fn interface_fields(&self) -> Vec<FieldRow> {
        let i = &self.config.interface;
        vec![
            row(
                FieldId::ITheme,
                "Тема",
                FieldKind::Choice(theme_label(i.theme)),
            ),
            row(
                FieldId::ISpell,
                "Спелл-чек",
                FieldKind::Toggle(i.spellcheck_enabled),
            ),
            row(
                FieldId::IDicts,
                "Словари (через запятую)",
                FieldKind::Text(if i.selected_dictionaries.is_empty() {
                    "(все)".to_string()
                } else {
                    i.selected_dictionaries.join(", ")
                }),
            ),
        ]
    }

    fn profile_fields(&self) -> Vec<FieldRow> {
        let Some(p) = self.profiles.get(self.profile_idx) else {
            return vec![row(
                FieldId::PSelect,
                "Профиль",
                FieldKind::Choice("(нет профилей)".to_string()),
            )];
        };
        let mut rows = vec![
            row(
                FieldId::PSelect,
                "Профиль",
                FieldKind::Choice(p.name.clone()),
            ),
            row(FieldId::PName, "Имя", FieldKind::Text(p.name.clone())),
            row(
                FieldId::PSystem,
                "Системное сообщение",
                FieldKind::Text(p.default_system_message.clone()),
            ),
            row(
                FieldId::PGreeting,
                "Приветствие",
                FieldKind::Text(p.greeting.clone().unwrap_or_default()),
            ),
        ];
        for (idx, tool) in Self::tool_catalog().into_iter().enumerate() {
            let on = p.enabled_tools.iter().any(|t| t == &tool);
            rows.push(row(
                FieldId::PTool(idx),
                &format!("инструмент: {tool}"),
                FieldKind::Toggle(on),
            ));
        }
        rows
    }

    // ---------- обработка клавиш ----------

    /// Обрабатывает нажатие, возвращая намерение для `app` (или `None`).
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<SettingsIntent> {
        if key.kind != KeyEventKind::Press {
            return None;
        }
        if self.editor.is_some() {
            return self.handle_editor_key(key);
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

    fn handle_menu_key(&mut self, key: KeyEvent) -> Option<SettingsIntent> {
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

    fn handle_fields_key(&mut self, key: KeyEvent) -> Option<SettingsIntent> {
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
            KeyCode::Enter => {
                let f = fields.get(self.field_idx)?;
                match &f.kind {
                    FieldKind::Toggle(_) => self.toggle_field(f.id),
                    FieldKind::Choice(_) => self.cycle_field(f.id, 1),
                    FieldKind::Text(value) => {
                        // Открываем редактор; PSelect — выбор профиля, не текст.
                        if f.id == FieldId::PSelect {
                            None
                        } else {
                            let mut input = InputBox::new();
                            // Не показываем плейсхолдеры «(все)»/«—» как значение.
                            let seed = self.field_seed(f.id, value);
                            input.set_text(&seed);
                            self.editor = Some(Editor { field: f.id, input });
                            None
                        }
                    }
                }
            }
            _ => None,
        }
    }

    fn handle_editor_key(&mut self, key: KeyEvent) -> Option<SettingsIntent> {
        let editor = self.editor.as_mut()?;
        match key.code {
            KeyCode::Esc => {
                self.editor = None;
                None
            }
            KeyCode::Enter => {
                let editor = self.editor.take().unwrap();
                let text = editor.input.text();
                self.apply_text(editor.field, &text)
            }
            _ => {
                editor.input.on_key(key);
                None
            }
        }
    }

    fn move_section(&mut self, delta: i32) {
        let n = SECTIONS.len() as i32;
        self.section_idx = (((self.section_idx as i32 + delta) % n + n) % n) as usize;
        self.field_idx = 0;
        self.focus = Focus::Menu;
    }

    // ---------- применение правок ----------

    /// Значение для затравки редактора (без плейсхолдеров).
    fn field_seed(&self, id: FieldId, shown: &str) -> String {
        match id {
            FieldId::IDicts => self.config.interface.selected_dictionaries.join(", "),
            // «—» для пустых числовых — затравка пустой.
            _ if shown == "—" => String::new(),
            _ => shown.to_string(),
        }
    }

    /// Переключает булев тумблер и возвращает соответствующее намерение.
    fn toggle_field(&mut self, id: FieldId) -> Option<SettingsIntent> {
        match id {
            FieldId::XJinja => self.config.engine.jinja = !self.config.engine.jinja,
            FieldId::TWeb => self.config.tools.web_enabled = !self.config.tools.web_enabled,
            FieldId::TPython => {
                self.config.tools.python_enabled = !self.config.tools.python_enabled
            }
            FieldId::ISpell => {
                self.config.interface.spellcheck_enabled = !self.config.interface.spellcheck_enabled
            }
            FieldId::PTool(idx) => return self.toggle_profile_tool(idx),
            _ => return None,
        }
        Some(self.save_config())
    }

    fn toggle_profile_tool(&mut self, idx: usize) -> Option<SettingsIntent> {
        let catalog = Self::tool_catalog();
        let tool = catalog.get(idx)?.clone();
        let p = self.profiles.get_mut(self.profile_idx)?;
        if let Some(pos) = p.enabled_tools.iter().position(|t| t == &tool) {
            p.enabled_tools.remove(pos);
        } else {
            p.enabled_tools.push(tool);
        }
        Some(self.save_profile())
    }

    /// Циклически меняет значение Choice-поля.
    fn cycle_field(&mut self, id: FieldId, dir: i32) -> Option<SettingsIntent> {
        match id {
            FieldId::XMode => {
                self.config.engine.mode = cycle_mode(self.config.engine.mode);
                Some(self.save_config())
            }
            FieldId::EMode => {
                self.config.embed.mode = cycle_mode(self.config.embed.mode);
                Some(self.save_config())
            }
            FieldId::ITheme => {
                self.config.interface.theme = cycle_theme(self.config.interface.theme);
                Some(self.save_config())
            }
            FieldId::SThinking => {
                self.config.default_sampling.thinking =
                    cycle_opt_bool(self.config.default_sampling.thinking);
                Some(self.save_config())
            }
            FieldId::SReasoning => {
                self.config.default_sampling.reasoning_effort =
                    cycle_reasoning(self.config.default_sampling.reasoning_effort);
                Some(self.save_config())
            }
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

    /// Применяет текст из редактора к полю и возвращает намерение сохранения.
    fn apply_text(&mut self, id: FieldId, text: &str) -> Option<SettingsIntent> {
        let trimmed = text.trim();
        let opt = |s: &str| (!s.is_empty()).then(|| s.to_string());
        let s = &mut self.config;
        match id {
            FieldId::XUrl => s.engine.url = opt(trimmed),
            FieldId::XBinary => s.engine.binary = opt(trimmed),
            FieldId::XModel => s.engine.model_path = opt(trimmed),
            FieldId::XHost => {
                if !trimmed.is_empty() {
                    s.engine.host = trimmed.to_string();
                }
            }
            FieldId::XNgl => {
                if let Ok(v) = trimmed.parse() {
                    s.engine.gpu_layers = v;
                }
            }
            FieldId::XCtx => {
                if let Ok(v) = trimmed.parse() {
                    s.engine.context_size = v;
                }
            }
            FieldId::XPort => {
                if let Ok(p) = trimmed.parse() {
                    s.engine.port = p;
                }
            }
            FieldId::MaxToolRounds => {
                if let Ok(v) = trimmed.parse() {
                    s.max_tool_rounds = v;
                }
            }
            FieldId::STemp => s.default_sampling.temperature = parse_opt_f32(trimmed),
            FieldId::STopK => s.default_sampling.top_k = parse_opt(trimmed),
            FieldId::STopP => s.default_sampling.top_p = parse_opt_f32(trimmed),
            FieldId::SFreqPen => s.default_sampling.frequency_penalty = parse_opt_f32(trimmed),
            FieldId::SPresPen => s.default_sampling.presence_penalty = parse_opt_f32(trimmed),
            FieldId::SMaxTokens => s.default_sampling.max_tokens = parse_opt(trimmed),
            FieldId::TPythonPath => s.tools.python_path = opt(trimmed),
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
            FieldId::EUrl => s.embed.url = opt(trimmed),
            FieldId::EBinary => s.embed.binary = opt(trimmed),
            FieldId::EModel => s.embed.model_path = opt(trimmed),
            FieldId::EPort => {
                if let Ok(p) = trimmed.parse() {
                    s.embed.port = p;
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
            FieldId::PName | FieldId::PSystem | FieldId::PGreeting => {
                return self.apply_profile_text(id, trimmed);
            }
            _ => return None,
        }
        Some(self.save_config())
    }

    fn apply_profile_text(&mut self, id: FieldId, text: &str) -> Option<SettingsIntent> {
        let p = self.profiles.get_mut(self.profile_idx)?;
        match id {
            FieldId::PName => {
                if text.is_empty() {
                    return None; // пустое имя не применяем
                }
                p.name = text.to_string();
            }
            FieldId::PSystem => p.default_system_message = text.to_string(),
            FieldId::PGreeting => {
                p.greeting = (!text.is_empty()).then(|| text.to_string());
            }
            _ => return None,
        }
        Some(self.save_profile())
    }

    /// Намерение сохранить текущую рабочую конфигурацию.
    fn save_config(&self) -> SettingsIntent {
        SettingsIntent::SaveConfig(Box::new(self.config.clone()))
    }

    /// Намерение сохранить выбранный профиль (полный снимок его полей).
    fn save_profile(&self) -> SettingsIntent {
        let p = &self.profiles[self.profile_idx];
        SettingsIntent::SaveProfile {
            id: p.id,
            edit: Box::new(ProfileEdit {
                name: Some(p.name.clone()),
                system_message: Some(p.default_system_message.clone()),
                greeting: Some(p.greeting.clone()),
                character_names: Some(p.character_names.clone()),
                default_sampling: Some(p.default_sampling.clone()),
                enabled_tools: Some(p.enabled_tools.clone()),
            }),
        }
    }

    // ---------- отрисовка ----------

    pub fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Настройки ")
            .title_bottom(
                Line::from(
                    " Tab секция · ↑↓ поля · Enter правка · Space тумблер · ←→ выбор · Esc выход ",
                )
                .dim(),
            );
        let inner = block.inner(area);
        frame.render_widget(Clear, area);
        frame.render_widget(&block, area);

        let [menu_area, fields_area] =
            Layout::horizontal([Constraint::Length(22), Constraint::Min(20)]).areas(inner);

        self.render_menu(frame, menu_area);
        self.render_fields(frame, fields_area);

        // Редактор поверх — с реальным курсором (InputBox::render требует &mut).
        let popup = centered_rect(60, 30, 3, frame.area());
        let palette = Palette::for_theme(self.config.interface.theme);
        if let Some(editor) = self.editor.as_mut() {
            frame.render_widget(Clear, popup);
            editor.input.render(
                frame,
                popup,
                "правка · Enter ок · Esc отмена",
                true,
                &palette,
            );
        }
    }

    fn render_menu(&self, frame: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = SECTIONS
            .iter()
            .map(|s| ListItem::new(Line::from(s.title())))
            .collect();
        let focused = self.focus == Focus::Menu;
        let block = Block::default().borders(Borders::RIGHT).title(if focused {
            "▶ Секции"
        } else {
            "Секции"
        });
        let hl = if focused {
            Style::new().reversed()
        } else {
            Style::new().bold()
        };
        let list = List::new(items).block(block).highlight_style(hl);
        let mut state = ListState::default();
        state.select(Some(self.section_idx));
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn render_fields(&self, frame: &mut Frame, area: Rect) {
        let fields = self.fields();
        let focused = self.focus == Focus::Fields;
        let items: Vec<ListItem> = fields
            .iter()
            .map(|f| ListItem::new(render_field_line(f)))
            .collect();
        let block = Block::default()
            .borders(Borders::NONE)
            .title(format!(" {} ", self.section().title()));
        let hl = if focused {
            Style::new().reversed()
        } else {
            Style::new()
        };
        let list = List::new(items).block(block).highlight_style(hl);
        let mut state = ListState::default();
        if focused && !fields.is_empty() {
            state.select(Some(self.field_idx.min(fields.len() - 1)));
        }
        frame.render_stateful_widget(list, area, &mut state);
    }
}

// ---------- свободные функции ----------

fn row(id: FieldId, label: &str, kind: FieldKind) -> FieldRow {
    FieldRow {
        id,
        label: label.to_string(),
        kind,
    }
}

/// Текстовая строка из `Option<String>` (пусто → «—»).
fn text_row(id: FieldId, label: &str, value: &Option<String>) -> FieldRow {
    row(
        id,
        label,
        FieldKind::Text(value.clone().unwrap_or_else(|| "—".to_string())),
    )
}

/// Числовая строка из `Option<T>` (None → «—»).
fn num_row<T: ToString>(id: FieldId, label: &str, value: Option<T>) -> FieldRow {
    row(
        id,
        label,
        FieldKind::Text(
            value
                .map(|v| v.to_string())
                .unwrap_or_else(|| "—".to_string()),
        ),
    )
}

fn render_field_line(f: &FieldRow) -> Line<'static> {
    let value = match &f.kind {
        FieldKind::Toggle(on) => {
            if *on {
                "[x]".to_string()
            } else {
                "[ ]".to_string()
            }
        }
        FieldKind::Choice(v) => format!("‹ {v} ›"),
        FieldKind::Text(v) => v.clone(),
    };
    Line::from(format!("{:<28} {}", f.label, value))
}

fn mode_label(m: ServerMode) -> String {
    match m {
        ServerMode::Managed => "managed".into(),
        ServerMode::External => "external".into(),
    }
}

fn cycle_mode(m: ServerMode) -> ServerMode {
    match m {
        ServerMode::Managed => ServerMode::External,
        ServerMode::External => ServerMode::Managed,
    }
}

fn theme_label(t: Theme) -> String {
    match t {
        Theme::Auto => "авто".into(),
        Theme::Dark => "тёмная".into(),
        Theme::Light => "светлая".into(),
    }
}

fn cycle_theme(t: Theme) -> Theme {
    match t {
        Theme::Auto => Theme::Dark,
        Theme::Dark => Theme::Light,
        Theme::Light => Theme::Auto,
    }
}

fn opt_bool_label(b: Option<bool>) -> String {
    match b {
        None => "—".into(),
        Some(true) => "вкл".into(),
        Some(false) => "выкл".into(),
    }
}

fn cycle_opt_bool(b: Option<bool>) -> Option<bool> {
    match b {
        None => Some(true),
        Some(true) => Some(false),
        Some(false) => None,
    }
}

fn reasoning_label(r: Option<ReasoningEffort>) -> String {
    match r {
        None => "—".into(),
        Some(e) => e.as_wire().to_string(),
    }
}

fn cycle_reasoning(r: Option<ReasoningEffort>) -> Option<ReasoningEffort> {
    match r {
        None => Some(ReasoningEffort::None),
        Some(ReasoningEffort::None) => Some(ReasoningEffort::Low),
        Some(ReasoningEffort::Low) => Some(ReasoningEffort::Medium),
        Some(ReasoningEffort::Medium) => Some(ReasoningEffort::High),
        Some(ReasoningEffort::High) => None,
    }
}

fn parse_opt<T: std::str::FromStr>(s: &str) -> Option<T> {
    if s.is_empty() { None } else { s.parse().ok() }
}

fn parse_opt_f32(s: &str) -> Option<f32> {
    parse_opt(s)
}

/// Прямоугольник по центру `area`: `pct_x`% ширины (≥`min_w`), фикс. высота.
fn centered_rect(pct_x: u16, min_w: u16, height: u16, area: Rect) -> Rect {
    let w = area.width.saturating_mul(pct_x) / 100;
    let [h] = Layout::horizontal([Constraint::Length(w.max(min_w).min(area.width))])
        .flex(Flex::Center)
        .areas(area);
    let [v] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(h);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> SettingsScreen {
        let mut p = Profile::new("Базовый", "Ты — ассистент.");
        p.enabled_tools = default_tool_ids();
        SettingsScreen::new(AppConfig::default(), vec![p])
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn esc_closes() {
        let mut s = screen();
        assert_eq!(s.handle_key(key(KeyCode::Esc)), Some(SettingsIntent::Close));
    }

    #[test]
    fn tab_cycles_sections() {
        let mut s = screen();
        assert_eq!(s.section(), Section::Model);
        s.handle_key(key(KeyCode::Tab));
        assert_eq!(s.section(), Section::Inference);
        s.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(s.section(), Section::Model);
    }

    #[test]
    fn toggle_web_emits_save_with_flipped_value() {
        let mut s = screen();
        // Переходим в Инструменты, в список полей, на первый тумблер (web).
        s.handle_key(key(KeyCode::Tab)); // Inference
        s.handle_key(key(KeyCode::Tab)); // Sampling
        s.handle_key(key(KeyCode::Tab)); // Profiles
        s.handle_key(key(KeyCode::Tab)); // Tools
        s.handle_key(key(KeyCode::Enter)); // фокус на поля
        let intent = s.handle_key(key(KeyCode::Char(' ')));
        match intent {
            Some(SettingsIntent::SaveConfig(c)) => assert!(!c.tools.web_enabled),
            other => panic!("ожидался SaveConfig, получено {other:?}"),
        }
    }

    #[test]
    fn cycle_mode_changes_server_mode() {
        let mut s = screen();
        s.handle_key(key(KeyCode::Enter)); // фокус на поля (Модель)
        // Первое поле — режим (Choice). →
        let intent = s.handle_key(key(KeyCode::Right));
        match intent {
            Some(SettingsIntent::SaveConfig(c)) => assert_eq!(c.engine.mode, ServerMode::External),
            other => panic!("ожидался SaveConfig, получено {other:?}"),
        }
    }

    #[test]
    fn editing_model_commits_text() {
        let mut s = screen();
        s.handle_key(key(KeyCode::Enter)); // фокус на поля (XMode)
        // XMode → XUrl → XBinary → XModel.
        for _ in 0..3 {
            s.handle_key(key(KeyCode::Down));
        }
        s.handle_key(key(KeyCode::Enter)); // открыть редактор XModel
        assert!(s.editor.is_some());
        for c in "gemma.gguf".chars() {
            s.handle_key(key(KeyCode::Char(c)));
        }
        let intent = s.handle_key(key(KeyCode::Enter));
        match intent {
            Some(SettingsIntent::SaveConfig(c)) => {
                assert_eq!(c.engine.model_path.as_deref(), Some("gemma.gguf"))
            }
            other => panic!("ожидался SaveConfig, получено {other:?}"),
        }
        assert!(s.editor.is_none());
    }

    #[test]
    fn editor_esc_discards() {
        let mut s = screen();
        s.handle_key(key(KeyCode::Enter)); // XMode
        s.handle_key(key(KeyCode::Down)); // XUrl
        s.handle_key(key(KeyCode::Down)); // XBinary
        s.handle_key(key(KeyCode::Enter)); // редактор XBinary
        s.handle_key(key(KeyCode::Char('x')));
        let intent = s.handle_key(key(KeyCode::Esc));
        assert_eq!(intent, None);
        assert!(s.editor.is_none());
        // значение не изменилось
        assert!(s.config.engine.binary.is_none());
    }

    #[test]
    fn create_and_delete_profile_in_profiles_section() {
        let mut s = screen();
        for _ in 0..3 {
            s.handle_key(key(KeyCode::Tab));
        }
        assert_eq!(s.section(), Section::Profiles);
        let create = s.handle_key(ctrl('n'));
        assert!(matches!(create, Some(SettingsIntent::CreateProfile { .. })));
        let id = s.profiles[0].id;
        let del = s.handle_key(ctrl('d'));
        assert_eq!(del, Some(SettingsIntent::DeleteProfile(id)));
    }

    #[test]
    fn toggling_profile_tool_emits_save_profile() {
        let mut s = screen();
        for _ in 0..3 {
            s.handle_key(key(KeyCode::Tab));
        }
        s.handle_key(key(KeyCode::Enter)); // фокус на поля
        // Перейти к первому тумблеру инструмента (после PSelect/PName/PSystem/PGreeting).
        for _ in 0..4 {
            s.handle_key(key(KeyCode::Down));
        }
        let before = s.profiles[0].enabled_tools.len();
        let intent = s.handle_key(key(KeyCode::Char(' ')));
        match intent {
            Some(SettingsIntent::SaveProfile { edit, .. }) => {
                let tools = edit.enabled_tools.unwrap();
                assert_eq!(tools.len(), before - 1, "первый инструмент выключился");
            }
            other => panic!("ожидался SaveProfile, получено {other:?}"),
        }
    }

    #[test]
    fn profile_select_cycles() {
        let mut p2 = Profile::new("Второй", "sys2");
        p2.enabled_tools = default_tool_ids();
        let mut s = SettingsScreen::new(AppConfig::default(), {
            let mut p1 = Profile::new("Первый", "sys1");
            p1.enabled_tools = default_tool_ids();
            vec![p1, p2]
        });
        for _ in 0..3 {
            s.handle_key(key(KeyCode::Tab));
        }
        s.handle_key(key(KeyCode::Enter)); // поля; курсор на PSelect
        assert_eq!(s.profile_idx, 0);
        s.handle_key(key(KeyCode::Right));
        assert_eq!(s.profile_idx, 1);
    }

    #[test]
    fn render_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut s = screen();
        for (w, h) in [(80u16, 24u16), (40, 12)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| s.render(f)).unwrap();
        }
    }

    #[test]
    fn render_with_editor_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut s = screen();
        s.handle_key(key(KeyCode::Enter));
        s.handle_key(key(KeyCode::Down));
        s.handle_key(key(KeyCode::Down));
        s.handle_key(key(KeyCode::Enter)); // редактор
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
    }
}

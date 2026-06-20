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
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use uuid::Uuid;

use crate::entities::profile::Profile;
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::features::profiles::ProfileEdit;
use crate::features::tools::default_tool_ids;
use crate::shared::config::{AppConfig, ImpersonationMode, ServerMode, Theme};
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

/// Подсекция «Ассистент» / «Имперсонация» внутри секций Модель/Семплинг/Профили.
/// См. spec §11.8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Subsection {
    Assistant,
    Impersonation,
}

impl Subsection {
    fn label(self) -> String {
        match self {
            Subsection::Assistant => "Ассистент".into(),
            Subsection::Impersonation => "Имперсонация".into(),
        }
    }

    fn toggled(self) -> Self {
        match self {
            Subsection::Assistant => Subsection::Impersonation,
            Subsection::Impersonation => Subsection::Assistant,
        }
    }
}

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

/// Параметр семплинга. Адресует конкретное поле [`SamplingConfig`] внутри
/// подсекции; сама подсекция («Ассистент»/«Имперсонация») кодируется
/// конструктором [`FieldId::S`]/[`FieldId::IS`]. Числовые параметры
/// редактируются текстом, `Thinking`/`Reasoning` — циклическим выбором.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SamplingParam {
    Temp,
    TopK,
    TopP,
    MinP,
    TopNSigma,
    TypicalP,
    FreqPen,
    PresPen,
    RepeatPenalty,
    RepeatLastN,
    DryMultiplier,
    DryBase,
    DryAllowedLength,
    DryPenaltyLastN,
    XtcProbability,
    XtcThreshold,
    Mirostat,
    MirostatTau,
    MirostatEta,
    MaxTokens,
    Seed,
    Thinking,
    Reasoning,
}

/// Порядок параметров семплинга в секции (стабильный = порядок отрисовки).
const SAMPLING_PARAMS: &[SamplingParam] = {
    use SamplingParam::*;
    &[
        Temp,
        TopK,
        TopP,
        MinP,
        TopNSigma,
        TypicalP,
        FreqPen,
        PresPen,
        RepeatPenalty,
        RepeatLastN,
        DryMultiplier,
        DryBase,
        DryAllowedLength,
        DryPenaltyLastN,
        XtcProbability,
        XtcThreshold,
        Mirostat,
        MirostatTau,
        MirostatEta,
        MaxTokens,
        Seed,
        Thinking,
        Reasoning,
    ]
};

impl SamplingParam {
    /// Подпись поля в UI.
    fn label(self) -> &'static str {
        use SamplingParam::*;
        match self {
            Temp => "Температура",
            TopK => "top_k",
            TopP => "top_p",
            MinP => "min_p",
            TopNSigma => "top_n_sigma",
            TypicalP => "typical_p",
            FreqPen => "frequency_penalty",
            PresPen => "presence_penalty",
            RepeatPenalty => "repeat_penalty",
            RepeatLastN => "repeat_last_n",
            DryMultiplier => "dry_multiplier",
            DryBase => "dry_base",
            DryAllowedLength => "dry_allowed_length",
            DryPenaltyLastN => "dry_penalty_last_n",
            XtcProbability => "xtc_probability",
            XtcThreshold => "xtc_threshold",
            Mirostat => "mirostat",
            MirostatTau => "mirostat_tau",
            MirostatEta => "mirostat_eta",
            MaxTokens => "max_tokens",
            Seed => "seed",
            Thinking => "Мысли (thinking)",
            Reasoning => "reasoning_effort",
        }
    }

    /// Подсказка-описание (показывается под полем при фокусе). `None` — без подсказки.
    fn description(self) -> Option<&'static str> {
        use SamplingParam::*;
        Some(match self {
            MinP => {
                "min-p: отсекает токены с вероятностью ниже доли от самой \
                     вероятной. 0 — выключено. Расширение llama.cpp."
            }
            TopNSigma => {
                "Отсев токенов дальше N стандартных отклонений (σ) от \
                          максимального логита. -1 — выключено. Расширение llama.cpp."
            }
            TypicalP => "Locally typical sampling. 1.0 — выключено. Расширение llama.cpp.",
            RepeatPenalty => {
                "Штраф за повтор токенов (отдельно от presence/frequency). \
                              1.0 — выключено. Расширение llama.cpp."
            }
            RepeatLastN => {
                "Сколько последних токенов учитывает repeat_penalty. \
                            0 — выключено, -1 — весь контекст."
            }
            DryMultiplier => {
                "DRY: сила штрафа за дословные повторы. 0 — выключено. \
                              Расширение llama.cpp."
            }
            DryBase => "DRY: основание роста штрафа с длиной повтора.",
            DryAllowedLength => "DRY: длина повтора, не штрафуемая (обычно 2).",
            DryPenaltyLastN => "DRY: глубина сканирования в токенах. -1 — весь контекст.",
            XtcProbability => {
                "XTC: вероятность срезать вероятные токены ради \
                               разнообразия. 0 — выключено. Расширение llama.cpp."
            }
            XtcThreshold => "XTC: порог вероятности для среза (обычно 0.1–0.2).",
            Mirostat => {
                "Mirostat: 0 — выкл, 1 или 2 — версия. Игнорирует top_k/top_p/\
                         typical_p. Расширение llama.cpp."
            }
            MirostatTau => "Mirostat: целевая энтропия (τ).",
            MirostatEta => "Mirostat: скорость адаптации (η).",
            Seed => "RNG-seed на запрос: -1 — случайный. Расширение llama.cpp.",
            _ => return None,
        })
    }
}

/// Идентификатор редактируемого поля (стабильный порядок = порядок в секции).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldId {
    // Селекторы подсекций «Ассистент»/«Имперсонация»
    ModelSub,
    SamplingSub,
    ProfileSub,
    // Модель/сервер — Ассистент (llama-server)
    XMode,
    XUrl,
    XBinary,
    XModel,
    XNgl,
    XCtx,
    XJinja,
    XNoMmap,
    XHost,
    XPort,
    // Модель/сервер — Имперсонация
    IxMode,
    IxUrl,
    IxBinary,
    IxModel,
    IxNgl,
    IxCtx,
    IxJinja,
    IxNoMmap,
    IxHost,
    IxPort,
    // Инференс
    MaxToolRounds,
    // Семплинг — поле по параметру; подсекция кодируется конструктором.
    // `S` — Ассистент (`default_sampling`), `IS` — Имперсонация
    // (`impersonation_sampling`).
    S(SamplingParam),
    IS(SamplingParam),
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
    /// Системное сообщение профиля для имперсонации.
    PImpSystem,
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
    /// Многострочный редактор (системное сообщение и приветствие): перенос длинных
    /// строк, ввод перевода строки по `Shift+Enter`, крупный попап. Прочие поля —
    /// однострочные.
    multiline: bool,
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
    /// Активные подсекции «Ассистент»/«Имперсонация» (Модель/Семплинг/Профили).
    model_sub: Subsection,
    sampling_sub: Subsection,
    profile_sub: Subsection,
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
            model_sub: Subsection::Assistant,
            sampling_sub: Subsection::Assistant,
            profile_sub: Subsection::Assistant,
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
        let mut rows = vec![row(
            FieldId::ModelSub,
            "Подсекция",
            FieldKind::Choice(self.model_sub.label()),
        )];
        match self.model_sub {
            Subsection::Assistant => {
                let x = &self.config.engine;
                rows.extend([
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
                    row(
                        FieldId::XJinja,
                        "Шаблон (--jinja)",
                        FieldKind::Toggle(x.jinja),
                    ),
                    row(
                        FieldId::XNoMmap,
                        "No-mmap (--no-mmap)",
                        FieldKind::Toggle(x.no_mmap),
                    ),
                    row(FieldId::XHost, "Host", FieldKind::Text(x.host.clone())),
                    row(FieldId::XPort, "Порт", FieldKind::Text(x.port.to_string())),
                ]);
            }
            Subsection::Impersonation => {
                let x = &self.config.impersonation_engine;
                rows.extend([
                    row(
                        FieldId::IxMode,
                        "Режим",
                        FieldKind::Choice(imp_mode_label(x.mode)),
                    ),
                    text_row(FieldId::IxUrl, "URL (external)", &x.url),
                    text_row(FieldId::IxBinary, "Бинарник llama-server", &x.binary),
                    text_row(FieldId::IxModel, "GGUF-модель (-m)", &x.model_path),
                    row(
                        FieldId::IxNgl,
                        "GPU-слои (-ngl)",
                        FieldKind::Text(x.gpu_layers.to_string()),
                    ),
                    row(
                        FieldId::IxCtx,
                        "Контекст (-c)",
                        FieldKind::Text(x.context_size.to_string()),
                    ),
                    row(
                        FieldId::IxJinja,
                        "Шаблон (--jinja)",
                        FieldKind::Toggle(x.jinja),
                    ),
                    row(
                        FieldId::IxNoMmap,
                        "No-mmap (--no-mmap)",
                        FieldKind::Toggle(x.no_mmap),
                    ),
                    row(FieldId::IxHost, "Host", FieldKind::Text(x.host.clone())),
                    row(FieldId::IxPort, "Порт", FieldKind::Text(x.port.to_string())),
                ]);
            }
        }
        rows
    }

    fn inference_fields(&self) -> Vec<FieldRow> {
        vec![row(
            FieldId::MaxToolRounds,
            "Лимит раундов инструментов",
            FieldKind::Text(self.config.max_tool_rounds.to_string()),
        )]
    }

    fn sampling_fields(&self) -> Vec<FieldRow> {
        let mut rows = vec![row(
            FieldId::SamplingSub,
            "Подсекция",
            FieldKind::Choice(self.sampling_sub.label()),
        )];
        let (s, imp) = match self.sampling_sub {
            Subsection::Assistant => (&self.config.default_sampling, false),
            Subsection::Impersonation => (&self.config.impersonation_sampling, true),
        };
        let mk = |p: SamplingParam| if imp { FieldId::IS(p) } else { FieldId::S(p) };
        rows.extend(SAMPLING_PARAMS.iter().map(|&p| sampling_row(mk(p), p, s)));
        rows
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
                FieldId::ProfileSub,
                "Подсекция",
                FieldKind::Choice(self.profile_sub.label()),
            ),
        ];
        match self.profile_sub {
            Subsection::Assistant => {
                rows.push(row(
                    FieldId::PSystem,
                    "Системное сообщение",
                    FieldKind::Text(p.default_system_message.clone()),
                ));
                rows.push(row(
                    FieldId::PGreeting,
                    "Приветствие",
                    FieldKind::Text(p.greeting.clone().unwrap_or_default()),
                ));
                for (idx, tool) in Self::tool_catalog().into_iter().enumerate() {
                    let on = p.enabled_tools.iter().any(|t| t == &tool);
                    rows.push(row(
                        FieldId::PTool(idx),
                        &format!("инструмент: {tool}"),
                        FieldKind::Toggle(on),
                    ));
                }
            }
            // В имперсонации инструментов нет (spec §11.8) — только сис. сообщение.
            Subsection::Impersonation => {
                rows.push(row(
                    FieldId::PImpSystem,
                    "Системное сообщение",
                    FieldKind::Text(p.impersonation_system_message.clone()),
                ));
            }
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
                            });
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

    /// Вставка из буфера обмена (bracketed paste): осмысленна только когда открыт
    /// текстовый редактор поля (например, путь к модели) — иначе no-op. См. spec §11.5.
    pub fn handle_paste(&mut self, text: &str) {
        if let Some(editor) = self.editor.as_mut() {
            editor.input.insert_str(text);
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
            FieldId::XNoMmap => self.config.engine.no_mmap = !self.config.engine.no_mmap,
            FieldId::IxJinja => {
                self.config.impersonation_engine.jinja = !self.config.impersonation_engine.jinja
            }
            FieldId::IxNoMmap => {
                self.config.impersonation_engine.no_mmap = !self.config.impersonation_engine.no_mmap
            }
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
            // Переключение подсекций «Ассистент»/«Имперсонация» — чисто навигация
            // (без сохранения). Сбрасываем курсор на селектор подсекции.
            FieldId::ModelSub => {
                self.model_sub = self.model_sub.toggled();
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
                self.config.engine.mode = cycle_mode(self.config.engine.mode);
                Some(self.save_config())
            }
            FieldId::IxMode => {
                self.config.impersonation_engine.mode =
                    cycle_imp_mode(self.config.impersonation_engine.mode, dir);
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
    fn cycle_sampling_field(&mut self, imp: bool, p: SamplingParam) -> Option<SettingsIntent> {
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
            // Имперсонация — сервер.
            FieldId::IxUrl => s.impersonation_engine.url = opt(trimmed),
            FieldId::IxBinary => s.impersonation_engine.binary = opt(trimmed),
            FieldId::IxModel => s.impersonation_engine.model_path = opt(trimmed),
            FieldId::IxHost => {
                if !trimmed.is_empty() {
                    s.impersonation_engine.host = trimmed.to_string();
                }
            }
            FieldId::IxNgl => {
                if let Ok(v) = trimmed.parse() {
                    s.impersonation_engine.gpu_layers = v;
                }
            }
            FieldId::IxCtx => {
                if let Ok(v) = trimmed.parse() {
                    s.impersonation_engine.context_size = v;
                }
            }
            FieldId::IxPort => {
                if let Ok(p) = trimmed.parse() {
                    s.impersonation_engine.port = p;
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
            FieldId::PName | FieldId::PSystem | FieldId::PGreeting | FieldId::PImpSystem => {
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
            FieldId::PImpSystem => p.impersonation_system_message = text.to_string(),
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
                impersonation_system_message: Some(p.impersonation_system_message.clone()),
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
        let palette = Palette::for_theme(self.config.interface.theme);
        let area = frame.area();
        if let Some(editor) = self.editor.as_mut() {
            // Системное сообщение/приветствие — крупный многострочный попап с
            // переносом; прочие поля — компактная однострочная полоса.
            let (popup, title) = if editor.multiline {
                (
                    centered_rect(80, 40, multiline_popup_height(area), area),
                    "правка · Shift+Enter перенос · Enter ок · Esc отмена",
                )
            } else {
                (
                    centered_rect(60, 30, 3, area),
                    "правка · Enter ок · Esc отмена",
                )
            };
            frame.render_widget(Clear, popup);
            editor
                .input
                .render(frame, popup, title, true, &palette, false);
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
        // Подсказка-описание сфокусированного поля (если оно есть) — отдельной
        // строкой внизу секции. Резервируем место только когда описание есть,
        // чтобы прочие секции выглядели как раньше.
        let description = focused
            .then(|| {
                fields
                    .get(self.field_idx)
                    .and_then(|f| field_description(f.id))
            })
            .flatten();
        let [list_area, desc_area] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(if description.is_some() { 4 } else { 0 }),
        ])
        .areas(area);
        // Колонку со значениями (в т.ч. чекбоксы [x]) выравниваем по самой длинной
        // подписи — иначе при разной длине имён инструментов [x] «гуляют». Минимум 28,
        // чтобы короткие секции выглядели как раньше.
        let label_col = fields
            .iter()
            .map(|f| label_width(&f.label))
            .max()
            .unwrap_or(0)
            .max(28);
        let items: Vec<ListItem> = fields
            .iter()
            .map(|f| ListItem::new(render_field_line(f, label_col)))
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
        frame.render_stateful_widget(list, list_area, &mut state);

        if let Some(text) = description {
            let para = Paragraph::new(text)
                .block(Block::default().borders(Borders::TOP))
                .style(Style::new().dim())
                .wrap(Wrap { trim: true });
            frame.render_widget(para, desc_area);
        }
    }
}

// ---------- свободные функции ----------

/// Человекопонятное описание поля для подсказки внизу секции (`None` — без подсказки).
fn field_description(id: FieldId) -> Option<&'static str> {
    match id {
        FieldId::XNgl => Some(
            "Сколько слоёв модели выгрузить на видеокарту (GPU). Больше слоёв — \
             быстрее, но нужна видеопамять; 0 — считать только на процессоре, \
             99 — вся модель на GPU.",
        ),
        FieldId::XJinja => Some(
            "Использовать встроенный chat-шаблон модели (Jinja). Нужен для \
             правильного формата сообщений и вызова инструментов — обычно держат включённым.",
        ),
        FieldId::XNoMmap | FieldId::IxNoMmap => Some(
            "Грузить веса модели целиком в оперативную память вместо отображения \
             файла с диска (mmap). Помогает на сетевых и медленных дисках, но требует \
             больше свободной RAM.",
        ),
        FieldId::IxMode => Some(
            "shared — использовать тот же сервер, что и для ассистента (с семплингом \
             имперсонации); managed — поднять отдельный llama-server; external — \
             подключиться к отдельному удалённому серверу.",
        ),
        FieldId::ModelSub | FieldId::SamplingSub | FieldId::ProfileSub => Some(
            "Переключение между настройками ассистента и имперсонации (написание \
             сообщения от лица пользователя, Ctrl+U). ←/→ или Enter.",
        ),
        FieldId::IxJinja => Some(
            "Использовать встроенный chat-шаблон модели (Jinja) для сервера \
             имперсонации.",
        ),
        FieldId::IxNgl => Some(
            "Сколько слоёв модели имперсонации выгрузить на видеокарту (GPU). \
             0 — только процессор, 99 — вся модель на GPU.",
        ),
        // Описания параметров семплинга (одинаковые для обеих подсекций).
        FieldId::S(p) | FieldId::IS(p) => p.description(),
        _ => None,
    }
}

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

/// Строка поля семплинга по параметру: числовые — текст (`num_row`),
/// `Thinking`/`Reasoning` — циклический выбор.
fn sampling_row(id: FieldId, p: SamplingParam, s: &SamplingConfig) -> FieldRow {
    use SamplingParam::*;
    let label = p.label();
    match p {
        Temp => num_row(id, label, s.temperature),
        TopK => num_row(id, label, s.top_k),
        TopP => num_row(id, label, s.top_p),
        MinP => num_row(id, label, s.min_p),
        TopNSigma => num_row(id, label, s.top_n_sigma),
        TypicalP => num_row(id, label, s.typical_p),
        FreqPen => num_row(id, label, s.frequency_penalty),
        PresPen => num_row(id, label, s.presence_penalty),
        RepeatPenalty => num_row(id, label, s.repeat_penalty),
        RepeatLastN => num_row(id, label, s.repeat_last_n),
        DryMultiplier => num_row(id, label, s.dry_multiplier),
        DryBase => num_row(id, label, s.dry_base),
        DryAllowedLength => num_row(id, label, s.dry_allowed_length),
        DryPenaltyLastN => num_row(id, label, s.dry_penalty_last_n),
        XtcProbability => num_row(id, label, s.xtc_probability),
        XtcThreshold => num_row(id, label, s.xtc_threshold),
        Mirostat => num_row(id, label, s.mirostat),
        MirostatTau => num_row(id, label, s.mirostat_tau),
        MirostatEta => num_row(id, label, s.mirostat_eta),
        MaxTokens => num_row(id, label, s.max_tokens),
        Seed => num_row(id, label, s.seed),
        Thinking => row(id, label, FieldKind::Choice(opt_bool_label(s.thinking))),
        Reasoning => row(
            id,
            label,
            FieldKind::Choice(reasoning_label(s.reasoning_effort)),
        ),
    }
}

/// Применяет текст редактора к числовому параметру семплинга. `Thinking`/`Reasoning`
/// — Choice-поля (редактируются ←/→), текстом не правятся.
fn apply_sampling_text(s: &mut SamplingConfig, p: SamplingParam, trimmed: &str) {
    use SamplingParam::*;
    match p {
        Temp => s.temperature = parse_opt_f32(trimmed),
        TopK => s.top_k = parse_opt(trimmed),
        TopP => s.top_p = parse_opt_f32(trimmed),
        MinP => s.min_p = parse_opt_f32(trimmed),
        TopNSigma => s.top_n_sigma = parse_opt_f32(trimmed),
        TypicalP => s.typical_p = parse_opt_f32(trimmed),
        FreqPen => s.frequency_penalty = parse_opt_f32(trimmed),
        PresPen => s.presence_penalty = parse_opt_f32(trimmed),
        RepeatPenalty => s.repeat_penalty = parse_opt_f32(trimmed),
        RepeatLastN => s.repeat_last_n = parse_opt(trimmed),
        DryMultiplier => s.dry_multiplier = parse_opt_f32(trimmed),
        DryBase => s.dry_base = parse_opt_f32(trimmed),
        DryAllowedLength => s.dry_allowed_length = parse_opt(trimmed),
        DryPenaltyLastN => s.dry_penalty_last_n = parse_opt(trimmed),
        XtcProbability => s.xtc_probability = parse_opt_f32(trimmed),
        XtcThreshold => s.xtc_threshold = parse_opt_f32(trimmed),
        Mirostat => s.mirostat = parse_opt(trimmed),
        MirostatTau => s.mirostat_tau = parse_opt_f32(trimmed),
        MirostatEta => s.mirostat_eta = parse_opt_f32(trimmed),
        MaxTokens => s.max_tokens = parse_opt(trimmed),
        Seed => s.seed = parse_opt(trimmed),
        Thinking | Reasoning => {}
    }
}

/// Ширина подписи в терминальных колонках (кириллица/латиница = 1, CJK/эмодзи = 2).
fn label_width(label: &str) -> usize {
    crate::shared::wrap::display_width(&label.chars().collect::<Vec<_>>())
}

fn render_field_line(f: &FieldRow, label_col: usize) -> Line<'static> {
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
    // Дополняем подпись пробелами до ширины колонки по реальной ширине в колонках
    // (Rust `{:<N}` считает символы, а не колонки — для CJK/эмодзи это разъезжается).
    let pad = label_col.saturating_sub(label_width(&f.label));
    Line::from(format!("{}{} {}", f.label, " ".repeat(pad), value))
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

fn imp_mode_label(m: ImpersonationMode) -> String {
    match m {
        ImpersonationMode::Shared => "shared".into(),
        ImpersonationMode::Managed => "managed".into(),
        ImpersonationMode::External => "external".into(),
    }
}

/// Циклически меняет режим имперсонации (три значения, с учётом направления).
fn cycle_imp_mode(m: ImpersonationMode, dir: i32) -> ImpersonationMode {
    use ImpersonationMode::*;
    let order = [Shared, Managed, External];
    let idx = order.iter().position(|x| *x == m).unwrap_or(0) as i32;
    let n = order.len() as i32;
    order[(((idx + dir) % n + n) % n) as usize]
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

/// Высота крупного попапа редактора системного сообщения: ~60% высоты экрана,
/// но не меньше 8 строк и не выше самого экрана.
fn multiline_popup_height(area: Rect) -> u16 {
    (area.height.saturating_mul(60) / 100)
        .max(8)
        .min(area.height)
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
        s.handle_key(key(KeyCode::Enter)); // фокус на поля (ModelSub)
        s.handle_key(key(KeyCode::Down)); // XMode (режим, Choice)
        let intent = s.handle_key(key(KeyCode::Right));
        match intent {
            Some(SettingsIntent::SaveConfig(c)) => assert_eq!(c.engine.mode, ServerMode::External),
            other => panic!("ожидался SaveConfig, получено {other:?}"),
        }
    }

    #[test]
    fn model_subsection_switches_to_impersonation_fields() {
        let mut s = screen();
        s.handle_key(key(KeyCode::Enter)); // фокус на поля (ModelSub)
        // → переключает подсекцию на «Имперсонация» (без сохранения).
        assert_eq!(s.handle_key(key(KeyCode::Right)), None);
        assert_eq!(s.model_sub, Subsection::Impersonation);
        // Первое поле подсекции — режим имперсонации (3 значения).
        s.handle_key(key(KeyCode::Down)); // IxMode
        // Цикл shared → managed.
        let intent = s.handle_key(key(KeyCode::Right));
        match intent {
            Some(SettingsIntent::SaveConfig(c)) => {
                assert_eq!(c.impersonation_engine.mode, ImpersonationMode::Managed)
            }
            other => panic!("ожидался SaveConfig, получено {other:?}"),
        }
    }

    #[test]
    fn impersonation_profile_subsection_has_no_tools() {
        let mut s = screen();
        for _ in 0..3 {
            s.handle_key(key(KeyCode::Tab)); // → Profiles
        }
        s.handle_key(key(KeyCode::Enter)); // фокус на поля; PSelect
        s.handle_key(key(KeyCode::Down)); // PName
        s.handle_key(key(KeyCode::Down)); // ProfileSub
        s.handle_key(key(KeyCode::Right)); // → Имперсонация
        assert_eq!(s.profile_sub, Subsection::Impersonation);
        let fields = s.fields();
        assert!(
            !fields.iter().any(|f| matches!(f.id, FieldId::PTool(_))),
            "в подсекции имперсонации нет тумблеров инструментов"
        );
        assert!(fields.iter().any(|f| f.id == FieldId::PImpSystem));
    }

    #[test]
    fn editing_model_commits_text() {
        let mut s = screen();
        s.handle_key(key(KeyCode::Enter)); // фокус на поля (ModelSub)
        // ModelSub → XMode → XUrl → XBinary → XModel.
        for _ in 0..4 {
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
        s.handle_key(key(KeyCode::Enter)); // ModelSub
        s.handle_key(key(KeyCode::Down)); // XMode
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
        // Перейти к первому тумблеру (после PSelect/PName/ProfileSub/PSystem/PGreeting).
        for _ in 0..5 {
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
    fn system_message_editor_is_multiline_and_keeps_newlines() {
        let mut s = screen();
        for _ in 0..3 {
            s.handle_key(key(KeyCode::Tab)); // → Profiles
        }
        s.handle_key(key(KeyCode::Enter)); // фокус на поля; PSelect
        s.handle_key(key(KeyCode::Down)); // PName
        s.handle_key(key(KeyCode::Down)); // ProfileSub
        s.handle_key(key(KeyCode::Down)); // PSystem
        s.handle_key(key(KeyCode::Enter)); // открыть редактор
        let editor = s.editor.as_ref().expect("редактор открыт");
        assert!(
            editor.multiline,
            "системное сообщение редактируется многострочно"
        );
        // Shift+Enter вставляет перевод строки, а не коммитит.
        s.handle_key(key(KeyCode::Char('A')));
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        s.handle_key(key(KeyCode::Char('B')));
        assert!(s.editor.is_some(), "Shift+Enter не закрывает редактор");
        let intent = s.handle_key(key(KeyCode::Enter)); // коммит
        match intent {
            Some(SettingsIntent::SaveProfile { edit, .. }) => {
                assert_eq!(edit.system_message.unwrap(), "Ты — ассистент.A\nB");
            }
            other => panic!("ожидался SaveProfile, получено {other:?}"),
        }
    }

    #[test]
    fn greeting_editor_is_multiline_and_keeps_newlines() {
        let mut s = screen();
        for _ in 0..3 {
            s.handle_key(key(KeyCode::Tab)); // → Profiles
        }
        s.handle_key(key(KeyCode::Enter)); // фокус на поля; PSelect
        s.handle_key(key(KeyCode::Down)); // PName
        s.handle_key(key(KeyCode::Down)); // ProfileSub
        s.handle_key(key(KeyCode::Down)); // PSystem
        s.handle_key(key(KeyCode::Down)); // PGreeting
        s.handle_key(key(KeyCode::Enter)); // открыть редактор
        let editor = s.editor.as_ref().expect("редактор открыт");
        assert!(editor.multiline, "приветствие редактируется многострочно");
        // Shift+Enter вставляет перевод строки, а не коммитит.
        s.handle_key(key(KeyCode::Char('A')));
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        s.handle_key(key(KeyCode::Char('B')));
        assert!(s.editor.is_some(), "Shift+Enter не закрывает редактор");
        let intent = s.handle_key(key(KeyCode::Enter)); // коммит
        match intent {
            Some(SettingsIntent::SaveProfile { edit, .. }) => {
                assert_eq!(edit.greeting.unwrap().as_deref(), Some("A\nB"));
            }
            other => panic!("ожидался SaveProfile, получено {other:?}"),
        }
    }

    #[test]
    fn other_fields_edit_single_line() {
        let mut s = screen();
        s.handle_key(key(KeyCode::Enter)); // поля (ModelSub)
        s.handle_key(key(KeyCode::Down)); // XMode
        s.handle_key(key(KeyCode::Down)); // XUrl
        s.handle_key(key(KeyCode::Enter)); // редактор
        let editor = s.editor.as_ref().expect("редактор открыт");
        assert!(!editor.multiline, "URL редактируется однострочно");
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

    #[test]
    fn flag_fields_have_descriptions() {
        // -ngl, --jinja и --no-mmap снабжены человекопонятной подсказкой; обычное поле — нет.
        assert!(field_description(FieldId::XNgl).is_some());
        assert!(field_description(FieldId::XJinja).is_some());
        assert!(field_description(FieldId::XNoMmap).is_some());
        assert!(field_description(FieldId::XPort).is_none());
    }

    #[test]
    fn render_with_focused_description_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut s = screen();
        s.handle_key(key(KeyCode::Enter)); // фокус на поля (XMode)
        // Дойти до тумблера --no-mmap (есть описание-подсказка внизу).
        while s.fields().get(s.field_idx).map(|f| f.id) != Some(FieldId::XNoMmap) {
            s.handle_key(key(KeyCode::Down));
        }
        for (w, h) in [(80u16, 24u16), (40, 12)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| s.render(f)).unwrap();
        }
    }

    #[test]
    fn editing_new_sampling_field_commits() {
        let mut s = screen();
        s.handle_key(key(KeyCode::Tab)); // Inference
        s.handle_key(key(KeyCode::Tab)); // Sampling
        s.handle_key(key(KeyCode::Enter)); // фокус на поля (SamplingSub)
        // Дойти до нового поля min_p (адресуется параметрически).
        while s.fields().get(s.field_idx).map(|f| f.id) != Some(FieldId::S(SamplingParam::MinP)) {
            s.handle_key(key(KeyCode::Down));
        }
        s.handle_key(key(KeyCode::Enter)); // открыть редактор min_p
        for c in "0.03".chars() {
            s.handle_key(key(KeyCode::Char(c)));
        }
        let intent = s.handle_key(key(KeyCode::Enter)); // коммит
        match intent {
            Some(SettingsIntent::SaveConfig(c)) => {
                assert_eq!(c.default_sampling.min_p, Some(0.03))
            }
            other => panic!("ожидался SaveConfig, получено {other:?}"),
        }
    }

    #[test]
    fn sampling_extensions_have_descriptions() {
        // Расширения llama.cpp снабжены подсказкой в обеих подсекциях.
        assert!(field_description(FieldId::S(SamplingParam::MinP)).is_some());
        assert!(field_description(FieldId::IS(SamplingParam::DryMultiplier)).is_some());
        // Базовые поля (температура) — без отдельной подсказки.
        assert!(field_description(FieldId::S(SamplingParam::Temp)).is_none());
    }
}

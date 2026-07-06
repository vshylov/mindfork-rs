//! Экран настроек (FSD "page"): секции, навигация и редактирование полей.
//! Вход по `Ctrl+P` из чата. См. spec §11.6.
//!
//! Как и [`super::chat::ChatScreen`], экран не знает про `app`/каналы: на правки
//! он возвращает [`SettingsIntent`], который `app` транслирует в `AppCommand`
//! (`UpdateConfig`/`UpdateProfile`/`CreateProfile`/`DeleteProfile`). Правки
//! применяются **сразу при коммите** поля (оркестратор — единственный писатель и
//! перезапускает сервер при смене модели). Работает на собственной рабочей копии
//! `AppConfig`/профилей, обновляемой теми же правками.

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use uuid::Uuid;

use crate::entities::profile::Profile;
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::features::profiles::ProfileEdit;
use crate::features::tools::all_tool_ids;
use crate::features::tools::meta::{self, ToolGate};
use crate::shared::config::{
    AppConfig, CloudProvider, CloudSettings, FlashAttn, ImpersonationMode, ManagedSettings,
    ServerMode, SpecType, Theme,
};
use crate::shared::keys;
use crate::shared::theme::Palette;
use crate::shared::ui::{dim_background, render_scrollbar};
use crate::widgets::input_box::InputBox;

/// Намерение, которое исполняет `app` (транслирует в `AppCommand`).
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsIntent {
    /// Закрыть экран настроек (вернуться в чат).
    Close,
    /// Выйти из приложения (`Ctrl+C`).
    Quit,
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
    Sampling,
    Tools,
    Memory,
    Profiles,
    Interface,
}

const SECTIONS: [Section; 6] = [
    Section::Model,
    Section::Sampling,
    Section::Tools,
    Section::Memory,
    Section::Profiles,
    Section::Interface,
];

/// Подсекция «Ассистент» / «Имперсонация» внутри секций Семплинг/Профили.
/// См. spec §11.8. Отображается как таб-стрип над полями секции.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Subsection {
    Assistant,
    Impersonation,
}

/// Подписи вкладок [`Subsection`] (порядок = дискриминанты).
const SUB_TABS: [&str; 2] = ["Ассистент", "Имперсонация"];

impl Subsection {
    fn label(self) -> String {
        SUB_TABS[self as usize].to_string()
    }

    fn toggled(self) -> Self {
        match self {
            Subsection::Assistant => Subsection::Impersonation,
            Subsection::Impersonation => Subsection::Assistant,
        }
    }

    /// Все варианты (для перечисления полей всех подсекций при поиске).
    const ALL: [Subsection; 2] = [Subsection::Assistant, Subsection::Impersonation];

    fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or(Subsection::Assistant)
    }
}

/// Подсекция секции «Модель/сервер»: три сервера приложения (зеркало чипов
/// статус-бара чат/имп/эмб) — ассистент, имперсонация, эмбеддинги. Отображается
/// как таб-стрип над полями. См. spec §11.6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelTab {
    Assistant,
    Impersonation,
    Embeddings,
}

/// Подписи вкладок [`ModelTab`] (порядок = дискриминанты).
const MODEL_TABS: [&str; 3] = ["Ассистент", "Имперсонация", "Эмбеддинги"];

impl ModelTab {
    fn label(self) -> String {
        MODEL_TABS[self as usize].to_string()
    }

    /// Все варианты (для перечисления полей всех подсекций при поиске).
    const ALL: [ModelTab; 3] = [
        ModelTab::Assistant,
        ModelTab::Impersonation,
        ModelTab::Embeddings,
    ];

    /// Циклический сдвиг вкладки (←/→ по таб-стрипу).
    fn cycle(self, dir: i32) -> Self {
        let idx = self as i32;
        let n = Self::ALL.len() as i32;
        Self::ALL[(((idx + dir) % n + n) % n) as usize]
    }

    fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or(ModelTab::Assistant)
    }
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Section::Model => "Модель/сервер",
            Section::Sampling => "Семплинг",
            Section::Tools => "Инструменты",
            Section::Memory => "Память",
            Section::Profiles => "Профили",
            Section::Interface => "Интерфейс",
        }
    }
}

/// Числовой вид редактируемого поля (для валидации ввода без закрытия редактора).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumKind {
    Int,
    Float,
}

/// Параметр семплинга. Адресует конкретное поле [`SamplingConfig`] внутри
/// подсекции; сама подсекция («Ассистент»/«Имперсонация») кодируется
/// конструктором [`FieldId::S`]/[`FieldId::IS`]. Числовые параметры
/// редактируются текстом, `Thinking`/`Reasoning` — циклическим выбором.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SamplingParam {
    Temp,
    DynatempRange,
    DynatempExp,
    TopK,
    TopP,
    MinP,
    TopNSigma,
    TypicalP,
    AdaptiveTarget,
    AdaptiveDecay,
    FreqPen,
    PresPen,
    RepeatPenalty,
    RepeatLastN,
    DryMultiplier,
    DryBase,
    DryAllowedLength,
    DryPenaltyLastN,
    DrySeqBreakers,
    XtcProbability,
    XtcThreshold,
    Mirostat,
    MirostatTau,
    MirostatEta,
    MaxTokens,
    Seed,
    Samplers,
    Thinking,
    Reasoning,
}

/// Порядок параметров семплинга в секции (стабильный = порядок отрисовки).
/// Сгруппирован по смыслу: параметры одной группы идут подряд, чтобы заголовок
/// группы ([`SamplingParam::group`]) в UI ставился один раз перед серией.
const SAMPLING_PARAMS: &[SamplingParam] = {
    use SamplingParam::*;
    &[
        // Основные
        Temp,
        TopK,
        TopP,
        MaxTokens,
        Seed,
        // Динамическая температура
        DynatempRange,
        DynatempExp,
        // Разнообразие
        MinP,
        TopNSigma,
        TypicalP,
        AdaptiveTarget,
        AdaptiveDecay,
        XtcProbability,
        XtcThreshold,
        // Штрафы за повтор
        FreqPen,
        PresPen,
        RepeatPenalty,
        RepeatLastN,
        // DRY (анти-повтор)
        DryMultiplier,
        DryBase,
        DryAllowedLength,
        DryPenaltyLastN,
        DrySeqBreakers,
        // Mirostat
        Mirostat,
        MirostatTau,
        MirostatEta,
        // Порядок семплеров
        Samplers,
        // Рассуждения
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
            DynatempRange => "dynatemp_range",
            DynatempExp => "dynatemp_exponent",
            TopK => "top_k",
            TopP => "top_p",
            MinP => "min_p",
            TopNSigma => "top_n_sigma",
            TypicalP => "typical_p",
            AdaptiveTarget => "adaptive_target",
            AdaptiveDecay => "adaptive_decay",
            FreqPen => "frequency_penalty",
            PresPen => "presence_penalty",
            RepeatPenalty => "repeat_penalty",
            RepeatLastN => "repeat_last_n",
            DryMultiplier => "dry_multiplier",
            DryBase => "dry_base",
            DryAllowedLength => "dry_allowed_length",
            DryPenaltyLastN => "dry_penalty_last_n",
            DrySeqBreakers => "dry_sequence_breakers",
            XtcProbability => "xtc_probability",
            XtcThreshold => "xtc_threshold",
            Mirostat => "mirostat",
            MirostatTau => "mirostat_tau",
            MirostatEta => "mirostat_eta",
            MaxTokens => "max_tokens",
            Seed => "seed",
            Samplers => "samplers",
            Thinking => "Мысли (thinking)",
            Reasoning => "reasoning_effort",
        }
    }

    /// Имя JSON-поля `SamplingConfig` (для сверки с набором, доступным провайдеру).
    fn field_name(self) -> &'static str {
        use SamplingParam::*;
        match self {
            Temp => "temperature",
            Thinking => "thinking",
            Reasoning => "reasoning_effort",
            // Остальные параметры подписаны именем своего JSON-поля.
            _ => self.label(),
        }
    }

    /// Числовой вид параметра для валидации редактора (`None` — не число: списки/
    /// выбор `Thinking`/`Reasoning`).
    fn num_kind(self) -> Option<NumKind> {
        use SamplingParam::*;
        match self {
            // Целочисленные.
            TopK | RepeatLastN | DryAllowedLength | DryPenaltyLastN | Mirostat | MaxTokens
            | Seed => Some(NumKind::Int),
            // Списки/выбор — не число.
            DrySeqBreakers | Samplers | Thinking | Reasoning => None,
            // Остальные — вещественные.
            _ => Some(NumKind::Float),
        }
    }

    /// Смысловая группа параметра (заголовок группы в секции «Семплинг»).
    fn group(self) -> &'static str {
        use SamplingParam::*;
        match self {
            Temp | TopK | TopP | MaxTokens | Seed => "Основные",
            DynatempRange | DynatempExp => "Динамическая температура",
            MinP | TopNSigma | TypicalP | AdaptiveTarget | AdaptiveDecay | XtcProbability
            | XtcThreshold => "Разнообразие",
            FreqPen | PresPen | RepeatPenalty | RepeatLastN => "Штрафы за повтор",
            DryMultiplier | DryBase | DryAllowedLength | DryPenaltyLastN | DrySeqBreakers => {
                "DRY (анти-повтор)"
            }
            Mirostat | MirostatTau | MirostatEta => "Mirostat",
            Samplers => "Порядок семплеров",
            Thinking | Reasoning => "Рассуждения",
        }
    }

    /// Подсказка-описание (показывается под полем при фокусе). `None` — без подсказки.
    fn description(self) -> Option<&'static str> {
        use SamplingParam::*;
        Some(match self {
            Temp => {
                "Температура: разброс при выборе токенов. Выше — разнообразнее и \
                 непредсказуемее, ниже — детерминированнее и точнее. 0 — почти \
                 жадный выбор."
            }
            TopK => {
                "top-k: выбирать только из K самых вероятных токенов. \
                 0 — выключено (без ограничения числа кандидатов)."
            }
            TopP => {
                "top-p (nucleus): выбор из наименьшего набора токенов, чья \
                 суммарная вероятность ≥ p. 1.0 — выключено."
            }
            FreqPen => {
                "Штраф за частоту: снижает вероятность токенов пропорционально \
                 тому, как часто они уже встречались (борьба с повторами). \
                 0 — выключено."
            }
            PresPen => {
                "Штраф за присутствие: снижает вероятность уже встречавшихся \
                 токенов (разово, без учёта частоты — подталкивает к новым темам). \
                 0 — выключено."
            }
            DynatempRange => {
                "Динамическая температура: ширина диапазона ± вокруг \
                 температуры, подстраиваемого по энтропии на каждом токене. \
                 0 — выключено. Расширение llama.cpp."
            }
            DynatempExp => "Динамическая температура: показатель кривой адаптации (обычно 1.0).",
            AdaptiveTarget => {
                "adaptive-p: целевая вероятность, около которой выбираются \
                 токены. Отрицательное — выключено. Экспериментально (llama.cpp)."
            }
            AdaptiveDecay => "adaptive-p: скорость адаптации цели (0..0.99; меньше — реактивнее).",
            DrySeqBreakers => {
                "DRY: брейкеры через запятую (сброс учёта повтора). Пусто — \
                 серверные по умолчанию. Эскейпы \\n \\t \\r поддержаны."
            }
            Samplers => {
                "Порядок семплеров через «;» (напр. penalties;dry;top_k;top_p;\
                 min_p;temperature). Пусто — порядок сервера. Не указанный \
                 семплер отключается."
            }
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
            MaxTokens => {
                "Максимум токенов в ответе. Пусто — без явного лимита \
                 (до EOS или конца контекста)."
            }
            Thinking => {
                "«Мысли» (chain-of-thought): включает рассуждения модели до ответа \
                 (для reasoning-моделей). Показываются отдельным сворачиваемым \
                 блоком (Ctrl+T)."
            }
            Reasoning => {
                "Усилие рассуждения для reasoning-моделей: low/medium/high. \
                 Выше — глубже размышляет перед ответом, но медленнее."
            }
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
    /// Имя облачной/мульти-модельной модели (`model_name`).
    XModelName,
    /// Имя env-переменной с API-ключом (облако).
    XApiKeyEnv,
    XNgl,
    XCtx,
    XFlashAttn,
    XJinja,
    XNoMmap,
    XSpecType,
    XDraftModel,
    XDraftNgl,
    XDraftNMax,
    XDraftNMin,
    XHost,
    XPort,
    // Модель/сервер — Имперсонация
    IxMode,
    IxUrl,
    IxBinary,
    IxModel,
    IxModelName,
    IxApiKeyEnv,
    IxNgl,
    IxCtx,
    IxFlashAttn,
    IxJinja,
    IxNoMmap,
    IxSpecType,
    IxDraftModel,
    IxDraftNgl,
    IxDraftNMax,
    IxDraftNMin,
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
    TWebFetch,
    TPython,
    TPythonPath,
    TFs,
    TFsRoot,
    TSubMaxTokens,
    TSubTimeout,
    EMode,
    EUrl,
    EBinary,
    EModel,
    EModelName,
    EApiKeyEnv,
    EPort,
    // RAG (чанкинг базы знаний)
    RagTarget,
    RagOverlap,
    RagMax,
    // Модель себя (нарратив, инъекция в промпт)
    SmMaxNarrative,
    SmNarrativeInPrompt,
    SmPromptCap,
    SmSummaryTarget,
    SmAutoReflect,
    SmProtocol,
    NotesAutoConsolidate,
    NotesRecallIncludesSelf,
    // Интерфейс
    ITheme,
    /// Режим совместимости со старыми терминалами (эмодзи → безопасные глифы).
    ICompat,
    ISpell,
    IDicts,
    /// Подтверждение перед `Ctrl+R`/`Ctrl+E` (необратимые операции).
    IConfirmKeys,
    /// Копировать «мысли» (CoT) при копировании переписки (`F5`).
    ICopyThoughts,
    /// Копировать параметры вызовов инструментов при копировании переписки.
    ICopyToolCalls,
    /// Копировать результаты вызовов инструментов при копировании переписки.
    ICopyToolResults,
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

/// Строка поля: идентификатор, подпись, текущее представление значения и
/// смысловая группа (для заголовка группы и выравнивания значений; `""` — вне
/// группы, без заголовка).
struct FieldRow {
    id: FieldId,
    label: String,
    kind: FieldKind,
    group: &'static str,
    /// Короткая инлайн-подсказка справа от значения (описание инструмента). `None` — нет.
    hint: Option<&'static str>,
    /// Значение и подсказку рисовать цветом предупреждения — инструмент включён в
    /// профиле, но выключен глобальным гейтом (недоступен модели).
    warn: bool,
}

/// Активный редактор текстового поля (попап).
struct Editor {
    field: FieldId,
    input: InputBox,
    /// Многострочный редактор (системное сообщение и приветствие): перенос длинных
    /// строк, ввод перевода строки по `Shift+Enter`, крупный попап. Прочие поля —
    /// однострочные.
    multiline: bool,
    /// Ошибка валидации (напр. «нужно число»): редактор не закрывается по `Enter`,
    /// подпись краснеет. `None` — ввод валиден.
    error: Option<&'static str>,
}

/// Фокус: левое меню секций или список полей справа.
#[derive(PartialEq)]
enum Focus {
    Menu,
    Fields,
}

/// Одна цель поиска по полям: координаты для прыжка + текст для показа/сопоставления.
struct SearchHit {
    section_idx: usize,
    /// Подсекция для прыжка (дискриминант; `None` — секция без подсекций).
    subsection: Option<usize>,
    /// Индекс поля в `*_fields()` соответствующей подсекции.
    field_idx: usize,
    /// «Секция › Группа › Подпись» для показа.
    crumb: String,
    /// Текущее значение поля (усекается при показе).
    value: String,
    /// Ловушка совпадения (lowercase): секция + группа + подпись + описание + hint.
    haystack: String,
}

/// Оверлей поиска по полям (`/`): строка запроса + плоская отфильтрованная выдача.
struct SearchState {
    input: InputBox,
    /// Полный индекс полей всех секций/подсекций (строится при открытии).
    all: Vec<SearchHit>,
    /// Индексы в `all`, прошедшие фильтр запроса.
    results: Vec<usize>,
    selected: usize,
}

/// Попап выбора значения Choice-поля (Enter): список вариантов с отметкой текущего.
struct ChoiceState {
    field: FieldId,
    options: Vec<String>,
    selected: usize,
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
    /// Активные подсекции. Модель — три вкладки (Ассистент/Имперсонация/Эмбеддинги);
    /// Семплинг/Профили — две (Ассистент/Имперсонация).
    model_sub: ModelTab,
    sampling_sub: Subsection,
    profile_sub: Subsection,
    editor: Option<Editor>,
    /// Оверлей поиска по полям (`/`); `None` — закрыт.
    search: Option<SearchState>,
    /// Попап выбора значения Choice-поля (Enter); `None` — закрыт.
    choice: Option<ChoiceState>,
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
            model_sub: ModelTab::Assistant,
            sampling_sub: Subsection::Assistant,
            profile_sub: Subsection::Assistant,
            editor: None,
            search: None,
            choice: None,
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

    /// Каталог всех известных инструментов (для тумблеров в профиле) — включая
    /// опциональные управляющие (по умолчанию выключенные). См. spec §9.3.
    fn tool_catalog() -> Vec<String> {
        all_tool_ids()
    }

    // ---------- построение полей текущей секции ----------

    fn fields(&self) -> Vec<FieldRow> {
        match self.section() {
            Section::Model => self.model_fields(),
            Section::Sampling => self.sampling_fields(),
            Section::Tools => self.tool_fields(),
            Section::Memory => self.memory_fields(),
            Section::Profiles => self.profile_fields(),
            Section::Interface => self.interface_fields(),
        }
    }

    fn model_fields(&self) -> Vec<FieldRow> {
        self.model_fields_for(self.model_sub)
    }

    /// Поля секции «Модель» для заданной подсекции (для перечисления при поиске —
    /// [`SettingsScreen::model_fields`] строит их для активной подсекции).
    fn model_fields_for(&self, model_sub: ModelTab) -> Vec<FieldRow> {
        // Селектор подсекции (таб-стрип) — всегда поле 0; в списке он не рисуется.
        let sub = row(
            FieldId::ModelSub,
            "Подсекция",
            FieldKind::Choice(model_sub.label()),
        );
        match model_sub {
            ModelTab::Assistant => {
                let x = &self.config.engine;
                let mut rows = vec![
                    sub,
                    row(
                        FieldId::XMode,
                        "Режим",
                        FieldKind::Choice(mode_label(x.mode)),
                    ),
                ];
                // Видимость полей зависит от режима (ADR 0004): для облака показываем
                // лишь модель/ключ/опц. base URL, для managed — параметры llama-server.
                match x.mode {
                    ServerMode::Managed => {
                        rows.extend(managed_rows(&x.managed, ASSISTANT_MANAGED_IDS))
                    }
                    ServerMode::External => rows.extend(grouped(
                        "Сервер",
                        vec![
                            text_row(FieldId::XUrl, "URL (external)", &x.external.url),
                            text_row(FieldId::XModelName, "Модель (опц.)", &x.external.model_name),
                        ],
                    )),
                    ServerMode::OpenAi | ServerMode::Gemini | ServerMode::Claude => {
                        rows.extend(grouped(
                            "Провайдер",
                            cloud_rows(
                                x.cloud(),
                                FieldId::XModelName,
                                FieldId::XApiKeyEnv,
                                FieldId::XUrl,
                            ),
                        ))
                    }
                }
                rows
            }
            ModelTab::Impersonation => {
                let x = &self.config.impersonation_engine;
                let mut rows = vec![
                    sub,
                    row(
                        FieldId::IxMode,
                        "Режим",
                        FieldKind::Choice(imp_mode_label(x.mode)),
                    ),
                ];
                match x.mode {
                    // Shared переиспользует движок ассистента — собственных полей нет.
                    ImpersonationMode::Shared => {}
                    ImpersonationMode::Managed => {
                        rows.extend(managed_rows(&x.managed, IMP_MANAGED_IDS))
                    }
                    ImpersonationMode::External => rows.extend(grouped(
                        "Сервер",
                        vec![
                            text_row(FieldId::IxUrl, "URL (external)", &x.external.url),
                            text_row(
                                FieldId::IxModelName,
                                "Модель (опц.)",
                                &x.external.model_name,
                            ),
                        ],
                    )),
                    ImpersonationMode::OpenAi
                    | ImpersonationMode::Gemini
                    | ImpersonationMode::Claude => rows.extend(grouped(
                        "Провайдер",
                        cloud_rows(
                            x.cloud(),
                            FieldId::IxModelName,
                            FieldId::IxApiKeyEnv,
                            FieldId::IxUrl,
                        ),
                    )),
                }
                rows
            }
            ModelTab::Embeddings => {
                // Эмбеддинги — выделенный сервер (память/RAG). Поля по режиму
                // (managed → llama-server; external/облако → URL/модель/ключ).
                let e = &self.config.embed;
                let mut rows = vec![
                    sub,
                    row(
                        FieldId::EMode,
                        "Режим",
                        FieldKind::Choice(mode_label(e.mode)),
                    ),
                ];
                match e.mode {
                    ServerMode::Managed => rows.extend(grouped(
                        "Сервер",
                        vec![
                            text_row(FieldId::EBinary, "Бинарник llama-server", &e.managed.binary),
                            text_row(FieldId::EModel, "GGUF-модель (-m)", &e.managed.model_path),
                            num_field(FieldId::EPort, "Порт", e.managed.port),
                        ],
                    )),
                    ServerMode::External => rows.extend(grouped(
                        "Сервер",
                        vec![
                            text_row(FieldId::EUrl, "URL (external)", &e.external.url),
                            text_row(FieldId::EModelName, "Модель (опц.)", &e.external.model_name),
                        ],
                    )),
                    // Claude поля показывает, но Anthropic не умеет embeddings —
                    // супервайзер вернёт «недоступно» (RAG отключится). ADR 0004.
                    ServerMode::OpenAi | ServerMode::Gemini | ServerMode::Claude => {
                        let none = CloudSettings::default();
                        let c = e.cloud().unwrap_or(&none);
                        rows.extend(grouped(
                            "Провайдер",
                            vec![
                                text_row(FieldId::EModelName, "Модель", &c.model_name),
                                text_row(FieldId::EApiKeyEnv, "API-ключ (env)", &c.api_key_env),
                                text_row(FieldId::EUrl, "Base URL (опц.)", &c.url),
                            ],
                        ))
                    }
                }
                rows
            }
        }
    }

    fn sampling_fields(&self) -> Vec<FieldRow> {
        self.sampling_fields_for(self.sampling_sub)
    }

    /// Поля секции «Семплинг» для заданной подсекции (для перечисления при поиске).
    fn sampling_fields_for(&self, sampling_sub: Subsection) -> Vec<FieldRow> {
        let mut rows = vec![row(
            FieldId::SamplingSub,
            "Подсекция",
            FieldKind::Choice(sampling_sub.label()),
        )];
        let (s, imp) = match sampling_sub {
            Subsection::Assistant => (&self.config.default_sampling, false),
            Subsection::Impersonation => (&self.config.impersonation_sampling, true),
        };
        let mk = |p: SamplingParam| if imp { FieldId::IS(p) } else { FieldId::S(p) };
        // В облачном режиме показываем только параметры, которые провайдер реально
        // принимает (расширения llama.cpp и reasoning-поля скрыты — ADR 0004).
        // Значения скрытых параметров сохраняются и заработают на локальной модели.
        let provider = self.sampling_cloud_provider(imp);
        rows.extend(
            SAMPLING_PARAMS
                .iter()
                .filter(|&&p| match provider {
                    Some(provider) => cloud_supported_param(provider, p),
                    None => true,
                })
                .map(|&p| {
                    let mut r = sampling_row(mk(p), p, s);
                    r.group = p.group();
                    r
                }),
        );
        rows
    }

    fn tool_fields(&self) -> Vec<FieldRow> {
        let t = &self.config.tools;
        let mut rows = grouped(
            "Агентный цикл",
            vec![
                row(
                    FieldId::MaxToolRounds,
                    "Лимит раундов инструментов",
                    FieldKind::Text(self.config.max_tool_rounds.to_string()),
                ),
                row(
                    FieldId::TSubMaxTokens,
                    "Субагент: лимит токенов",
                    FieldKind::Text(t.subagent_max_tokens.to_string()),
                ),
                row(
                    FieldId::TSubTimeout,
                    "Субагент: таймаут (с)",
                    FieldKind::Text(t.subagent_timeout_secs.to_string()),
                ),
            ],
        );
        rows.extend(grouped(
            "Веб-поиск",
            vec![
                row(FieldId::TWeb, "Web-поиск", FieldKind::Toggle(t.web_enabled)),
                row(
                    FieldId::TWebFetch,
                    "Загрузка страниц",
                    FieldKind::Toggle(t.web_fetch_content),
                ),
            ],
        ));
        rows.extend(grouped(
            "Python",
            vec![
                row(
                    FieldId::TPython,
                    "Python-исполнение",
                    FieldKind::Toggle(t.python_enabled),
                ),
                text_row(
                    FieldId::TPythonPath,
                    "Путь к интерпретатору",
                    &t.python_path,
                ),
            ],
        ));
        rows.extend(grouped(
            "Файлы",
            vec![
                row(
                    FieldId::TFs,
                    "Доступ к файлам",
                    FieldKind::Toggle(t.fs_enabled),
                ),
                text_row(FieldId::TFsRoot, "Каталог-песочница", &t.fs_root),
            ],
        ));
        rows
    }

    /// Секция «Память»: чанкинг базы знаний (RAG), заметки, «модель себя».
    fn memory_fields(&self) -> Vec<FieldRow> {
        let mut rows = grouped(
            "База знаний (RAG)",
            vec![
                row(
                    FieldId::RagTarget,
                    "Размер чанка (симв.)",
                    FieldKind::Text(self.config.rag.chunk_target_chars.to_string()),
                ),
                row(
                    FieldId::RagOverlap,
                    "Перекрытие (симв.)",
                    FieldKind::Text(self.config.rag.chunk_overlap_chars.to_string()),
                ),
                row(
                    FieldId::RagMax,
                    "Потолок чанка (симв.)",
                    FieldKind::Text(self.config.rag.chunk_max_chars.to_string()),
                ),
            ],
        );
        rows.extend(grouped(
            "Заметки",
            vec![
                row(
                    FieldId::NotesAutoConsolidate,
                    "Авто-консолидация (кажд. N)",
                    FieldKind::Text(self.config.notes.auto_consolidate_every.to_string()),
                ),
                row(
                    FieldId::NotesRecallIncludesSelf,
                    "Наблюдения «о себе» в note_recall",
                    FieldKind::Toggle(self.config.notes.recall_includes_self),
                ),
            ],
        ));
        rows.extend(grouped(
            "Модель себя",
            vec![
                row(
                    FieldId::SmMaxNarrative,
                    "Хранить инсайтов",
                    FieldKind::Text(self.config.self_model.max_narrative.to_string()),
                ),
                row(
                    FieldId::SmNarrativeInPrompt,
                    "Инсайтов в промпт",
                    FieldKind::Text(self.config.self_model.narrative_in_prompt.to_string()),
                ),
                row(
                    FieldId::SmPromptCap,
                    "Лимит инъекции (симв.)",
                    FieldKind::Text(self.config.self_model.prompt_cap.to_string()),
                ),
                row(
                    FieldId::SmSummaryTarget,
                    "Ориентир описания (симв.)",
                    FieldKind::Text(self.config.self_model.summary_target_chars.to_string()),
                ),
                row(
                    FieldId::SmAutoReflect,
                    "Авто-рефлексия (кажд. N)",
                    FieldKind::Text(self.config.self_model.auto_reflect_every.to_string()),
                ),
                row(
                    FieldId::SmProtocol,
                    "Протокол ведения",
                    FieldKind::Toggle(self.config.self_model.maintenance_protocol),
                ),
            ],
        ));
        rows
    }

    fn interface_fields(&self) -> Vec<FieldRow> {
        let i = &self.config.interface;
        let mut rows = grouped(
            "Оформление",
            vec![
                row(
                    FieldId::ITheme,
                    "Тема",
                    FieldKind::Choice(theme_label(i.theme)),
                ),
                row(
                    FieldId::ICompat,
                    "Совместимость со старым терминалом",
                    FieldKind::Toggle(i.terminal_compat),
                ),
            ],
        );
        rows.extend(grouped(
            "Орфография",
            vec![
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
            ],
        ));
        rows.extend(grouped(
            "Поведение",
            vec![row(
                FieldId::IConfirmKeys,
                "Подтверждать Ctrl+R / Ctrl+E",
                FieldKind::Toggle(i.confirm_destructive_keys),
            )],
        ));
        rows.extend(grouped(
            "Копирование переписки (F5)",
            vec![
                row(
                    FieldId::ICopyThoughts,
                    "Копировать с «мыслями»",
                    FieldKind::Toggle(self.config.copy.copy_thoughts),
                ),
                row(
                    FieldId::ICopyToolCalls,
                    "Копировать с параметрами инструментов",
                    FieldKind::Toggle(self.config.copy.copy_tool_calls),
                ),
                row(
                    FieldId::ICopyToolResults,
                    "Копировать с ответами инструментов",
                    FieldKind::Toggle(self.config.copy.copy_tool_results),
                ),
            ],
        ));
        rows
    }

    fn profile_fields(&self) -> Vec<FieldRow> {
        self.profile_fields_for(self.profile_sub)
    }

    /// Поля секции «Профили» для заданной подсекции (для перечисления при поиске).
    fn profile_fields_for(&self, profile_sub: Subsection) -> Vec<FieldRow> {
        let Some(p) = self.profiles.get(self.profile_idx) else {
            return vec![row(
                FieldId::PSelect,
                "Профиль",
                FieldKind::Choice("(нет профилей)".to_string()),
            )];
        };
        // ProfileSub — селектор подсекции (таб-стрип, поле 0, в списке не рисуется);
        // выбор профиля и имя — секционные (общие для обеих подсекций).
        let mut rows = vec![
            row(
                FieldId::ProfileSub,
                "Подсекция",
                FieldKind::Choice(profile_sub.label()),
            ),
            row(
                FieldId::PSelect,
                "Профиль",
                FieldKind::Choice(p.name.clone()),
            ),
            row(FieldId::PName, "Имя", FieldKind::Text(p.name.clone())),
        ];
        match profile_sub {
            Subsection::Assistant => {
                rows.extend(grouped(
                    "Персона",
                    vec![
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
                    ],
                ));
                // Тумблеры инструментов: раскладываем по смысловым группам
                // (`meta::tool_group`), с коротким описанием и честным гейтом.
                // Индекс `PTool` — позиция в `tool_catalog()` (источник истины для
                // `toggle_profile_tool`); порядок ПОКАЗА группируем стабильной
                // сортировкой, не трогая индексы.
                let mut indexed: Vec<(usize, String)> =
                    Self::tool_catalog().into_iter().enumerate().collect();
                indexed.sort_by_key(|(_, id)| {
                    meta::TOOL_GROUPS
                        .iter()
                        .position(|g| *g == meta::tool_group(id))
                        .unwrap_or(usize::MAX)
                });
                for (idx, tool) in indexed {
                    let on = p.enabled_tools.iter().any(|t| t == &tool);
                    let gate = meta::tool_gate(&tool);
                    let gated_off = on && gate.is_some_and(|g| self.gate_disabled(g));
                    let mut r = row(FieldId::PTool(idx), &tool, FieldKind::Toggle(on));
                    r.group = meta::tool_group(&tool);
                    r.warn = gated_off;
                    r.hint = if gated_off {
                        gate.map(gate_hint)
                    } else {
                        let d = meta::tool_description(&tool);
                        (!d.is_empty()).then_some(d)
                    };
                    rows.push(r);
                }
            }
            // В имперсонации инструментов нет (spec §11.8) — только сис. сообщение.
            Subsection::Impersonation => {
                rows.extend(grouped(
                    "Персона",
                    vec![row(
                        FieldId::PImpSystem,
                        "Системное сообщение",
                        FieldKind::Text(p.impersonation_system_message.clone()),
                    )],
                ));
            }
        }
        rows
    }

    /// Выключен ли глобальный гейт инструмента (тогда инструмент недоступен модели,
    /// даже если включён в профиле).
    fn gate_disabled(&self, gate: ToolGate) -> bool {
        match gate {
            ToolGate::Web => !self.config.tools.web_enabled,
            ToolGate::Python => !self.config.tools.python_enabled,
            ToolGate::Fs => !self.config.tools.fs_enabled,
        }
    }

    /// Облачный провайдер сэмплинга подсекции (`None` — локальный движок). Для
    /// имперсонации в режиме `shared` эффективный провайдер — движок ассистента.
    fn sampling_cloud_provider(&self, imp: bool) -> Option<CloudProvider> {
        if imp {
            match self.config.impersonation_engine.mode {
                ImpersonationMode::Shared => self.config.engine.mode.cloud_provider(),
                m => m.cloud_provider(),
            }
        } else {
            self.config.engine.mode.cloud_provider()
        }
    }

    // ---------- попап выбора Choice-поля / сброс к дефолту ----------

    /// Список вариантов Choice-поля + индекс текущего (`None` — поле не Choice).
    fn choice_menu(&self, id: FieldId) -> Option<(Vec<String>, usize)> {
        let mode_menu = |m: ServerMode| index_menu(&SERVER_MODES, m, mode_label);
        match id {
            FieldId::XMode => Some(mode_menu(self.config.engine.mode)),
            FieldId::EMode => Some(mode_menu(self.config.embed.mode)),
            FieldId::IxMode => Some(index_menu(
                &IMP_MODES,
                self.config.impersonation_engine.mode,
                imp_mode_label,
            )),
            FieldId::XFlashAttn => Some(flash_menu(self.config.engine.managed.flash_attn)),
            FieldId::IxFlashAttn => Some(flash_menu(
                self.config.impersonation_engine.managed.flash_attn,
            )),
            FieldId::XSpecType => Some(spec_menu(self.config.engine.managed.spec_type)),
            FieldId::IxSpecType => Some(spec_menu(
                self.config.impersonation_engine.managed.spec_type,
            )),
            FieldId::ITheme => Some(index_menu(
                &THEMES,
                self.config.interface.theme,
                theme_label,
            )),
            FieldId::S(p @ (SamplingParam::Thinking | SamplingParam::Reasoning)) => {
                Some(sampling_choice_menu(&self.config.default_sampling, p))
            }
            FieldId::IS(p @ (SamplingParam::Thinking | SamplingParam::Reasoning)) => {
                Some(sampling_choice_menu(&self.config.impersonation_sampling, p))
            }
            FieldId::PSelect => {
                let opts: Vec<String> = self.profiles.iter().map(|p| p.name.clone()).collect();
                (!opts.is_empty()).then_some((opts, self.profile_idx))
            }
            _ => None,
        }
    }

    fn open_choice(&mut self, id: FieldId) {
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
    fn apply_choice(&mut self, id: FieldId, target: usize) -> Option<SettingsIntent> {
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

    fn handle_choice_key(&mut self, key: KeyEvent) -> Option<SettingsIntent> {
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
    fn default_fields(&self) -> Vec<FieldRow> {
        let mut tmp = SettingsScreen::new(AppConfig::default(), self.profiles.clone());
        tmp.section_idx = self.section_idx;
        tmp.model_sub = self.model_sub;
        tmp.sampling_sub = self.sampling_sub;
        tmp.profile_sub = self.profile_sub;
        tmp.profile_idx = self.profile_idx;
        tmp.fields()
    }

    /// Сбрасывает config-поле к значению по умолчанию. Профильные поля и уже
    /// дефолтные значения — no-op (без лишнего сохранения).
    fn reset_field(&mut self, id: FieldId) -> Option<SettingsIntent> {
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
        if value_text(&cur_kind) == value_text(&default_kind) {
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

    // ---------- поиск по полям (`/`) ----------

    /// Строит полный индекс полей всех секций/подсекций для поиска. Поля
    /// mode-зависимой видимости берутся по текущему режиму (managed/облако).
    fn build_search_index(&self) -> Vec<SearchHit> {
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

    fn open_search(&mut self) {
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
    fn search_filter(&mut self) {
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
    fn jump_to_selected(&mut self) {
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

    fn handle_search_key(&mut self, key: KeyEvent) -> Option<SettingsIntent> {
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
                    s.input.clear_or_restore();
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
        // `/` открывает поиск по полям (в редакторе `/` — обычный символ, обработан выше).
        if key.code == KeyCode::Char('/')
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

    /// Палитра по рабочей копии конфига: тема + режим совместимости терминала.
    fn palette(&self) -> Palette {
        Palette::for_theme(self.config.interface.theme)
            .with_compat(self.config.interface.terminal_compat)
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let palette = self.palette();
        // Контекстный футер: базовые хоткеи + специфичные для секции. В «Профилях» —
        // создание/удаление профиля.
        let mut hints: Vec<(&str, &str)> = vec![
            ("Tab", "секция"),
            ("↑↓", "поля"),
            ("Enter", "правка"),
            ("Space", "тумблер"),
            ("←→", "выбор"),
            ("/", "поиск"),
        ];
        if self.focus == Focus::Fields {
            hints.push(("Del", "сброс"));
        }
        if self.section() == Section::Profiles {
            hints.push(("Ctrl+N", "новый"));
            hints.push(("Ctrl+D", "удалить"));
        }
        hints.push(("Esc", "назад"));
        hints.push(("Ctrl+C", "выход"));
        let mut footer = vec![Span::raw("")];
        for (key, desc) in hints {
            footer.push(palette.keycap(key));
            footer.push(Span::styled(format!(" {desc}  "), palette.muted_style()));
        }
        let block = palette
            .panel(format!("{}Настройки", palette.glyphs().settings_icon), true)
            .title_bottom(Line::from(footer));
        let inner = block.inner(area);
        frame.render_widget(Clear, area);
        frame.render_widget(&block, area);

        let [menu_area, fields_area] =
            Layout::horizontal([Constraint::Length(24), Constraint::Min(20)]).areas(inner);

        self.render_menu(frame, menu_area);
        self.render_fields(frame, fields_area);

        // Редактор поверх — с реальным курсором (InputBox::render требует &mut).
        if let Some(editor) = self.editor.as_mut() {
            // Системное сообщение/приветствие — крупный многострочный попап с
            // переносом; прочие поля — компактная однострочная полоса. При ошибке
            // валидации титул несёт красное сообщение и редактор не закрывается.
            let err = editor.error;
            let base_title = if editor.multiline {
                "правка · Shift+Enter перенос · Enter ок · Esc отмена"
            } else {
                "правка · Enter ок · Esc отмена"
            };
            let title = match err {
                Some(e) => format!("{} {e} · Esc отмена", palette.glyphs().warn),
                None => base_title.to_string(),
            };
            let popup = if editor.multiline {
                centered_rect(80, 40, multiline_popup_height(area), area)
            } else {
                centered_rect(60, 30, 3, area)
            };
            // Крупный многострочный попап (системное сообщение/приветствие)
            // притеняет фон, чтобы не сливаться; компактные однострочные полосы —
            // нет (правка на месте).
            if editor.multiline {
                dim_background(frame, &palette);
            }
            frame.render_widget(Clear, popup);
            editor
                .input
                .render(frame, popup, &title, true, &palette, false);
        }

        // Попап выбора Choice-поля — поверх (при поиске редактор/выбор закрыты).
        if self.choice.is_some() {
            self.render_choice(frame, area, &palette);
        }

        // Оверлей поиска по полям — поверх всего (редактор при поиске закрыт).
        if self.search.is_some() {
            self.render_search(frame, area, &palette);
        }
    }

    /// Рисует попап выбора значения Choice-поля: список вариантов, текущий отмечен.
    fn render_choice(&self, frame: &mut Frame, area: Rect, palette: &Palette) {
        let st = self.choice.as_ref().unwrap();
        // Высота = число вариантов + рамка, но не выше экрана; ширина по самой
        // длинной подписи (с запасом), центрирован.
        let want_h = (st.options.len() as u16 + 2).min(area.height.max(3));
        let want_w = st
            .options
            .iter()
            .map(|o| o.chars().count())
            .max()
            .unwrap_or(4) as u16
            + 8;
        let popup = centered_rect_wh(want_w.max(24), want_h.max(3), area);
        dim_background(frame, palette);
        frame.render_widget(Clear, popup);
        let items: Vec<ListItem> = st
            .options
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let mark = if i == st.selected { "› " } else { "  " };
                ListItem::new(Line::from(vec![
                    Span::styled(mark, Style::new().fg(palette.accent)),
                    Span::styled(o.clone(), Style::new().fg(palette.text)),
                ]))
            })
            .collect();
        let block = palette
            .panel("выбор · Enter · Esc", true)
            .border_style(palette.border_style(true));
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::new().reversed());
        let mut state = ListState::default();
        state.select(Some(st.selected.min(st.options.len().saturating_sub(1))));
        frame.render_stateful_widget(list, popup, &mut state);
    }

    /// Рисует оверлей поиска: строка запроса + отфильтрованная выдача.
    fn render_search(&mut self, frame: &mut Frame, area: Rect, palette: &Palette) {
        let popup = centered_rect(72, 50, (area.height * 3 / 4).max(8), area);
        dim_background(frame, palette);
        frame.render_widget(Clear, popup);

        let [input_area, list_area] =
            Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(popup);

        // Снимок для списка (селект/выдача) до мутабельного заимствования input.
        let (results, all_len, selected): (Vec<(String, String)>, usize, usize) = {
            let s = self.search.as_ref().unwrap();
            let rows = s
                .results
                .iter()
                .map(|&ai| {
                    let h = &s.all[ai];
                    (h.crumb.clone(), h.value.clone())
                })
                .collect();
            (rows, s.all.len(), s.selected)
        };

        let title = format!("Поиск полей ({}/{})", results.len(), all_len);
        self.search
            .as_mut()
            .unwrap()
            .input
            .render(frame, input_area, &title, true, palette, false);

        // Список результатов: «крошка   значение» (значение приглушённо).
        let inner_w = list_area.width.saturating_sub(2) as usize;
        let items: Vec<ListItem> = if results.is_empty() {
            vec![ListItem::new(Line::styled(
                "  ничего не найдено",
                palette.muted_style(),
            ))]
        } else {
            results
                .iter()
                .map(|(crumb, value)| {
                    let vw = if value.is_empty() {
                        0
                    } else {
                        (value.chars().count() + 2).min(inner_w / 2)
                    };
                    let (crumb_s, cw) = truncate_to_width(crumb, inner_w.saturating_sub(vw + 1));
                    let mut spans = vec![Span::styled(crumb_s, Style::new().fg(palette.text))];
                    if vw > 0 {
                        let (vs, _) = truncate_to_width(value, inner_w.saturating_sub(cw + 2));
                        spans.push(Span::raw("  "));
                        spans.push(Span::styled(vs, palette.muted_style()));
                    }
                    ListItem::new(Line::from(spans))
                })
                .collect()
        };
        let block = palette
            .panel("Enter — перейти · ↑↓ — выбор · Esc — отмена", false)
            .border_style(palette.border_style(true));
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::new().reversed());
        let mut state = ListState::default();
        if !results.is_empty() {
            state.select(Some(selected.min(results.len() - 1)));
        }
        frame.render_stateful_widget(list, list_area, &mut state);

        // Скроллбар на правой рамке панели, когда результатов больше видимой высоты.
        if list_area.height > 2 {
            let bar = Rect {
                x: list_area.x,
                y: list_area.y + 1,
                width: list_area.width,
                height: list_area.height - 2,
            };
            render_scrollbar(
                frame,
                bar,
                results.len(),
                bar.height as usize,
                state.offset(),
                true,
                palette,
            );
        }
    }

    /// Число редактируемых полей секции (для счётчика в меню слева).
    fn section_field_count(&self, s: Section) -> usize {
        match s {
            Section::Model => self.model_fields().len(),
            Section::Sampling => self.sampling_fields().len(),
            Section::Tools => self.tool_fields().len(),
            Section::Memory => self.memory_fields().len(),
            Section::Profiles => self.profile_fields().len(),
            Section::Interface => self.interface_fields().len(),
        }
    }

    fn render_menu(&self, frame: &mut Frame, area: Rect) {
        let palette = self.palette();
        let focused = self.focus == Focus::Menu;
        // Ширина под содержимое строки меню (минус правая рамка) — для правого
        // выравнивания счётчика полей.
        let inner_w = area.width.saturating_sub(1) as usize;
        // Активная секция помечается цветным рейлом и насыщенным заголовком вне
        // зависимости от фокуса; выбор клавиатурой подсвечивает List highlight.
        let items: Vec<ListItem> = SECTIONS
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let active = i == self.section_idx;
                let bar = if active {
                    Span::styled("▌ ", Style::new().fg(palette.success))
                } else {
                    Span::styled("  ", Style::new())
                };
                let title = if active {
                    Span::styled(s.title(), Style::new().fg(palette.text).bold())
                } else {
                    Span::styled(s.title(), palette.muted_style())
                };
                // Счётчик полей секции, прижатый к правому краю меню.
                let count = self.section_field_count(*s).to_string();
                let used = 2 + label_width(s.title()) + count.chars().count();
                let pad = inner_w.saturating_sub(used).max(1);
                ListItem::new(Line::from(vec![
                    bar,
                    title,
                    Span::raw(" ".repeat(pad)),
                    Span::styled(count, palette.muted_style()),
                ]))
            })
            .collect();
        let block = Block::default()
            .borders(Borders::RIGHT)
            .border_style(palette.border_style(false))
            .title(Span::styled(
                if focused {
                    format!(" {} Секции ", palette.glyphs().collapsed)
                } else {
                    " Секции ".to_string()
                },
                palette.muted_style(),
            ));
        let hl = if focused {
            Style::new().reversed()
        } else {
            Style::new()
        };
        let list = List::new(items).block(block).highlight_style(hl);
        let mut state = ListState::default();
        state.select(Some(self.section_idx));
        frame.render_stateful_widget(list, area, &mut state);
    }

    /// Таб-стрип подсекции для текущей секции: (подписи вкладок, активная).
    /// `None` — секция без подсекций.
    fn subsection_tabs(&self) -> Option<(&'static [&'static str], usize)> {
        match self.section() {
            Section::Model => Some((&MODEL_TABS, self.model_sub as usize)),
            Section::Sampling => Some((&SUB_TABS, self.sampling_sub as usize)),
            Section::Profiles => Some((&SUB_TABS, self.profile_sub as usize)),
            _ => None,
        }
    }

    fn render_fields(&self, frame: &mut Frame, area: Rect) {
        let fields = self.fields();
        let focused = self.focus == Focus::Fields;
        let focused_field = focused.then(|| fields.get(self.field_idx)).flatten();
        let palette = self.palette();

        // Селектор подсекции (если есть в текущем наборе полей) рисуется не строкой
        // списка, а таб-стрипом над ним. Его позиция нужна для «фокуса на вкладках».
        let sub_pos = fields.iter().position(|f| is_subsection(f.id));
        let tabs = sub_pos.and(self.subsection_tabs());

        // Шапка: титул секции (всегда) + таб-стрип (если есть подсекции). Нижняя
        // панель (значение+описание) резервируется всегда при наличии полей.
        let desc_h: u16 = if fields.is_empty() { 0 } else { 4 };
        let head_h: u16 = 1 + if tabs.is_some() { 1 } else { 0 };
        let [head_area, list_area, desc_area] = Layout::vertical([
            Constraint::Length(head_h),
            Constraint::Min(1),
            Constraint::Length(desc_h),
        ])
        .areas(area);

        let mut head_lines = vec![Line::from(vec![
            Span::styled(
                format!(" {} ", palette.glyphs().title_marker),
                Style::new().fg(palette.assistant),
            ),
            Span::styled(
                format!("{} ", self.section().title()),
                Style::new().fg(palette.text).bold(),
            ),
        ])];
        if let Some((labels, active)) = tabs {
            let on_tabs = focused && sub_pos == Some(self.field_idx);
            head_lines.push(tab_strip_line(labels, active, on_tabs, &palette));
        }
        frame.render_widget(Paragraph::new(head_lines), head_area);

        // Колонку значений выравниваем по самой длинной подписи ВНУТРИ группы (не
        // всей секции): одно длинное имя больше не отгоняет значения других групп.
        // Заодно считаем тумблеры группы (вкл/всего) для счётчика в заголовке.
        // Селектор подсекции в списке не рисуется — из выравнивания исключён.
        let mut group_col: HashMap<&str, usize> = HashMap::new();
        let mut group_toggles: HashMap<&str, (usize, usize)> = HashMap::new();
        for f in &fields {
            if is_subsection(f.id) {
                continue;
            }
            let w = group_col.entry(f.group).or_insert(0);
            *w = (*w).max(label_width(&f.label));
            if let FieldKind::Toggle(on) = f.kind {
                let e = group_toggles.entry(f.group).or_insert((0, 0));
                e.1 += 1;
                if on {
                    e.0 += 1;
                }
            }
        }
        const MIN_LABEL_COL: usize = 20;

        // Поля из дефолтного конфига — для маркера «изменено» (строим один раз).
        let default_fields = self.default_fields();

        // Строим элементы: заголовок группы вставляется на переходе к новой
        // непустой группе; `select` — позиция выбранного поля среди элементов (с
        // учётом заголовков) для подсветки/скролла. Селектор подсекции пропускаем
        // (он — таб-стрип): когда курсор на нём, список без выделения.
        let inner_w = list_area.width as usize;
        let mut items: Vec<ListItem> = Vec::with_capacity(fields.len() + 8);
        let mut select: Option<usize> = None;
        let mut prev_group: Option<&str> = None;
        for (i, f) in fields.iter().enumerate() {
            if is_subsection(f.id) {
                continue;
            }
            if !f.group.is_empty() && prev_group != Some(f.group) {
                // Счётчик «вкл/всего» — только для групп с ≥2 тумблерами (там он
                // информативен; для одиночного тумблера дублировал бы видимый [x]).
                let count = group_toggles
                    .get(f.group)
                    .copied()
                    .filter(|&(_, total)| total >= 2);
                items.push(ListItem::new(header_line(
                    f.group, count, inner_w, &palette,
                )));
            }
            prev_group = Some(f.group);
            if focused && i == self.field_idx {
                select = Some(items.len());
            }
            let col = group_col
                .get(f.group)
                .copied()
                .unwrap_or(0)
                .max(MIN_LABEL_COL);
            let modified = default_fields
                .iter()
                .find(|d| d.id == f.id)
                .map(|d| value_text(&d.kind) != value_text(&f.kind))
                .unwrap_or(false);
            // Ширина под значение: минус маркер(2)+подпись+отступ и правый зазор.
            let value_w = inner_w.saturating_sub(col + 4);
            items.push(ListItem::new(render_field_line(
                f, col, value_w, modified, &palette,
            )));
        }
        let total = items.len();

        let hl = if focused {
            Style::new().reversed()
        } else {
            Style::new()
        };
        let list = List::new(items)
            .block(Block::default().borders(Borders::NONE))
            .highlight_style(hl);
        let mut state = ListState::default();
        if let Some(sel) = select {
            state.select(Some(sel));
        }
        frame.render_stateful_widget(list, list_area, &mut state);

        // Скроллбар, когда элементов больше видимой высоты. Рисуем поверх правой
        // рамки экрана настроек: `fields_area` доходит ровно до неё (inner панели),
        // поэтому колонка `list_area.right()` — это линия рамки. Титул/таб-стрип
        // теперь в отдельной шапке (не в списке) → бар на всю высоту `list_area`.
        // Длина содержимого — ПОЛНОЕ число элементов (заголовки групп тоже строки).
        if list_area.height > 0 {
            let bar = Rect {
                width: list_area.width + 1,
                ..list_area
            };
            render_scrollbar(
                frame,
                bar,
                total,
                list_area.height as usize,
                state.offset(),
                true, // рамка экрана настроек рисуется в фокусном цвете
                &palette,
            );
        }

        // Нижняя панель: полное значение выбранного текстового поля (пути целиком,
        // в списке они усечены «…») + описание-подсказка.
        if desc_h > 0 {
            let mut lines: Vec<Line<'static>> = Vec::new();
            if let Some(f) = focused_field {
                if let FieldKind::Text(v) = &f.kind {
                    let shown = v.trim();
                    // Полное значение показываем только для «длинных» полей (пути, URL,
                    // системное сообщение) — в списке они усекаются «…». Короткие
                    // значения (числа, host) в списке видны целиком, дублировать незачем.
                    let long =
                        crate::shared::wrap::display_width(&shown.chars().collect::<Vec<_>>()) > 32;
                    if !shown.is_empty() && shown != "—" && long {
                        // Ограничиваем превью (многострочное системное сообщение
                        // может быть огромным) — панель всё равно клипует по высоте.
                        let preview: String = shown.chars().take(400).collect();
                        lines.push(Line::styled(preview, Style::new().fg(palette.text)));
                    }
                }
                if let Some(text) = field_description(f.id) {
                    lines.push(Line::styled(text, palette.muted_style()));
                }
                // Выключенный глобально инструмент — развёрнутое пояснение (цветом
                // предупреждения), чтобы честный гейт был понятен, а не только «⊘».
                if f.warn {
                    lines.push(Line::styled(
                        "Инструмент включён в профиле, но выключен глобальным \
                         выключателем — он недоступен модели. Включите его в секции \
                         «Инструменты».",
                        Style::new().fg(palette.warning),
                    ));
                }
            }
            let para = Paragraph::new(lines)
                .block(
                    Block::default()
                        .borders(Borders::TOP)
                        .border_style(palette.border_style(false)),
                )
                .wrap(Wrap { trim: true });
            frame.render_widget(para, desc_area);
        }
    }
}

// ---------- свободные функции ----------

/// Входит ли параметр сэмплинга в подмножество, принимаемое облачным провайдером.
/// OpenAI/Gemini (строгий OpenAI-диалект, `restrict_to_strict`): temperature/top_p/
/// penalties/seed/max_tokens. Anthropic (Claude): **только `max_tokens`** — новейшие
/// модели 4.x «зафиксировали» сэмплинг и отвергают `temperature`/`top_p`/`top_k` как
/// deprecated, поэтому их не шлём (см. `anthropic::wire`). Остальные — расширения
/// llama.cpp и reasoning-поля — облако не принимает. См. ADR 0004.
fn cloud_supported_param(provider: CloudProvider, p: SamplingParam) -> bool {
    // Единый источник истины с инструментами get_sampling/set_sampling — набор
    // полей, принимаемых движком провайдера (зеркало wire-диалекта).
    crate::entities::sampling::supported_sampling_fields(Some(provider)).contains(&p.field_name())
}

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
        FieldId::XMode | FieldId::EMode => Some(
            "managed — локальный llama-server (приложение запускает процесс); \
             external — свой OpenAI-совместимый сервер по URL; openai/gemini — облако \
             (нужны имя модели и API-ключ из env-переменной).",
        ),
        FieldId::IxMode => Some(
            "shared — тот же движок, что у ассистента (с семплингом имперсонации); \
             managed — отдельный llama-server; external — отдельный удалённый сервер; \
             openai/gemini — облако (имя модели + API-ключ из env).",
        ),
        FieldId::XApiKeyEnv | FieldId::IxApiKeyEnv | FieldId::EApiKeyEnv => Some(
            "Имя переменной окружения с API-ключом (например OPENAI_API_KEY). Хранится \
             только имя — сам ключ читается из окружения и на диск не пишется.",
        ),
        FieldId::XModelName | FieldId::IxModelName | FieldId::EModelName => Some(
            "Имя модели у провайдера (например gpt-4o, gemini-2.5-pro, \
             text-embedding-3-small). Для облака обязательно.",
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
        FieldId::XFlashAttn | FieldId::IxFlashAttn => Some(
            "FlashAttention — оптимизация механизма внимания: ускоряет генерацию и \
             экономит видеопамять на поддерживаемых GPU. auto — пусть llama.cpp решит \
             сам; on/off — включить/выключить принудительно.",
        ),
        FieldId::XSpecType | FieldId::IxSpecType => Some(
            "Спекулятивное декодирование ускоряет генерацию: «черновик» предлагает \
             несколько токенов вперёд, основная модель их разом проверяет. draft-* — \
             нужна отдельная черновая модель (-md); для MTP-моделей (mtp-gemma-…) — \
             draft-mtp; ngram-* — без модели (черновик из контекста). none — выключено.",
        ),
        FieldId::XDraftModel | FieldId::IxDraftModel => Some(
            "Путь к «черновой» GGUF-модели для спекулятивного декодирования (-md). \
             Должна быть совместима с основной по словарю. Для MTP — путь к \
             соответствующему MTP-GGUF.",
        ),
        FieldId::XDraftNgl | FieldId::IxDraftNgl => {
            Some("Сколько слоёв черновой модели выгрузить на GPU (-ngld). Пусто — авто.")
        }
        FieldId::XDraftNMax | FieldId::IxDraftNMax => Some(
            "Сколько токенов черновая модель предлагает за один шаг \
             (--spec-draft-n-max). Пусто — значение llama.cpp по умолчанию (3).",
        ),
        FieldId::XDraftNMin | FieldId::IxDraftNMin => Some(
            "Минимум черновых токенов за шаг (--spec-draft-n-min). Пусто — по \
             умолчанию (0).",
        ),
        FieldId::MaxToolRounds => Some(
            "Максимум раундов клиентского agentic-loop за один ответ: сколько раз \
             модель может вызвать инструменты подряд, прежде чем цикл принудительно \
             завершится. Защита от зацикливания (по умолчанию 8).",
        ),
        FieldId::TSubMaxTokens => Some(
            "Лимит токенов в ответе субагента (call_subagent) — независимого одно-ходового \
             запроса без истории и инструментов.",
        ),
        FieldId::TSubTimeout => Some("Таймаут запроса субагента (call_subagent) в секундах."),
        FieldId::TWeb => Some(
            "Разрешить инструменты web_search и fetch_url (сетевой доступ). Мастер-гейт: \
             при выключении оба инструмента недоступны модели независимо от настроек профиля.",
        ),
        FieldId::TPython => Some(
            "Разрешить инструмент python_exec (исполнение кода в отдельном процессе). \
             Выключено по умолчанию: код исполняется на вашей машине.",
        ),
        FieldId::TWebFetch => Some(
            "Загружать страницы результатов web-поиска, извлекать читаемый текст и \
             переупорядочивать по релевантности запросу (эмбеддингами). Даёт модели \
             содержимое страниц, но добавляет задержку. Выкл — только заголовки/сниппеты.",
        ),
        FieldId::TFs => Some(
            "Разрешить инструменты чтения/записи/листинга локальных файлов \
             (fs_read/fs_write/fs_list). Выключено по умолчанию: инструмент может \
             прочитать или перезаписать любой файл. Ограничить можно каталогом-песочницей.",
        ),
        FieldId::TFsRoot => Some(
            "Каталог-«песочница» для файловых инструментов: если задан, доступ к файлам \
             ограничен этим каталогом и его подкаталогами (выход через .. блокируется). \
             Пусто — доступ ко всей файловой системе.",
        ),
        FieldId::RagTarget => Some(
            "Целевой размер фрагмента (чанка) базы знаний в символах. Меньше — точнее \
             попадание, но больше фрагментов; больше — шире контекст. Применяется при \
             индексации (/rag add) и реиндексации (/rag rebuild).",
        ),
        FieldId::RagOverlap => Some(
            "Перекрытие соседних фрагментов в символах: хвост предыдущего повторяется \
             в начале следующего, чтобы запрос у границы не терял контекст. При \
             извлечении дубль снимается склейкой.",
        ),
        FieldId::RagMax => Some(
            "Жёсткий потолок неделимого фрагмента в символах (очень длинная строка/слово \
             без пунктуации). Не меньше целевого размера.",
        ),
        FieldId::SmMaxNarrative => Some(
            "Сколько инсайтов (наблюдений) хранить в нарративе «модели себя». При \
             переполнении старые вытесняются. Только для профилей с включёнными \
             инструментами модели себя.",
        ),
        FieldId::SmNarrativeInPrompt => Some(
            "Сколько самых свежих инсайтов подмешивать в системный промпт (новейшие \
             первыми). Больше — богаче контекст «я», но дороже по токенам.",
        ),
        FieldId::SmPromptCap => Some(
            "Потолок символов компактного блока «модели себя», подмешиваемого в системный \
             промпт. Защита окна контекста: длинный блок усекается.",
        ),
        FieldId::SmSummaryTarget => Some(
            "Ориентир размера описания себя (summary) в символах. Сверх него инструменты и \
             протокол ведения мягко предлагают сократить описание, вынеся событийное в \
             наблюдения. Это ворота, а не потолок: данные не усекаются.",
        ),
        FieldId::SmAutoReflect => Some(
            "Авто-рефлексия: каждые N ответов ассистента модель в фоне сама пересматривает \
             разговор и обновляет «модель себя». 0 — выключено. Работает только в профилях \
             с включёнными инструментами модели себя.",
        ),
        FieldId::SmProtocol => Some(
            "Подмешивать в системный промпт нейтральную к персоне инструкцию: когда \
             фиксировать изменения инструментами, «мимолётное — в наблюдения», «точность \
             важнее угодливости». Делает использование инструментов предсказуемым \
             независимо от персоны. Работает только в профилях с включёнными инструментами \
             модели себя.",
        ),
        FieldId::NotesAutoConsolidate => Some(
            "Авто-консолидация («сон»): каждые N ответов модель в фоне сама пересматривает \
             базу заметок — сливает дубли, переписывает устаревшее, связывает родственное. \
             0 — выключено. Работает только в профилях с включёнными инструментами заметок.",
        ),
        FieldId::NotesRecallIncludesSelf => Some(
            "Показывать наблюдения «о себе» (@self) в общем note_recall — с пометкой \
             [о себе]. По умолчанию выключено: память о себе ≠ память о собеседнике. \
             Включение смешивает выдачу (модель увидит свои наблюдения при поиске заметок).",
        ),
        FieldId::ICompat => Some(
            "Режим совместимости со старыми эмуляторами терминала (conhost Windows 10 \
             и т.п.): эмодзи и редкие символы заменяются на простые глифы, рамки — \
             прямые, спиннер — ASCII, затемнение фона попапов — цветом. Включите, \
             если вместо иконок видны квадраты-«тофу».",
        ),
        FieldId::IConfirmKeys => Some(
            "Спрашивать подтверждение перед перегенерацией (Ctrl+R) и удалением последнего \
             обмена (Ctrl+E) — обе операции необратимы в UI. Выключено — комбинации \
             срабатывают сразу.",
        ),
        FieldId::ICopyThoughts => Some(
            "При копировании переписки (F5) включать блок «мыслей» (CoT) ассистента. \
             По умолчанию копируется только текст сообщений.",
        ),
        FieldId::ICopyToolCalls => Some(
            "При копировании переписки (F5) включать параметры вызовов инструментов \
             (имя инструмента и аргументы).",
        ),
        FieldId::ICopyToolResults => Some(
            "При копировании переписки (F5) включать результаты (ответы) вызовов \
             инструментов.",
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
        group: "",
        hint: None,
        warn: false,
    }
}

/// Проставляет группу всем строкам батча — секции строятся как серии
/// `grouped("Группа", vec![...])`, а заголовок группы UI ставит на переходе.
fn grouped(group: &'static str, mut rows: Vec<FieldRow>) -> Vec<FieldRow> {
    for r in &mut rows {
        r.group = group;
    }
    rows
}

/// Текстовая строка из `Option<String>` (пусто → «—»).
fn text_row(id: FieldId, label: &str, value: &Option<String>) -> FieldRow {
    row(
        id,
        label,
        FieldKind::Text(value.clone().unwrap_or_else(|| "—".to_string())),
    )
}

/// Строка из обязательного числового значения (рендерится как текст).
fn num_field<T: ToString>(id: FieldId, label: &str, value: T) -> FieldRow {
    row(id, label, FieldKind::Text(value.to_string()))
}

/// Идентификаторы полей managed-сервера для одного движка (ассистент/имперсонация).
/// Группируем в структуру, чтобы [`managed_rows`] не разрастался списком аргументов.
#[derive(Clone, Copy)]
struct ManagedFieldIds {
    binary: FieldId,
    model: FieldId,
    ngl: FieldId,
    ctx: FieldId,
    flash_attn: FieldId,
    jinja: FieldId,
    no_mmap: FieldId,
    spec_type: FieldId,
    draft_model: FieldId,
    draft_ngl: FieldId,
    draft_n_max: FieldId,
    draft_n_min: FieldId,
    host: FieldId,
    port: FieldId,
}

/// Набор FieldId для модели/сервера ассистента.
const ASSISTANT_MANAGED_IDS: ManagedFieldIds = ManagedFieldIds {
    binary: FieldId::XBinary,
    model: FieldId::XModel,
    ngl: FieldId::XNgl,
    ctx: FieldId::XCtx,
    flash_attn: FieldId::XFlashAttn,
    jinja: FieldId::XJinja,
    no_mmap: FieldId::XNoMmap,
    spec_type: FieldId::XSpecType,
    draft_model: FieldId::XDraftModel,
    draft_ngl: FieldId::XDraftNgl,
    draft_n_max: FieldId::XDraftNMax,
    draft_n_min: FieldId::XDraftNMin,
    host: FieldId::XHost,
    port: FieldId::XPort,
};

/// Набор FieldId для модели/сервера имперсонации.
const IMP_MANAGED_IDS: ManagedFieldIds = ManagedFieldIds {
    binary: FieldId::IxBinary,
    model: FieldId::IxModel,
    ngl: FieldId::IxNgl,
    ctx: FieldId::IxCtx,
    flash_attn: FieldId::IxFlashAttn,
    jinja: FieldId::IxJinja,
    no_mmap: FieldId::IxNoMmap,
    spec_type: FieldId::IxSpecType,
    draft_model: FieldId::IxDraftModel,
    draft_ngl: FieldId::IxDraftNgl,
    draft_n_max: FieldId::IxDraftNMax,
    draft_n_min: FieldId::IxDraftNMin,
    host: FieldId::IxHost,
    port: FieldId::IxPort,
};

/// Поля managed-сервера `llama-server` (общие для движка ассистента/имперсонации),
/// разложенные по смысловым группам: Сервер / Модель / Производительность /
/// Спекулятивное декодирование.
fn managed_rows(m: &ManagedSettings, ids: ManagedFieldIds) -> Vec<FieldRow> {
    let mut rows = grouped(
        "Сервер",
        vec![
            text_row(ids.binary, "Бинарник llama-server", &m.binary),
            row(ids.host, "Host", FieldKind::Text(m.host.clone())),
            num_field(ids.port, "Порт", m.port),
        ],
    );
    rows.extend(grouped(
        "Модель",
        vec![
            text_row(ids.model, "GGUF-модель (-m)", &m.model_path),
            num_field(ids.ctx, "Контекст (-c)", m.context_size),
            row(ids.jinja, "Шаблон (--jinja)", FieldKind::Toggle(m.jinja)),
        ],
    ));
    rows.extend(grouped(
        "Производительность",
        vec![
            num_field(ids.ngl, "GPU-слои (-ngl)", m.gpu_layers),
            row(
                ids.flash_attn,
                "FlashAttn (--flash-attn)",
                FieldKind::Choice(m.flash_attn.label().to_string()),
            ),
            row(
                ids.no_mmap,
                "No-mmap (--no-mmap)",
                FieldKind::Toggle(m.no_mmap),
            ),
        ],
    ));
    let mut spec = vec![row(
        ids.spec_type,
        "Спек. декод. (--spec-type)",
        FieldKind::Choice(m.spec_type.label().to_string()),
    )];
    // Поля черновой модели показываем только для типов draft-* (им нужна модель);
    // ngram-* и none их не используют — не загромождаем секцию.
    if m.spec_type.needs_draft_model() {
        spec.push(text_row(
            ids.draft_model,
            "Черновая модель (-md)",
            &m.draft_model,
        ));
        spec.push(num_row(
            ids.draft_ngl,
            "Черновик GPU-слои (-ngld)",
            m.draft_gpu_layers,
        ));
        spec.push(num_row(ids.draft_n_max, "Черновик n-max", m.draft_n_max));
        spec.push(num_row(ids.draft_n_min, "Черновик n-min", m.draft_n_min));
    }
    rows.extend(grouped("Спекулятивное декодирование", spec));
    rows
}

/// Поля облачного провайдера (модель/API-ключ-env/base URL). `cloud` — настройки
/// активного провайдера (`None` маловероятен в облачном режиме — тогда пустые поля).
fn cloud_rows(
    cloud: Option<&CloudSettings>,
    model_name: FieldId,
    api_key_env: FieldId,
    url: FieldId,
) -> Vec<FieldRow> {
    let none = CloudSettings::default();
    let c = cloud.unwrap_or(&none);
    vec![
        text_row(model_name, "Модель", &c.model_name),
        text_row(api_key_env, "API-ключ (env)", &c.api_key_env),
        text_row(url, "Base URL (опц.)", &c.url),
    ]
}

/// Парсит редактируемое значение опционального числа: пусто → `None` (очистить),
/// корректное → `Some`, нечисло → оставить прежнее значение (как у обязательных полей).
fn parse_opt_num<T: std::str::FromStr + Copy>(text: &str, current: Option<T>) -> Option<T> {
    if text.is_empty() {
        None
    } else {
        text.parse().ok().or(current)
    }
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
        DynatempRange => num_row(id, label, s.dynatemp_range),
        DynatempExp => num_row(id, label, s.dynatemp_exponent),
        TopK => num_row(id, label, s.top_k),
        TopP => num_row(id, label, s.top_p),
        MinP => num_row(id, label, s.min_p),
        TopNSigma => num_row(id, label, s.top_n_sigma),
        TypicalP => num_row(id, label, s.typical_p),
        AdaptiveTarget => num_row(id, label, s.adaptive_target),
        AdaptiveDecay => num_row(id, label, s.adaptive_decay),
        FreqPen => num_row(id, label, s.frequency_penalty),
        PresPen => num_row(id, label, s.presence_penalty),
        RepeatPenalty => num_row(id, label, s.repeat_penalty),
        RepeatLastN => num_row(id, label, s.repeat_last_n),
        DryMultiplier => num_row(id, label, s.dry_multiplier),
        DryBase => num_row(id, label, s.dry_base),
        DryAllowedLength => num_row(id, label, s.dry_allowed_length),
        DryPenaltyLastN => num_row(id, label, s.dry_penalty_last_n),
        DrySeqBreakers => row(
            id,
            label,
            FieldKind::Text(join_breakers(s.dry_sequence_breakers.as_deref())),
        ),
        XtcProbability => num_row(id, label, s.xtc_probability),
        XtcThreshold => num_row(id, label, s.xtc_threshold),
        Mirostat => num_row(id, label, s.mirostat),
        MirostatTau => num_row(id, label, s.mirostat_tau),
        MirostatEta => num_row(id, label, s.mirostat_eta),
        MaxTokens => num_row(id, label, s.max_tokens),
        Seed => num_row(id, label, s.seed),
        Samplers => row(
            id,
            label,
            FieldKind::Text(join_list(s.samplers.as_deref(), ';')),
        ),
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
        DynatempRange => s.dynatemp_range = parse_opt_f32(trimmed),
        DynatempExp => s.dynatemp_exponent = parse_opt_f32(trimmed),
        TopK => s.top_k = parse_opt(trimmed),
        TopP => s.top_p = parse_opt_f32(trimmed),
        MinP => s.min_p = parse_opt_f32(trimmed),
        TopNSigma => s.top_n_sigma = parse_opt_f32(trimmed),
        TypicalP => s.typical_p = parse_opt_f32(trimmed),
        AdaptiveTarget => s.adaptive_target = parse_opt_f32(trimmed),
        AdaptiveDecay => s.adaptive_decay = parse_opt_f32(trimmed),
        FreqPen => s.frequency_penalty = parse_opt_f32(trimmed),
        PresPen => s.presence_penalty = parse_opt_f32(trimmed),
        RepeatPenalty => s.repeat_penalty = parse_opt_f32(trimmed),
        RepeatLastN => s.repeat_last_n = parse_opt(trimmed),
        DryMultiplier => s.dry_multiplier = parse_opt_f32(trimmed),
        DryBase => s.dry_base = parse_opt_f32(trimmed),
        DryAllowedLength => s.dry_allowed_length = parse_opt(trimmed),
        DryPenaltyLastN => s.dry_penalty_last_n = parse_opt(trimmed),
        DrySeqBreakers => s.dry_sequence_breakers = parse_breakers(trimmed),
        XtcProbability => s.xtc_probability = parse_opt_f32(trimmed),
        XtcThreshold => s.xtc_threshold = parse_opt_f32(trimmed),
        Mirostat => s.mirostat = parse_opt(trimmed),
        MirostatTau => s.mirostat_tau = parse_opt_f32(trimmed),
        MirostatEta => s.mirostat_eta = parse_opt_f32(trimmed),
        MaxTokens => s.max_tokens = parse_opt(trimmed),
        Seed => s.seed = parse_opt(trimmed),
        Samplers => s.samplers = parse_list(trimmed, ';'),
        Thinking | Reasoning => {}
    }
}

/// Ширина подписи в терминальных колонках (кириллица/латиница = 1, CJK/эмодзи = 2).
fn label_width(label: &str) -> usize {
    crate::shared::wrap::display_width(&label.chars().collect::<Vec<_>>())
}

/// Инлайн-подсказка для инструмента, выключенного глобальным гейтом («выкл.
/// глобально: <выключатель>»). Показывается цветом предупреждения.
fn gate_hint(gate: ToolGate) -> &'static str {
    match gate {
        ToolGate::Web => "выкл. глобально: Web-поиск",
        ToolGate::Python => "выкл. глобально: Python",
        ToolGate::Fs => "выкл. глобально: файлы",
    }
}

/// Является ли поле селектором подсекции (рисуется таб-стрипом, а не строкой списка).
fn is_subsection(id: FieldId) -> bool {
    matches!(
        id,
        FieldId::ModelSub | FieldId::SamplingSub | FieldId::ProfileSub
    )
}

/// Отображаемое значение поля (для крошки поиска).
fn value_text(kind: &FieldKind) -> String {
    match kind {
        FieldKind::Toggle(on) => (if *on { "вкл" } else { "выкл" }).to_string(),
        FieldKind::Choice(v) => v.clone(),
        FieldKind::Text(v) => v.clone(),
    }
}

/// Добавляет поля секции/подсекции в индекс поиска (пропуская селектор подсекции).
/// `sub_label` — подпись подсекции (в крошку и ловушку), чтобы одинаковые поля
/// разных вкладок различались.
fn collect_hits(
    out: &mut Vec<SearchHit>,
    section_idx: usize,
    section: Section,
    subsection: Option<usize>,
    sub_label: Option<&str>,
    fields: Vec<FieldRow>,
) {
    let head = match sub_label {
        Some(sub) => format!("{} · {}", section.title(), sub),
        None => section.title().to_string(),
    };
    for (fi, f) in fields.iter().enumerate() {
        if is_subsection(f.id) {
            continue;
        }
        let desc = field_description(f.id).unwrap_or("");
        let crumb = if f.group.is_empty() {
            format!("{head} › {}", f.label)
        } else {
            format!("{head} › {} › {}", f.group, f.label)
        };
        let value = value_text(&f.kind);
        let haystack = format!(
            "{head} {} {} {} {}",
            f.group,
            f.label,
            desc,
            f.hint.unwrap_or("")
        )
        .to_lowercase();
        out.push(SearchHit {
            section_idx,
            subsection,
            field_idx: fi,
            crumb,
            value,
            haystack,
        });
    }
}

/// Таб-стрип подсекции: `Ассистент │ Имперсонация │ Эмбеддинги`. Активная вкладка
/// выделена (при фокусе на стрипе — подложкой, иначе — акцентным цветом), справа
/// при фокусе — подсказка `←→`. Разделитель `│` и всё содержимое — WGL4-безопасны.
fn tab_strip_line(tabs: &[&str], active: usize, focused: bool, palette: &Palette) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
    for (i, t) in tabs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" │", palette.border_style(false)));
        }
        let style = if i == active {
            if focused {
                Style::new().fg(palette.text).bg(palette.keycap_bg).bold()
            } else {
                Style::new().fg(palette.accent).bold()
            }
        } else {
            palette.muted_style()
        };
        spans.push(Span::styled(format!(" {t} "), style));
    }
    if focused {
        spans.push(Span::styled("   ←→", palette.muted_style()));
    }
    Line::from(spans)
}

/// Заголовок группы полей: `Группа ──── N/M ──` на всю ширину. Имя — приглушённо-
/// жирным, продолжение — линией цветом рамки; `count = (вкл, всего)` показывает
/// счётчик тумблеров группы. `─` входит в WGL4 → без компат-замены.
fn header_line(
    name: &str,
    count: Option<(usize, usize)>,
    width: usize,
    palette: &Palette,
) -> Line<'static> {
    let label = format!(" {name} ");
    let mut spans = vec![Span::styled(
        label.clone(),
        Style::new().fg(palette.muted).bold(),
    )];
    let mut used = label_width(&label);
    if let Some((on, total)) = count {
        let tag = format!("{on}/{total} ");
        used += label_width(&tag) + 1;
        // Тонкий разделитель + счётчик приглушённым перед линией.
        let dashes_lead = "── ";
        used += label_width(dashes_lead);
        spans.push(Span::styled(dashes_lead, Style::new().fg(palette.border)));
        spans.push(Span::styled(tag, palette.muted_style()));
    }
    let dashes = width.saturating_sub(used + 1);
    spans.push(Span::styled(
        "─".repeat(dashes),
        Style::new().fg(palette.border),
    ));
    Line::from(spans)
}

/// Строка поля: подпись + значение, окрашенное по типу (тумблер — зелёный/
/// приглушённый, выбор — синий, прочерк/пусто — цвет рамки, текст — основной).
/// Значение усекается по `value_w` с «…» (полностью его видно в нижней панели).
fn render_field_line(
    f: &FieldRow,
    label_col: usize,
    value_w: usize,
    modified: bool,
    palette: &Palette,
) -> Line<'static> {
    let (value, value_style) = match &f.kind {
        FieldKind::Toggle(on) => {
            // Гейт: включённый в профиле, но выключенный глобально инструмент —
            // цветом предупреждения (он не действует), а не зелёным.
            let color = if *on {
                if f.warn {
                    palette.warning
                } else {
                    palette.success
                }
            } else {
                palette.muted
            };
            (
                (if *on { "[x]" } else { "[ ]" }).to_string(),
                Style::new().fg(color),
            )
        }
        FieldKind::Choice(v) => (format!("‹ {v} ›"), Style::new().fg(palette.user)),
        FieldKind::Text(v) => {
            let set = v.trim() != "—" && !v.trim().is_empty();
            let style = if set {
                Style::new().fg(palette.text)
            } else {
                Style::new().fg(palette.border)
            };
            (v.clone(), style)
        }
    };
    let (value, vw) = truncate_to_width(&value, value_w.max(1));
    // Дополняем подпись пробелами до ширины колонки по реальной ширине в колонках
    // (Rust `{:<N}` считает символы, а не колонки — для CJK/эмодзи это разъезжается).
    let pad = label_col.saturating_sub(label_width(&f.label));
    // Маркер «изменено против дефолта» (2 колонки), фиксом слева — поля выглядят
    // отступленными под заголовком группы.
    let marker = if modified {
        Span::styled("• ", Style::new().fg(palette.accent))
    } else {
        Span::raw("  ")
    };
    let mut spans = vec![
        marker,
        Span::styled(f.label.clone(), palette.muted_style()),
        Span::raw(" ".repeat(pad + 1)),
        Span::styled(value, value_style),
    ];
    // Инлайн-подсказка (описание инструмента) справа от значения — в остатке ширины.
    if let Some(hint) = f.hint {
        let remaining = value_w.saturating_sub(vw + 2);
        if remaining >= 2 {
            let (h, _) = truncate_to_width(hint, remaining);
            let style = if f.warn {
                Style::new().fg(palette.warning)
            } else {
                palette.muted_style()
            };
            spans.push(Span::raw("  "));
            spans.push(Span::styled(h, style));
        }
    }
    Line::from(spans)
}

/// Усечение строки до `max` колонок с добавлением «…» (WGL4-безопасный). Возвращает
/// усечённую строку и её фактическую ширину в колонках.
fn truncate_to_width(s: &str, max: usize) -> (String, usize) {
    let chars: Vec<char> = s.chars().collect();
    let full = crate::shared::wrap::display_width(&chars);
    if full <= max {
        return (s.to_string(), full);
    }
    if max == 0 {
        return (String::new(), 0);
    }
    let budget = max.saturating_sub(1); // место под «…»
    let mut out = String::new();
    let mut w = 0;
    for i in 0..chars.len() {
        let cw = crate::shared::wrap::width_at(&chars, i);
        if w + cw > budget {
            break;
        }
        w += cw;
        out.push(chars[i]);
    }
    out.push('…');
    (out, w + 1)
}

fn mode_label(m: ServerMode) -> String {
    match m {
        ServerMode::Managed => "managed".into(),
        ServerMode::External => "external".into(),
        ServerMode::OpenAi => "openai".into(),
        ServerMode::Gemini => "gemini".into(),
        ServerMode::Claude => "claude".into(),
    }
}

/// Циклически меняет режим движка (5 значений, с учётом направления ←/→).
fn cycle_mode(m: ServerMode, dir: i32) -> ServerMode {
    use ServerMode::*;
    let order = [Managed, External, OpenAi, Gemini, Claude];
    let idx = order.iter().position(|x| *x == m).unwrap_or(0) as i32;
    let n = order.len() as i32;
    order[(((idx + dir) % n + n) % n) as usize]
}

fn imp_mode_label(m: ImpersonationMode) -> String {
    match m {
        ImpersonationMode::Shared => "shared".into(),
        ImpersonationMode::Managed => "managed".into(),
        ImpersonationMode::External => "external".into(),
        ImpersonationMode::OpenAi => "openai".into(),
        ImpersonationMode::Gemini => "gemini".into(),
        ImpersonationMode::Claude => "claude".into(),
    }
}

/// Циклически меняет режим имперсонации (6 значений, с учётом направления).
fn cycle_imp_mode(m: ImpersonationMode, dir: i32) -> ImpersonationMode {
    use ImpersonationMode::*;
    let order = [Shared, Managed, External, OpenAi, Gemini, Claude];
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

/// Числовой вид поля для валидации (`None` — не числовое: текст/URL/списки/выбор).
fn field_num_kind(id: FieldId) -> Option<NumKind> {
    use FieldId::*;
    match id {
        // Целочисленные поля.
        XNgl | XCtx | XPort | XDraftNgl | XDraftNMax | XDraftNMin | IxNgl | IxCtx | IxPort
        | IxDraftNgl | IxDraftNMax | IxDraftNMin | EPort | MaxToolRounds | TSubMaxTokens
        | TSubTimeout | RagTarget | RagOverlap | RagMax | SmMaxNarrative | SmNarrativeInPrompt
        | SmPromptCap | SmSummaryTarget | SmAutoReflect | NotesAutoConsolidate => {
            Some(NumKind::Int)
        }
        // Параметры семплинга — по своему виду.
        S(p) | IS(p) => p.num_kind(),
        _ => None,
    }
}

/// Ошибка валидации поля (`None` — валидно). Пустой ввод допустим (очистка/сохранение
/// прежнего); непустой в числовом поле обязан парситься. Проверка «мягкая» (i64/f64),
/// точный тип и диапазон досматривает `apply_text`.
fn field_validation_error(id: FieldId, text: &str) -> Option<&'static str> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    match field_num_kind(id) {
        Some(NumKind::Int) if t.parse::<i64>().is_err() => Some("нужно целое число"),
        Some(NumKind::Float) if t.parse::<f64>().is_err() => Some("нужно число"),
        _ => None,
    }
}

/// Порядок вариантов режима движка (для Choice-попапа; совпадает с `cycle_mode`).
const SERVER_MODES: [ServerMode; 5] = [
    ServerMode::Managed,
    ServerMode::External,
    ServerMode::OpenAi,
    ServerMode::Gemini,
    ServerMode::Claude,
];

/// Порядок вариантов режима имперсонации (совпадает с `cycle_imp_mode`).
const IMP_MODES: [ImpersonationMode; 6] = [
    ImpersonationMode::Shared,
    ImpersonationMode::Managed,
    ImpersonationMode::External,
    ImpersonationMode::OpenAi,
    ImpersonationMode::Gemini,
    ImpersonationMode::Claude,
];

/// Порядок тем (совпадает с `cycle_theme`).
const THEMES: [Theme; 3] = [Theme::Auto, Theme::Dark, Theme::Light];

/// Строит (подписи, индекс текущего) из массива вариантов и функции-подписи.
fn index_menu<T: Copy + PartialEq>(
    all: &[T],
    cur: T,
    label: impl Fn(T) -> String,
) -> (Vec<String>, usize) {
    let opts = all.iter().map(|&x| label(x)).collect();
    let idx = all.iter().position(|&x| x == cur).unwrap_or(0);
    (opts, idx)
}

fn flash_menu(cur: FlashAttn) -> (Vec<String>, usize) {
    index_menu(&FlashAttn::ALL, cur, |x| x.label().to_string())
}

fn spec_menu(cur: SpecType) -> (Vec<String>, usize) {
    index_menu(&SpecType::ALL, cur, |x| x.label().to_string())
}

/// Меню выбора для Choice-параметров семплинга (`Thinking`/`Reasoning`); порядок
/// подписей совпадает с циклом `cycle_opt_bool`/`cycle_reasoning`.
fn sampling_choice_menu(s: &SamplingConfig, p: SamplingParam) -> (Vec<String>, usize) {
    match p {
        SamplingParam::Thinking => {
            let opts = [None, Some(true), Some(false)]
                .iter()
                .map(|&b| opt_bool_label(b))
                .collect();
            let idx = match s.thinking {
                None => 0,
                Some(true) => 1,
                Some(false) => 2,
            };
            (opts, idx)
        }
        SamplingParam::Reasoning => {
            let order = [
                None,
                Some(ReasoningEffort::None),
                Some(ReasoningEffort::Low),
                Some(ReasoningEffort::Medium),
                Some(ReasoningEffort::High),
            ];
            let opts = order.iter().map(|&r| reasoning_label(r)).collect();
            let idx = order
                .iter()
                .position(|&r| r == s.reasoning_effort)
                .unwrap_or(0);
            (opts, idx)
        }
        _ => (Vec::new(), 0),
    }
}

/// Поле принадлежит профилю (у него нет config-дефолта → не участвует в `•`/сбросе).
fn is_profile_field(id: FieldId) -> bool {
    matches!(
        id,
        FieldId::PSelect
            | FieldId::PName
            | FieldId::PSystem
            | FieldId::PGreeting
            | FieldId::PImpSystem
            | FieldId::PTool(_)
            | FieldId::ProfileSub
    )
}

fn parse_opt<T: std::str::FromStr>(s: &str) -> Option<T> {
    if s.is_empty() { None } else { s.parse().ok() }
}

fn parse_opt_f32(s: &str) -> Option<f32> {
    parse_opt(s)
}

/// Разбор текстового поля-списка строк (UI): разбивает по `sep`, обрезает
/// пробелы у элементов, отбрасывает пустые; пустой ввод → `None`.
fn parse_list(s: &str, sep: char) -> Option<Vec<String>> {
    let v: Vec<String> = s
        .split(sep)
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
        .collect();
    (!v.is_empty()).then_some(v)
}

/// Склейка списка строк для отображения через `sep`; `None`/пусто → «—».
fn join_list(v: Option<&[String]>, sep: char) -> String {
    match v {
        Some(items) if !items.is_empty() => items.join(&sep.to_string()),
        _ => "—".to_string(),
    }
}

/// DRY-брейкеры: как [`parse_list`] по запятой, но с декодированием эскейпов
/// `\n`/`\t`/`\r` (однострочный редактор не даёт ввести их буквально).
fn parse_breakers(s: &str) -> Option<Vec<String>> {
    let v: Vec<String> = s
        .split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(decode_escapes)
        .collect();
    (!v.is_empty()).then_some(v)
}

/// DRY-брейкеры для отображения: кодирует управляющие символы обратно в `\n`
/// и т.п., склеивает через запятую; `None`/пусто → «—».
fn join_breakers(v: Option<&[String]>) -> String {
    match v {
        Some(items) if !items.is_empty() => items
            .iter()
            .map(|s| encode_escapes(s))
            .collect::<Vec<_>>()
            .join(","),
        _ => "—".to_string(),
    }
}

/// Декодирует литералы `\n`/`\t`/`\r`/`\\` в реальные символы.
fn decode_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Кодирует управляющие символы в литералы `\n`/`\t`/`\r`/`\\` (обратно к [`decode_escapes`]).
fn encode_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\\' => out.push_str("\\\\"),
            other => out.push(other),
        }
    }
    out
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

/// Прямоугольник по центру `area` с явными шириной/высотой (клампятся к `area`).
fn centered_rect_wh(width: u16, height: u16, area: Rect) -> Rect {
    let [h] = Layout::horizontal([Constraint::Length(width.min(area.width))])
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
    use crate::features::tools::default_tool_ids;
    use crate::shared::config::{FlashAttn, SpecType};

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

    /// Переходит на нужную секцию через Tab (устойчиво к порядку секций).
    /// После вызова фокус в меню (Tab сбрасывает его), поля не фокусированы.
    fn goto_section(s: &mut SettingsScreen, sec: Section) {
        for _ in 0..SECTIONS.len() {
            if s.section() == sec {
                return;
            }
            s.handle_key(key(KeyCode::Tab));
        }
        assert_eq!(s.section(), sec, "секция {sec:?} не найдена");
    }

    /// Фокусирует поля и доходит вниз до поля `id` (устойчиво к группам/порядку).
    /// Предполагает, что фокус в меню (как сразу после [`goto_section`]).
    fn goto_field(s: &mut SettingsScreen, id: FieldId) {
        s.handle_key(key(KeyCode::Enter)); // фокус на поля
        for _ in 0..300 {
            if s.fields().get(s.field_idx).map(|f| f.id) == Some(id) {
                return;
            }
            s.handle_key(key(KeyCode::Down));
        }
        panic!("поле {id:?} не найдено в секции {:?}", s.section());
    }

    #[test]
    fn esc_closes() {
        let mut s = screen();
        assert_eq!(s.handle_key(key(KeyCode::Esc)), Some(SettingsIntent::Close));
    }

    #[test]
    fn ctrl_c_quits() {
        let mut s = screen();
        assert_eq!(s.handle_key(ctrl('c')), Some(SettingsIntent::Quit));
        // И при открытом редакторе поля — тоже выход.
        s.editor = Some(Editor {
            field: FieldId::XBinary,
            input: InputBox::new(),
            multiline: false,
            error: None,
        });
        assert_eq!(s.handle_key(ctrl('c')), Some(SettingsIntent::Quit));
    }

    #[test]
    fn tab_cycles_sections() {
        let mut s = screen();
        assert_eq!(s.section(), Section::Model);
        s.handle_key(key(KeyCode::Tab));
        assert_eq!(s.section(), Section::Sampling);
        s.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(s.section(), Section::Model);
    }

    #[test]
    fn toggle_web_emits_save_with_flipped_value() {
        let mut s = screen();
        // Переходим в Инструменты, на тумблер web-поиска.
        goto_section(&mut s, Section::Tools);
        goto_field(&mut s, FieldId::TWeb);
        let intent = s.handle_key(key(KeyCode::Char(' ')));
        match intent {
            Some(SettingsIntent::SaveConfig(c)) => assert!(!c.tools.web_enabled),
            other => panic!("ожидался SaveConfig, получено {other:?}"),
        }
    }

    #[test]
    fn interface_has_terminal_compat_toggle() {
        let mut s = screen();
        // Поле есть в секции «Интерфейс», сразу после темы.
        let rows = s.interface_fields();
        assert!(rows.iter().any(|r| r.id == FieldId::ICompat));
        // Переключение сохраняет конфиг с поднятым флагом…
        match s.toggle_field(FieldId::ICompat) {
            Some(SettingsIntent::SaveConfig(c)) => assert!(c.interface.terminal_compat),
            other => panic!("ожидался SaveConfig, получено {other:?}"),
        }
        // …и палитра рабочей копии тут же переходит на компат-набор глифов.
        assert!(s.palette().compat);
        assert!(field_description(FieldId::ICompat).is_some());
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
        s.handle_key(key(KeyCode::Enter)); // фокус на поля (ModelSub — таб-стрип)
        // → переключает подсекцию на «Имперсонация» (без сохранения).
        assert_eq!(s.handle_key(key(KeyCode::Right)), None);
        assert_eq!(s.model_sub, ModelTab::Impersonation);
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
    fn model_subsection_third_tab_is_embeddings() {
        // Модель имеет третью вкладку «Эмбеддинги» (сервер переехал из «Инструментов»);
        // цикл вкладок вправо: Ассистент → Имперсонация → Эмбеддинги.
        let mut s = screen();
        s.handle_key(key(KeyCode::Enter)); // ModelSub (таб-стрип)
        s.handle_key(key(KeyCode::Right)); // → Имперсонация
        s.handle_key(key(KeyCode::Right)); // → Эмбеддинги
        assert_eq!(s.model_sub, ModelTab::Embeddings);
        let ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
        assert!(ids.contains(&FieldId::EMode));
        assert!(ids.contains(&FieldId::EBinary));
        // В «Инструментах» эмбеддингов больше нет.
        goto_section(&mut s, Section::Tools);
        assert!(!s.fields().iter().any(|f| f.id == FieldId::EMode));
    }

    #[test]
    fn impersonation_profile_subsection_has_no_tools() {
        let mut s = screen();
        goto_section(&mut s, Section::Profiles);
        goto_field(&mut s, FieldId::ProfileSub); // таб-стрип подсекции (поле 0)
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
        goto_field(&mut s, FieldId::XModel); // GGUF-модель (группа «Модель»)
        s.handle_key(key(KeyCode::Enter)); // открыть редактор XModel
        assert!(s.editor.is_some());
        for c in "gemma.gguf".chars() {
            s.handle_key(key(KeyCode::Char(c)));
        }
        let intent = s.handle_key(key(KeyCode::Enter));
        match intent {
            Some(SettingsIntent::SaveConfig(c)) => {
                assert_eq!(c.engine.managed.model_path.as_deref(), Some("gemma.gguf"))
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
        s.handle_key(key(KeyCode::Down)); // XBinary (managed-режим)
        s.handle_key(key(KeyCode::Enter)); // редактор XBinary
        s.handle_key(key(KeyCode::Char('x')));
        let intent = s.handle_key(key(KeyCode::Esc));
        assert_eq!(intent, None);
        assert!(s.editor.is_none());
        // значение не изменилось
        assert!(s.config.engine.managed.binary.is_none());
    }

    #[test]
    fn create_and_delete_profile_in_profiles_section() {
        let mut s = screen();
        goto_section(&mut s, Section::Profiles);
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
        goto_section(&mut s, Section::Profiles);
        goto_field(&mut s, FieldId::PTool(0)); // первый тумблер инструмента
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
        goto_section(&mut s, Section::Profiles);
        goto_field(&mut s, FieldId::PSelect); // селектор профиля (после таб-стрипа)
        assert_eq!(s.profile_idx, 0);
        s.handle_key(key(KeyCode::Right));
        assert_eq!(s.profile_idx, 1);
    }

    #[test]
    fn samplers_list_round_trip() {
        // Порядок семплеров: разбор по «;», склейка обратно, пустой → None.
        let v = parse_list("penalties;dry; temperature ", ';').unwrap();
        assert_eq!(v, vec!["penalties", "dry", "temperature"]);
        assert_eq!(join_list(Some(&v), ';'), "penalties;dry;temperature");
        assert_eq!(parse_list("", ';'), None);
        assert_eq!(parse_list("   ;  ", ';'), None);
        assert_eq!(join_list(None, ';'), "—");
    }

    #[test]
    fn dry_breakers_decode_and_encode_escapes() {
        // Эскейпы \n \t декодируются при вводе и кодируются обратно при показе.
        let v = parse_breakers(r#"\n, :, ", *"#).unwrap();
        assert_eq!(v, vec!["\n", ":", "\"", "*"]);
        assert_eq!(join_breakers(Some(&v)), r#"\n,:,",*"#);
        assert_eq!(parse_breakers(""), None);
        assert_eq!(join_breakers(None), "—");
    }

    #[test]
    fn system_message_editor_is_multiline_and_keeps_newlines() {
        let mut s = screen();
        goto_section(&mut s, Section::Profiles);
        goto_field(&mut s, FieldId::PSystem);
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
    fn ctrl_k_clears_and_restores_multiline_editor() {
        let mut s = screen();
        goto_section(&mut s, Section::Profiles);
        goto_field(&mut s, FieldId::PSystem);
        s.handle_key(key(KeyCode::Enter)); // открыть редактор (многострочный)
        s.handle_key(key(KeyCode::Char('A')));
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        s.handle_key(key(KeyCode::Char('B')));
        let ctrl_k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
        s.handle_key(ctrl_k); // очистка
        assert_eq!(s.editor.as_ref().unwrap().input.text(), "");
        s.handle_key(ctrl_k); // возврат удалённого
        assert_eq!(
            s.editor.as_ref().unwrap().input.text(),
            "Ты — ассистент.A\nB"
        );
        assert!(s.editor.is_some(), "Ctrl+K не закрывает редактор");
    }

    #[test]
    fn greeting_editor_is_multiline_and_keeps_newlines() {
        let mut s = screen();
        goto_section(&mut s, Section::Profiles);
        goto_field(&mut s, FieldId::PGreeting);
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
        s.handle_key(key(KeyCode::Down)); // XBinary (текст, managed-режим)
        s.handle_key(key(KeyCode::Enter)); // редактор
        let editor = s.editor.as_ref().expect("редактор открыт");
        assert!(!editor.multiline, "обычное поле редактируется однострочно");
    }

    #[test]
    fn cloud_mode_reveals_model_and_key_fields() {
        // Переключение режима ассистента на облако (openai) показывает поля
        // «Модель» и «API-ключ (env)» и скрывает параметры llama-server.
        let mut s = screen();
        s.handle_key(key(KeyCode::Enter)); // поля (ModelSub)
        s.handle_key(key(KeyCode::Down)); // XMode
        // managed → external → openai (cycle вправо дважды).
        s.handle_key(key(KeyCode::Right));
        s.handle_key(key(KeyCode::Right));
        assert_eq!(s.config.engine.mode, ServerMode::OpenAi);
        let ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
        assert!(ids.contains(&FieldId::XModelName));
        assert!(ids.contains(&FieldId::XApiKeyEnv));
        // Параметры локального сервера в облачном режиме скрыты.
        assert!(!ids.contains(&FieldId::XNgl));
        assert!(!ids.contains(&FieldId::XBinary));
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
        // Новые поля FlashAttention/спекулятивного декодирования тоже описаны.
        assert!(field_description(FieldId::XFlashAttn).is_some());
        assert!(field_description(FieldId::XSpecType).is_some());
        assert!(field_description(FieldId::XDraftModel).is_some());
    }

    #[test]
    fn managed_mode_shows_flash_attn_and_spec_type() {
        // В managed-режиме (дефолт) видны FlashAttention и --spec-type; черновые
        // поля скрыты, пока тип не draft-*.
        let s = screen();
        let ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
        assert!(ids.contains(&FieldId::XFlashAttn));
        assert!(ids.contains(&FieldId::XSpecType));
        assert!(!ids.contains(&FieldId::XDraftModel));
    }

    #[test]
    fn cycling_spec_type_to_draft_reveals_draft_fields() {
        let mut s = screen();
        s.handle_key(key(KeyCode::Enter)); // фокус на поля (XMode)
        while s.fields().get(s.field_idx).map(|f| f.id) != Some(FieldId::XSpecType) {
            s.handle_key(key(KeyCode::Down));
        }
        // none → draft-simple → draft-eagle3 → draft-mtp (три шага вправо).
        s.handle_key(key(KeyCode::Right));
        s.handle_key(key(KeyCode::Right));
        s.handle_key(key(KeyCode::Right));
        assert_eq!(s.config.engine.managed.spec_type, SpecType::DraftMtp);
        let ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
        assert!(ids.contains(&FieldId::XDraftModel));
        assert!(ids.contains(&FieldId::XDraftNMax));
    }

    #[test]
    fn cycling_flash_attn_changes_value_and_saves() {
        let mut s = screen();
        s.handle_key(key(KeyCode::Enter)); // фокус на поля (XMode)
        while s.fields().get(s.field_idx).map(|f| f.id) != Some(FieldId::XFlashAttn) {
            s.handle_key(key(KeyCode::Down));
        }
        assert_eq!(s.config.engine.managed.flash_attn, FlashAttn::Auto);
        let intent = s.handle_key(key(KeyCode::Right));
        assert_eq!(s.config.engine.managed.flash_attn, FlashAttn::On);
        assert!(matches!(intent, Some(SettingsIntent::SaveConfig(_))));
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
    fn fields_scrollbar_appears_only_on_overflow() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        // Бегунок «█» на правой рамке экрана — только когда полей больше высоты.
        let has_thumb = |term: &Terminal<TestBackend>| {
            let buf = term.backend().buffer();
            let x = buf.area.right() - 1; // колонка рамки панели настроек
            (buf.area.top()..buf.area.bottom()).any(|y| buf[(x, y)].symbol() == "█")
        };
        let mut s = screen();
        goto_section(&mut s, Section::Sampling); // полей+заголовков заведомо больше высоты
        let mut term = Terminal::new(TestBackend::new(80, 14)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
        assert!(has_thumb(&term), "переполненная секция — с бегунком");
        // В высоком окне все поля видны — бегунка нет.
        let mut term = Terminal::new(TestBackend::new(80, 50)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
        assert!(!has_thumb(&term), "все поля видны — без бегунка");
    }

    #[test]
    fn editing_new_sampling_field_commits() {
        let mut s = screen();
        goto_section(&mut s, Section::Sampling);
        goto_field(&mut s, FieldId::S(SamplingParam::MinP));
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
    fn cloud_hides_unsupported_sampling_params() {
        let mut s = screen();
        goto_section(&mut s, Section::Sampling);
        let has =
            |s: &SettingsScreen, p: SamplingParam| s.fields().iter().any(|f| f.id == FieldId::S(p));
        // Локально (managed по умолчанию) — видны все параметры.
        assert!(has(&s, SamplingParam::TopK));
        assert!(has(&s, SamplingParam::Thinking));
        // Облако (OpenAI): расширения llama.cpp/reasoning скрыты, базовые — видны.
        s.config.engine.mode = ServerMode::OpenAi;
        assert!(!has(&s, SamplingParam::TopK));
        assert!(!has(&s, SamplingParam::Thinking));
        assert!(!has(&s, SamplingParam::Reasoning));
        assert!(has(&s, SamplingParam::Temp));
        assert!(has(&s, SamplingParam::TopP));
        assert!(has(&s, SamplingParam::MaxTokens));
        // Claude 4.x «зафиксировал» сэмплинг: виден только max_tokens.
        s.config.engine.mode = ServerMode::Claude;
        assert!(!has(&s, SamplingParam::Temp));
        assert!(!has(&s, SamplingParam::TopP));
        assert!(!has(&s, SamplingParam::FreqPen));
        assert!(has(&s, SamplingParam::MaxTokens));
    }

    #[test]
    fn impersonation_shared_inherits_assistant_cloud_filter() {
        let mut s = screen();
        // Ассистент в облаке, имперсонация в shared → её сэмплинг фильтруется как облако.
        s.config.engine.mode = ServerMode::OpenAi;
        assert_eq!(
            s.config.impersonation_engine.mode,
            ImpersonationMode::Shared
        );
        goto_section(&mut s, Section::Sampling);
        s.handle_key(key(KeyCode::Enter)); // фокус (SamplingSub)
        s.handle_key(key(KeyCode::Right)); // → подсекция Имперсонация
        assert_eq!(s.sampling_sub, Subsection::Impersonation);
        let has_topk = s
            .fields()
            .iter()
            .any(|f| f.id == FieldId::IS(SamplingParam::TopK));
        assert!(
            !has_topk,
            "облако ассистента фильтрует и shared-имперсонацию"
        );
        // Локальная имперсонация (managed) показывает все параметры, даже если ассистент в облаке.
        s.config.impersonation_engine.mode = ImpersonationMode::Managed;
        let has_topk = s
            .fields()
            .iter()
            .any(|f| f.id == FieldId::IS(SamplingParam::TopK));
        assert!(has_topk);
    }

    #[test]
    fn subsection_renders_as_tab_strip_not_list_row() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut s = screen();
        s.handle_key(key(KeyCode::Enter)); // фокус на поля (Модель)
        let mut term = Terminal::new(TestBackend::new(90, 24)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
        let buf = term.backend().buffer();
        let text: String = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        // Все три вкладки подсекции модели видны как таб-стрип.
        assert!(text.contains("Ассистент"));
        assert!(text.contains("Эмбеддинги"));
        // Псевдо-поле «Подсекция» больше не рисуется строкой списка.
        assert!(
            !text.contains("Подсекция"),
            "селектор подсекции должен быть таб-стрипом, а не строкой списка"
        );
    }

    #[test]
    fn profile_tools_are_grouped_with_descriptions() {
        // Каждый тумблер инструмента размечен смысловой группой (из meta) и несёт
        // короткое инлайн-описание. Группы — из известного порядка TOOL_GROUPS.
        let mut s = screen();
        goto_section(&mut s, Section::Profiles);
        let fields = s.profile_fields();
        let tool_rows: Vec<&FieldRow> = fields
            .iter()
            .filter(|r| matches!(r.id, FieldId::PTool(_)))
            .collect();
        assert!(!tool_rows.is_empty());
        for r in &tool_rows {
            assert!(
                meta::TOOL_GROUPS.contains(&r.group),
                "инструмент вне известной группы: {}",
                r.label
            );
            assert!(r.hint.is_some(), "нет инлайн-описания у {}", r.label);
        }
        // Инструменты одной группы идут подряд (заголовок не повторяется).
        let groups: Vec<&str> = tool_rows.iter().map(|r| r.group).collect();
        let mut seen = std::collections::HashSet::new();
        let mut prev = "";
        for g in groups {
            if g != prev {
                assert!(seen.insert(g), "группа {g} не непрерывна");
                prev = g;
            }
        }
    }

    #[test]
    fn globally_disabled_tool_is_marked_gated() {
        // python выключен глобально, но включён в профиле → строка помечена гейтом
        // (warn + подсказка «выкл. глобально»); web включён → обычное описание.
        let mut s = screen();
        s.config.tools.python_enabled = false;
        s.config.tools.web_enabled = true;
        goto_section(&mut s, Section::Profiles);
        let fields = s.profile_fields();
        let idx_of =
            |name: &str| -> usize { all_tool_ids().iter().position(|t| t == name).unwrap() };
        let find = |id: FieldId| fields.iter().find(|r| r.id == id).unwrap();
        let py = find(FieldId::PTool(idx_of("python_exec")));
        assert!(py.warn, "выключенный глобально python_exec — гейт");
        assert!(py.hint.unwrap().contains("глобально"));
        let web = find(FieldId::PTool(idx_of("web_search")));
        assert!(!web.warn, "web включён глобально — не гейт");
        assert_eq!(web.hint, Some("поиск в интернете"));
    }

    #[test]
    fn group_header_shows_toggle_count() {
        // Заголовок группы с ≥2 тумблерами несёт счётчик «вкл/всего»; одиночный — нет.
        let palette = Palette::default();
        let line = header_line("Веб-поиск", Some((1, 2)), 60, &palette);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("1/2"), "нет счётчика: {text:?}");
        let plain = header_line("Сервер", None, 60, &palette);
        let ptext: String = plain.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            !ptext.contains('/'),
            "у группы без счётчика его быть не должно"
        );
    }

    #[test]
    fn choice_popup_opens_and_applies_selection() {
        // Enter на Choice-поле открывает попап списка; ↓ + Enter применяет выбор.
        let mut s = screen();
        goto_field(&mut s, FieldId::XSpecType);
        s.handle_key(key(KeyCode::Enter));
        assert!(s.choice.is_some(), "Enter на Choice открывает попап");
        assert_eq!(s.config.engine.managed.spec_type, SpecType::None);
        s.handle_key(key(KeyCode::Down)); // none → draft-simple
        let intent = s.handle_key(key(KeyCode::Enter));
        assert!(s.choice.is_none(), "Enter применяет и закрывает попап");
        assert_eq!(s.config.engine.managed.spec_type, SpecType::DraftSimple);
        assert!(matches!(intent, Some(SettingsIntent::SaveConfig(_))));
    }

    #[test]
    fn choice_popup_esc_cancels() {
        let mut s = screen();
        goto_field(&mut s, FieldId::XMode);
        s.handle_key(key(KeyCode::Enter));
        assert!(s.choice.is_some());
        s.handle_key(key(KeyCode::Down));
        s.handle_key(key(KeyCode::Esc));
        assert!(s.choice.is_none());
        assert_eq!(
            s.config.engine.mode,
            ServerMode::Managed,
            "Esc не меняет значение"
        );
    }

    #[test]
    fn invalid_number_keeps_editor_open() {
        // Нечисло в числовом поле оставляет редактор открытым с ошибкой; правка сбрасывает.
        let mut s = screen();
        goto_field(&mut s, FieldId::XNgl);
        s.handle_key(key(KeyCode::Enter)); // редактор
        for c in "abc".chars() {
            s.handle_key(key(KeyCode::Char(c)));
        }
        let intent = s.handle_key(key(KeyCode::Enter)); // валидация: не закрывать
        assert_eq!(intent, None);
        assert!(s.editor.is_some(), "невалидный ввод не закрывает редактор");
        assert!(s.editor.as_ref().unwrap().error.is_some());
        // Правка сбрасывает ошибку и валидное значение коммитится.
        s.handle_key(ctrl('k')); // очистить
        for c in "42".chars() {
            s.handle_key(key(KeyCode::Char(c)));
        }
        let intent = s.handle_key(key(KeyCode::Enter));
        assert!(s.editor.is_none());
        match intent {
            Some(SettingsIntent::SaveConfig(c)) => assert_eq!(c.engine.managed.gpu_layers, 42),
            other => panic!("ожидался SaveConfig, получено {other:?}"),
        }
    }

    #[test]
    fn field_validation_error_classifies_numbers() {
        assert!(field_validation_error(FieldId::XNgl, "abc").is_some());
        assert!(field_validation_error(FieldId::XNgl, "12").is_none());
        assert!(field_validation_error(FieldId::XNgl, "").is_none()); // пусто допустимо
        assert!(field_validation_error(FieldId::S(SamplingParam::Temp), "x").is_some());
        assert!(field_validation_error(FieldId::S(SamplingParam::Temp), "0.7").is_none());
        // Текстовые/списочные поля не валидируются как числа.
        assert!(field_validation_error(FieldId::XBinary, "любой текст").is_none());
        assert!(
            field_validation_error(FieldId::S(SamplingParam::Samplers), "top_k;top_p").is_none()
        );
    }

    #[test]
    fn del_resets_field_to_default() {
        let mut s = screen();
        s.config.engine.managed.gpu_layers = 40; // не дефолт
        let default_ngl = AppConfig::default().engine.managed.gpu_layers;
        goto_field(&mut s, FieldId::XNgl);
        let intent = s.handle_key(key(KeyCode::Delete));
        match intent {
            Some(SettingsIntent::SaveConfig(c)) => {
                assert_eq!(c.engine.managed.gpu_layers, default_ngl)
            }
            other => panic!("ожидался SaveConfig, получено {other:?}"),
        }
    }

    #[test]
    fn del_on_default_field_is_noop() {
        // Поле уже в дефолте → Del ничего не делает; профильные поля Del не трогает.
        let mut s = screen();
        goto_field(&mut s, FieldId::XNgl);
        assert_eq!(s.handle_key(key(KeyCode::Delete)), None);
    }

    #[test]
    fn modified_field_shows_marker() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let render_text = |s: &mut SettingsScreen| -> String {
            let mut term = Terminal::new(TestBackend::new(92, 24)).unwrap();
            term.draw(|f| s.render(f)).unwrap();
            let buf = term.backend().buffer();
            (0..buf.area.height)
                .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
                .map(|(x, y)| buf[(x, y)].symbol().to_string())
                .collect()
        };
        // Дефолтный конфиг — маркеров нет.
        let mut s = screen();
        s.handle_key(key(KeyCode::Enter));
        assert!(!render_text(&mut s).contains('•'), "в дефолте маркеров нет");
        // Изменённое поле — маркер появляется.
        s.config.engine.managed.gpu_layers = 40;
        assert!(
            render_text(&mut s).contains('•'),
            "изменённое поле помечено •"
        );
    }

    #[test]
    fn search_filters_and_jumps_to_field() {
        let mut s = screen();
        // `/` открывает поиск; ввод фильтрует по уникальному слову.
        s.handle_key(key(KeyCode::Char('/')));
        assert!(s.search.is_some(), "`/` открывает оверлей поиска");
        for c in "приветствие".chars() {
            s.handle_key(key(KeyCode::Char(c)));
        }
        {
            let st = s.search.as_ref().unwrap();
            assert!(!st.results.is_empty());
            assert!(
                st.results
                    .iter()
                    .all(|&i| st.all[i].haystack.contains("приветствие")),
                "все результаты содержат запрос"
            );
        }
        // Enter — прыжок к полю (секция/фокус/индекс), оверлей закрыт.
        s.handle_key(key(KeyCode::Enter));
        assert!(s.search.is_none());
        assert_eq!(s.section(), Section::Profiles);
        assert!(s.focus == Focus::Fields);
        assert_eq!(
            s.fields().get(s.field_idx).map(|f| f.id),
            Some(FieldId::PGreeting)
        );
    }

    #[test]
    fn search_jump_switches_subsection() {
        // Прыжок в поле неактивной подсекции переключает её (Модель → Эмбеддинги).
        let mut s = screen();
        assert_eq!(s.model_sub, ModelTab::Assistant);
        s.handle_key(key(KeyCode::Char('/')));
        for c in "эмбеддинги порт".chars() {
            s.handle_key(key(KeyCode::Char(c)));
        }
        assert!(!s.search.as_ref().unwrap().results.is_empty());
        s.handle_key(key(KeyCode::Enter));
        assert_eq!(s.section(), Section::Model);
        assert_eq!(s.model_sub, ModelTab::Embeddings);
        assert_eq!(
            s.fields().get(s.field_idx).map(|f| f.id),
            Some(FieldId::EPort)
        );
    }

    #[test]
    fn search_esc_cancels_without_jump() {
        let mut s = screen();
        let before = (s.section_idx, s.field_idx);
        s.handle_key(key(KeyCode::Char('/')));
        for c in "порт".chars() {
            s.handle_key(key(KeyCode::Char(c)));
        }
        s.handle_key(key(KeyCode::Esc));
        assert!(s.search.is_none());
        assert_eq!(
            (s.section_idx, s.field_idx),
            before,
            "Esc не двигает навигацию"
        );
    }

    #[test]
    fn search_index_covers_all_subsections() {
        // Индекс поиска содержит поля всех подсекций (напр. и managed-сервер, и
        // облачная модель ассистента доступны через поиск при текущем режиме).
        let s = screen();
        let idx = s.build_search_index();
        assert!(
            idx.len() > 100,
            "индекс охватывает все секции: {}",
            idx.len()
        );
        // Поле имперсонации-модели индексируется, хотя активна подсекция ассистента.
        assert!(
            idx.iter().any(|h| h.crumb.contains("Имперсонация")),
            "в индексе есть поля подсекции имперсонации"
        );
    }

    #[test]
    fn memory_section_gathers_rag_notes_self_model() {
        // Секция «Память» собрала поля, ранее размазанные по «Инструментам».
        let mut s = screen();
        goto_section(&mut s, Section::Memory);
        let ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
        for id in [
            FieldId::RagTarget,
            FieldId::NotesAutoConsolidate,
            FieldId::SmMaxNarrative,
            FieldId::SmProtocol,
        ] {
            assert!(ids.contains(&id), "в «Памяти» нет {id:?}");
        }
        // А в «Инструментах» их больше нет — там только гейты/параметры.
        goto_section(&mut s, Section::Tools);
        let tool_ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
        assert!(!tool_ids.contains(&FieldId::RagTarget));
        assert!(!tool_ids.contains(&FieldId::SmMaxNarrative));
        // max_tool_rounds переехал из бывшего «Инференса» в «Инструменты».
        assert!(tool_ids.contains(&FieldId::MaxToolRounds));
    }

    #[test]
    fn fields_carry_group_headers() {
        // Поля секции размечены смысловыми группами (заголовки групп в UI).
        let s = screen();
        let groups: Vec<&str> = s.model_fields().iter().map(|f| f.group).collect();
        // Подсекция/режим — вне группы; параметры сервера — в группе «Сервер».
        assert!(groups.iter().any(|g| g.is_empty()));
        assert!(groups.contains(&"Сервер"));
        // Семплинг: параметры сгруппированы по смыслу.
        let sg: Vec<&str> = s.sampling_fields().iter().map(|f| f.group).collect();
        assert!(sg.contains(&"Основные"));
        assert!(sg.contains(&"Рассуждения"));
    }

    #[test]
    fn long_value_is_truncated_with_ellipsis() {
        // Очень длинное значение усекается с «…» под ширину колонки.
        let palette = Palette::default();
        let f = FieldRow {
            id: FieldId::XModel,
            label: "GGUF-модель (-m)".into(),
            kind: FieldKind::Text("D:\\LLM\\GGUF\\very-long-model-name-".repeat(4)),
            group: "Модель",
            hint: None,
            warn: false,
        };
        let line = render_field_line(&f, 20, 24, false, &palette);
        let rendered: String = line.spans.iter().map(|sp| sp.content.as_ref()).collect();
        assert!(
            rendered.contains('…'),
            "длинное значение усечено: {rendered:?}"
        );
    }

    #[test]
    fn sampling_extensions_have_descriptions() {
        // Каждый параметр семплинга снабжён подсказкой в обеих подсекциях —
        // и расширения llama.cpp, и базовые OpenAI-поля.
        for &p in SAMPLING_PARAMS {
            assert!(
                field_description(FieldId::S(p)).is_some(),
                "нет подсказки для {:?} (Ассистент)",
                p.label()
            );
            assert!(
                field_description(FieldId::IS(p)).is_some(),
                "нет подсказки для {:?} (Имперсонация)",
                p.label()
            );
        }
    }
}

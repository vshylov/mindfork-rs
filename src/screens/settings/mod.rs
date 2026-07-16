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
use crate::entities::sampling::{ReasoningEffort, SamplingConfig, Verbosity};
use crate::features::profiles::ProfileEdit;
use crate::features::tools::meta::{ToolGate, ToolInfo};
use crate::shared::config::{
    AppConfig, CloudProvider, CloudSettings, FlashAttn, ImpersonationMode, ManagedSettings,
    PythonMode, ServerMode, SpecType, Theme,
};
use crate::shared::i18n::Locale;
use crate::shared::keys;
use crate::shared::server::{ServerStatus, ServerStatuses};
use crate::shared::theme::Palette;
use crate::shared::ui::{dim_background, render_scrollbar};
use crate::widgets::input_box::{InputBox, RenderOpts};
use crate::widgets::status_bar;

/// Намерение, которое исполняет `app` (транслирует в `AppCommand`).
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsIntent {
    /// Закрыть экран настроек (вернуться в чат).
    Close,
    /// Выйти из приложения (`Ctrl+Q`/`F10`).
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

/// i18n-ключи подписей вкладок [`Subsection`] (порядок = дискриминанты).
const SUB_TAB_KEYS: [&str; 2] = ["ui.settings.tab.assistant", "ui.settings.tab.impersonation"];

impl Subsection {
    fn label(self, loc: &Locale) -> String {
        loc.t(SUB_TAB_KEYS[self as usize]).to_string()
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

/// i18n-ключи подписей вкладок [`ModelTab`] (порядок = дискриминанты).
const MODEL_TAB_KEYS: [&str; 3] = [
    "ui.settings.tab.assistant",
    "ui.settings.tab.impersonation",
    "ui.settings.tab.embeddings",
];

impl ModelTab {
    fn label(self, loc: &Locale) -> String {
        loc.t(MODEL_TAB_KEYS[self as usize]).to_string()
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
    fn title(self, loc: &'static Locale) -> &'static str {
        loc.t(match self {
            Section::Model => "ui.settings.section.model",
            Section::Sampling => "ui.settings.section.sampling",
            Section::Tools => "ui.settings.section.tools",
            Section::Memory => "ui.settings.section.memory",
            Section::Profiles => "ui.settings.section.profiles",
            Section::Interface => "ui.settings.section.interface",
        })
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
    Verbosity,
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
        Verbosity,
    ]
};

impl SamplingParam {
    /// Имя JSON-поля `SamplingConfig` (ASCII). Для большинства параметров совпадает
    /// с UI-подписью; используется и для сверки с набором провайдера, и как
    /// «нетранслируемая» подпись ([`SamplingParam::label`]).
    fn json_name(self) -> &'static str {
        use SamplingParam::*;
        match self {
            Temp => "temperature",
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
            Thinking => "thinking",
            Reasoning => "reasoning_effort",
            Verbosity => "verbosity",
        }
    }

    /// Подпись поля в UI. Большинство параметров подписаны ASCII-именем JSON-поля
    /// (не переводятся); переводятся только «Температура» и «Мысли (thinking)».
    fn label(self, loc: &'static Locale) -> &'static str {
        use SamplingParam::*;
        match self {
            Temp => loc.t("ui.settings.sampling.temp"),
            Thinking => loc.t("ui.settings.sampling.thinking"),
            _ => self.json_name(),
        }
    }

    /// Имя JSON-поля `SamplingConfig` (для сверки с набором, доступным провайдеру).
    fn field_name(self) -> &'static str {
        self.json_name()
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
            DrySeqBreakers | Samplers | Thinking | Reasoning | Verbosity => None,
            // Остальные — вещественные.
            _ => Some(NumKind::Float),
        }
    }

    /// Смысловая группа параметра (заголовок группы в секции «Семплинг»).
    fn group(self, loc: &'static Locale) -> &'static str {
        use SamplingParam::*;
        match self {
            Temp | TopK | TopP | MaxTokens | Seed => loc.t("ui.settings.sampling.group.basic"),
            DynatempRange | DynatempExp => loc.t("ui.settings.sampling.group.dynatemp"),
            MinP | TopNSigma | TypicalP | AdaptiveTarget | AdaptiveDecay | XtcProbability
            | XtcThreshold => loc.t("ui.settings.sampling.group.diversity"),
            FreqPen | PresPen | RepeatPenalty | RepeatLastN => {
                loc.t("ui.settings.sampling.group.penalty")
            }
            DryMultiplier | DryBase | DryAllowedLength | DryPenaltyLastN | DrySeqBreakers => {
                loc.t("ui.settings.sampling.group.dry")
            }
            Mirostat | MirostatTau | MirostatEta => "Mirostat",
            Samplers => loc.t("ui.settings.sampling.group.samplers"),
            Thinking | Reasoning | Verbosity => loc.t("ui.settings.sampling.group.reasoning"),
        }
    }

    /// Подсказка-описание (показывается под полем при фокусе). `None` — без подсказки.
    fn description(self, loc: &'static Locale) -> Option<&'static str> {
        use SamplingParam::*;
        Some(loc.t(match self {
            Temp => "ui.settings.sampling.desc.temp",
            TopK => "ui.settings.sampling.desc.topk",
            TopP => "ui.settings.sampling.desc.topp",
            FreqPen => "ui.settings.sampling.desc.freqpen",
            PresPen => "ui.settings.sampling.desc.prespen",
            DynatempRange => "ui.settings.sampling.desc.dynatemp_range",
            DynatempExp => "ui.settings.sampling.desc.dynatemp_exp",
            AdaptiveTarget => "ui.settings.sampling.desc.adaptive_target",
            AdaptiveDecay => "ui.settings.sampling.desc.adaptive_decay",
            DrySeqBreakers => "ui.settings.sampling.desc.dry_seq_breakers",
            Samplers => "ui.settings.sampling.desc.samplers",
            MinP => "ui.settings.sampling.desc.minp",
            TopNSigma => "ui.settings.sampling.desc.top_n_sigma",
            TypicalP => "ui.settings.sampling.desc.typical_p",
            RepeatPenalty => "ui.settings.sampling.desc.repeat_penalty",
            RepeatLastN => "ui.settings.sampling.desc.repeat_last_n",
            DryMultiplier => "ui.settings.sampling.desc.dry_multiplier",
            DryBase => "ui.settings.sampling.desc.dry_base",
            DryAllowedLength => "ui.settings.sampling.desc.dry_allowed_length",
            DryPenaltyLastN => "ui.settings.sampling.desc.dry_penalty_last_n",
            XtcProbability => "ui.settings.sampling.desc.xtc_probability",
            XtcThreshold => "ui.settings.sampling.desc.xtc_threshold",
            Mirostat => "ui.settings.sampling.desc.mirostat",
            MirostatTau => "ui.settings.sampling.desc.mirostat_tau",
            MirostatEta => "ui.settings.sampling.desc.mirostat_eta",
            Seed => "ui.settings.sampling.desc.seed",
            MaxTokens => "ui.settings.sampling.desc.max_tokens",
            Thinking => "ui.settings.sampling.desc.thinking",
            Reasoning => "ui.settings.sampling.desc.reasoning",
            Verbosity => "ui.settings.sampling.desc.verbosity",
        }))
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
    TPythonMode,
    TPythonPath,
    TPythonNet,
    TPythonWasmTimeout,
    TPythonWasmMemory,
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
    /// Язык интерфейса (ось B, docs/i18n-ui.md) — независим от языка агентов.
    ILanguage,
    /// Режим совместимости со старыми терминалами (эмодзи → безопасные глифы).
    ICompat,
    /// Горизонтальные разделители между строками Markdown-таблиц в ленте.
    ITableSeparators,
    /// Рендер ```mermaid-блоков ленты диаграммой (фолбэк — исходник).
    IMermaid,
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
    /// Язык служебного каркаса профиля (ось A, docs/history/i18n.md). Choice ru/en;
    /// блокируется при появлении данных у профиля.
    PLanguage,
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
/// смысловая группа (для заголовка группы и счётчика тумблеров; `""` — вне
/// группы, без заголовка). Значения выравниваются единой колонкой на всю
/// секцию ([`section_label_col`]), группа на колонку не влияет.
struct FieldRow {
    id: FieldId,
    label: String,
    kind: FieldKind,
    group: &'static str,
    /// Короткая инлайн-подсказка справа от значения (описание инструмента). `None` — нет.
    hint: Option<&'static str>,
    /// Человекопонятное описание поля (нижняя панель настроек + ловушка поиска).
    /// Живёт рядом с подписью — задаётся при построении строки через [`FieldRow::describe`]
    /// (раньше — отдельный match `field_description(id)`). `None` — без описания.
    description: Option<&'static str>,
    /// Значение и подсказку рисовать цветом предупреждения — инструмент включён в
    /// профиле, но выключен глобальным гейтом (недоступен модели).
    warn: bool,
}

impl FieldRow {
    /// Прикрепляет описание поля (builder-стиль: `row(...).describe("…")`).
    fn describe(mut self, d: &'static str) -> Self {
        self.description = Some(d);
        self
    }
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
    /// Снимок статусов серверов (чат/эмбеддинги/имперсонация) — чипы в секции
    /// «Модель/сервер». Обновляется `app` из события `ServerStatus`. См. spec §11.6.
    statuses: ServerStatuses,
    /// Id профилей с заблокированным языком каркаса (у профиля появились данные —
    /// поле «Язык» рисуется заблокированным, правка гасится). Из снимка `Settings`
    /// (считает оркестратор). См. docs/history/i18n.md.
    language_locked: Vec<uuid::Uuid>,
}

// ---------- подмодули (разбор god-object'а: docs/history/refactoring-god-objects.md) ----------

mod apply;
mod catalog;
mod choice;
mod helpers;
mod render;
mod search;
mod spec;

#[cfg(test)]
mod tests;

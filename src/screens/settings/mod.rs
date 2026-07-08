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
use crate::features::tools::meta::{ToolGate, ToolInfo};
use crate::shared::config::{
    AppConfig, CloudProvider, CloudSettings, FlashAttn, ImpersonationMode, ManagedSettings,
    ServerMode, SpecType, Theme,
};
use crate::shared::keys;
use crate::shared::server::{ServerStatus, ServerStatuses};
use crate::shared::theme::Palette;
use crate::shared::ui::{dim_background, render_scrollbar};
use crate::widgets::input_box::InputBox;
use crate::widgets::status_bar;

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
}

// ---------- подмодули (разбор god-object'а: docs/refactoring-god-objects.md) ----------

mod apply;
mod catalog;
mod choice;
mod helpers;
mod render;
mod search;

#[cfg(test)]
mod tests;

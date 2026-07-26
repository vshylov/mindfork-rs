//! The settings screen (an FSD "page"): sections, navigation, and field editing.
//! Entered via `Ctrl+P` from chat. See spec §11.6.
//!
//! Like [`super::chat::ChatScreen`], the screen doesn't know about `app`/channels: on
//! an edit it returns a [`SettingsIntent`], which `app` translates into an `AppCommand`
//! (`UpdateConfig`/`UpdateProfile`/`CreateProfile`/`DeleteProfile`). Edits are
//! applied **immediately on commit** of the field (the orchestrator is the sole writer
//! and restarts the server on a model change). Works on its own working copy of
//! `AppConfig`/profiles, updated by the same edits.

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
    PythonMode, ServerMode, SpecType, Theme, TtsCloudSettings, TtsMode,
};
use crate::shared::i18n::Locale;
use crate::shared::keys;
use crate::shared::server::{ServerStatus, ServerStatuses};
use crate::shared::theme::Palette;
use crate::shared::ui::{dim_background, render_scrollbar};
use crate::widgets::input_box::{InputBox, RenderOpts};
use crate::widgets::status_bar;

/// The intent that `app` executes (translates into an `AppCommand`).
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsIntent {
    /// Close the settings screen (return to chat).
    Close,
    /// Quit the app (`Ctrl+Q`/`F10`).
    Quit,
    /// Save the configuration (an edit to any section except profiles).
    SaveConfig(Box<AppConfig>),
    /// Save profile edits.
    SaveProfile { id: Uuid, edit: Box<ProfileEdit> },
    /// Create a new profile.
    CreateProfile {
        name: String,
        system_message: String,
    },
    /// Delete a profile.
    DeleteProfile(Uuid),
    /// Confirm a changed MCP-server tool catalog (TOFU,
    /// Enter on a server row marked "catalog changed"). See spec §9.6.
    ConfirmMcpCatalog(String),
    /// Save the entered cloud-provider API key (empty — delete it).
    /// The orchestrator encrypts it with the machine key; the screen's config has no keys.
    /// See `shared::secrets`, docs/research/api-key-storage.md.
    SetApiKey {
        provider: crate::shared::config::CloudProvider,
        key: String,
    },
}

/// Settings sections (the left menu). See spec §11.6.
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

/// The "Assistant" / "Impersonation" subsection inside the Sampling/Profiles sections.
/// See spec §11.8. Shown as a tab strip above the section's fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Subsection {
    Assistant,
    Impersonation,
}

/// i18n keys for [`Subsection`] tab labels (order = discriminants).
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

    /// All variants (for enumerating fields of all subsections during search).
    const ALL: [Subsection; 2] = [Subsection::Assistant, Subsection::Impersonation];

    fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or(Subsection::Assistant)
    }
}

/// The "Model/server" section's subsection: the app's three servers (mirroring the
/// status bar's chat/imp/emb chips) — assistant, impersonation, embeddings. Shown as a
/// tab strip above the fields. See spec §11.6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelTab {
    Assistant,
    Impersonation,
    Embeddings,
    /// Speech (TTS) — an independent slot: Anthropic has no TTS at all, so the
    /// provider is chosen separately from the chat engine. See spec §11.9.
    Tts,
}

/// i18n keys for [`ModelTab`] tab labels (order = discriminants).
const MODEL_TAB_KEYS: [&str; 4] = [
    "ui.settings.tab.assistant",
    "ui.settings.tab.impersonation",
    "ui.settings.tab.embeddings",
    "ui.settings.tab.tts",
];

impl ModelTab {
    fn label(self, loc: &Locale) -> String {
        loc.t(MODEL_TAB_KEYS[self as usize]).to_string()
    }

    /// All variants (for enumerating fields of all subsections during search).
    const ALL: [ModelTab; 4] = [
        ModelTab::Assistant,
        ModelTab::Impersonation,
        ModelTab::Embeddings,
        ModelTab::Tts,
    ];

    /// Cyclically shifts the tab (←/→ across the tab strip).
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

/// The numeric kind of an editable field (for validating input without closing the editor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumKind {
    Int,
    Float,
}

/// A sampling parameter. Addresses a specific [`SamplingConfig`] field within a
/// subsection; the subsection itself ("Assistant"/"Impersonation") is encoded by
/// the [`FieldId::S`]/[`FieldId::IS`] constructor. Numeric parameters are
/// edited as text, `Thinking`/`Reasoning` — as a cyclic choice.
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

/// The order of sampling parameters in the section (stable = render order).
/// Grouped by meaning: parameters of one group run consecutively, so the group
/// header ([`SamplingParam::group`]) is drawn once before the series in the UI.
const SAMPLING_PARAMS: &[SamplingParam] = {
    use SamplingParam::*;
    &[
        // Basics
        Temp,
        TopK,
        TopP,
        MaxTokens,
        Seed,
        // Dynamic temperature
        DynatempRange,
        DynatempExp,
        // Diversity
        MinP,
        TopNSigma,
        TypicalP,
        AdaptiveTarget,
        AdaptiveDecay,
        XtcProbability,
        XtcThreshold,
        // Repeat penalties
        FreqPen,
        PresPen,
        RepeatPenalty,
        RepeatLastN,
        // DRY (anti-repeat)
        DryMultiplier,
        DryBase,
        DryAllowedLength,
        DryPenaltyLastN,
        DrySeqBreakers,
        // Mirostat
        Mirostat,
        MirostatTau,
        MirostatEta,
        // Sampler order
        Samplers,
        // Reasoning
        Thinking,
        Reasoning,
        Verbosity,
    ]
};

impl SamplingParam {
    /// The `SamplingConfig` JSON field name (ASCII). For most parameters matches
    /// the UI label; used both for cross-checking against the provider's set and as
    /// the "untranslated" label ([`SamplingParam::label`]).
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

    /// The field's UI label. Most parameters are labeled with the ASCII JSON-field
    /// name (not translated); only "Temperature" and "Thoughts (thinking)" are translated.
    fn label(self, loc: &'static Locale) -> &'static str {
        use SamplingParam::*;
        match self {
            Temp => loc.t("ui.settings.sampling.temp"),
            Thinking => loc.t("ui.settings.sampling.thinking"),
            _ => self.json_name(),
        }
    }

    /// The `SamplingConfig` JSON field name (for cross-checking against the set the
    /// provider accepts).
    fn field_name(self) -> &'static str {
        self.json_name()
    }

    /// The parameter's numeric kind for editor validation (`None` — not a number: lists/
    /// the `Thinking`/`Reasoning` choice).
    fn num_kind(self) -> Option<NumKind> {
        use SamplingParam::*;
        match self {
            // Integers.
            TopK | RepeatLastN | DryAllowedLength | DryPenaltyLastN | Mirostat | MaxTokens
            | Seed => Some(NumKind::Int),
            // Lists/choice — not a number.
            DrySeqBreakers | Samplers | Thinking | Reasoning | Verbosity => None,
            // The rest — floats.
            _ => Some(NumKind::Float),
        }
    }

    /// The parameter's semantic group (the group header in the "Sampling" section).
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

    /// The description hint (shown under the field when focused). `None` — no hint.
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

/// An editable field's identifier (stable order = order within the section).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldId {
    // "Assistant"/"Impersonation" subsection selectors
    ModelSub,
    SamplingSub,
    ProfileSub,
    // Model/server — Assistant (llama-server)
    XMode,
    XUrl,
    XBinary,
    XModel,
    /// The cloud/multi-model model's name (`model_name`).
    XModelName,
    /// The env-variable name holding the API key (cloud).
    XApiKeyEnv,
    /// The cloud provider's API key itself (entered in settings, stored
    /// encrypted with the machine key). The field's value is a **status** "configured/not
    /// set", not a secret; editing opens an empty masked editor.
    /// See `shared::secrets`, docs/research/api-key-storage.md.
    XApiKey,
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
    // Model/server — Impersonation
    IxMode,
    IxUrl,
    IxBinary,
    IxModel,
    IxModelName,
    IxApiKeyEnv,
    /// The cloud API key for the impersonation engine (see [`FieldId::XApiKey`]).
    IxApiKey,
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
    // Inference
    MaxToolRounds,
    // Sampling — a field per parameter; the subsection is encoded by the constructor.
    // `S` — Assistant (`default_sampling`), `IS` — Impersonation
    // (`impersonation_sampling`).
    S(SamplingParam),
    IS(SamplingParam),
    // Tools
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
    /// The MCP host's master switch (`config.mcp.enabled`); servers are edited in
    /// `settings.json` (R6).
    TMcpEnabled,
    /// An MCP server's status row by index in the `mcp.servers` snapshot (read-only;
    /// Enter when "catalog changed" confirms the new catalog — TOFU).
    TMcpServer(usize),
    TSubMaxTokens,
    TSubTimeout,
    EMode,
    EUrl,
    EBinary,
    EModel,
    EModelName,
    EApiKeyEnv,
    /// The cloud API key for the embedding server (see [`FieldId::XApiKey`]).
    EApiKey,
    EPort,
    // Speech (TTS, the "Speech" tab of the "Model" section). See spec §11.9.
    TtsMode,
    TtsModelName,
    TtsVoice,
    TtsUserVoice,
    TtsInstructions,
    TtsSpeed,
    /// The speech cloud provider's API key (shared with chat, ADR 0008).
    TtsApiKey,
    TtsApiKeyEnv,
    TtsUrl,
    TtsSpeakRoles,
    TtsStopOnSwitch,
    TtsStopOnGeneration,
    // RAG (knowledge-base chunking)
    RagTarget,
    RagOverlap,
    RagMax,
    // Self-model (narrative, prompt injection)
    SmMaxNarrative,
    SmNarrativeInPrompt,
    SmPromptCap,
    SmSummaryTarget,
    SmAutoReflect,
    SmAutoConsolidate,
    SmProtocol,
    NotesAutoConsolidate,
    NotesRecallIncludesSelf,
    // Interface
    ITheme,
    /// The interface language (axis B, docs/i18n-ui.md) — independent of the agent language.
    ILanguage,
    /// Compatibility mode for old terminals (emoji → safe glyphs).
    ICompat,
    /// Horizontal separators between rows of feed Markdown tables.
    ITableSeparators,
    /// Render ```mermaid blocks in the feed as a diagram (fallback — the source).
    IMermaid,
    ISpell,
    IDicts,
    /// Confirmation before `Ctrl+R`/`Ctrl+E` (irreversible operations).
    IConfirmKeys,
    /// Copy "thoughts" (CoT) when copying the conversation (`F5`).
    ICopyThoughts,
    /// Copy tool-call parameters when copying the conversation.
    ICopyToolCalls,
    /// Copy tool-call results when copying the conversation.
    ICopyToolResults,
    // Profiles (dynamic)
    PSelect,
    PName,
    /// The profile's scaffold language (axis A, docs/history/i18n.md). Choice ru/en;
    /// locked once the profile has data.
    PLanguage,
    PSystem,
    PGreeting,
    /// What the feed and the `F5` export call the user in this profile's chats
    /// (`character_names.user`). Empty — the localized default (`YOU`). See spec §5.1.
    PUserName,
    /// The same for the assistant (`character_names.assistant`).
    PAssistantName,
    /// The impersonation profile the assistant profile's chats use (a reference by
    /// id; the first option — "no reference", the shared default text). See spec §11.8.
    PImpProfile,
    /// A profile tool toggle by index in the catalog.
    PTool(usize),
    // Impersonation profiles (the "Impersonation" subsection of "Profiles"; the list
    // lives in `AppConfig.impersonation_profiles`).
    /// The impersonation profile selector.
    IpSelect,
    IpName,
    IpSystem,
}

/// How a field is edited (for rendering and key handling).
enum FieldKind {
    /// A boolean toggle (Space flips it).
    Toggle(bool),
    /// A cyclic choice among options (←/→ cycle it).
    Choice(String),
    /// Text/number (Enter opens the editor).
    Text(String),
}

/// A field row: identifier, label, the current value representation, and the
/// semantic group (for the group header and toggle counter; `""` — outside a
/// group, no header). Values are aligned on a single column across the whole
/// section ([`section_label_col`]); the group doesn't affect the column.
struct FieldRow {
    id: FieldId,
    label: String,
    kind: FieldKind,
    group: &'static str,
    /// A short inline hint right of the value (a tool's description). `None` — none.
    hint: Option<&'static str>,
    /// A human-readable field description (the settings bottom panel + the search
    /// trap). Lives next to the label — set when building the row via [`FieldRow::describe`]
    /// (previously — a separate `field_description(id)` match). `None` — no description.
    /// `String` (not `&'static`): MCP-tool descriptions are dynamic
    /// server text (full display is an antidote to tool-poisoning, spec §9.6).
    description: Option<String>,
    /// Draw the value and hint in warning color — a tool enabled in the
    /// profile but disabled by a global gate (unavailable to the model).
    warn: bool,
}

impl FieldRow {
    /// Attaches a field's description (builder-style: `row(...).describe("…")`).
    fn describe(mut self, d: impl Into<String>) -> Self {
        self.description = Some(d.into());
        self
    }
}

/// The active text-field editor (a popup).
struct Editor {
    field: FieldId,
    input: InputBox,
    /// A multiline editor (system message and greeting): wraps long
    /// lines, `Shift+Enter` inserts a line break, a large popup. Other fields —
    /// single-line.
    multiline: bool,
    /// A validation error (e.g. "need a number"): the editor doesn't close on
    /// `Enter`, the label turns red. `None` — the input is valid.
    error: Option<&'static str>,
}

/// Focus: the left section menu or the field list on the right.
#[derive(PartialEq)]
enum Focus {
    Menu,
    Fields,
}

/// One field-search target: jump coordinates + text for display/matching.
struct SearchHit {
    section_idx: usize,
    /// The subsection to jump to (a discriminant; `None` — a section with no subsections).
    subsection: Option<usize>,
    /// The field's index in the matching subsection's `*_fields()`.
    field_idx: usize,
    /// "Section › Group › Label" for display.
    crumb: String,
    /// The field's current value (truncated for display).
    value: String,
    /// The match trap (lowercase): section + group + label + description + hint.
    haystack: String,
}

/// The field-search overlay (`/`): a query line + a flat filtered result set.
struct SearchState {
    input: InputBox,
    /// The full index of fields across all sections/subsections (built on open).
    all: Vec<SearchHit>,
    /// Indices into `all` that passed the query filter.
    results: Vec<usize>,
    selected: usize,
}

/// The Choice-field value picker popup (Enter): the option list with the current one marked.
struct ChoiceState {
    field: FieldId,
    options: Vec<String>,
    selected: usize,
}

/// The settings screen: a working copy of the configuration and profiles + navigation state.
pub struct SettingsScreen {
    config: AppConfig,
    profiles: Vec<Profile>,
    section_idx: usize,
    field_idx: usize,
    focus: Focus,
    /// The selected profile in the "Profiles" section.
    profile_idx: usize,
    /// The selected impersonation profile in the "Impersonation" subsection of
    /// "Profiles" (an index into `config.impersonation_profiles`).
    imp_profile_idx: usize,
    /// A profile creation (`Ctrl+N`) is in flight: the orchestrator owns the profile
    /// list, so the new profile only arrives with the next `Settings` snapshot —
    /// [`SettingsScreen::refresh`] then selects whichever profile is new. One-shot.
    pending_profile_select: bool,
    /// The active subsections. Model — three tabs (Assistant/Impersonation/Embeddings);
    /// Sampling/Profiles — two (Assistant/Impersonation).
    model_sub: ModelTab,
    sampling_sub: Subsection,
    profile_sub: Subsection,
    editor: Option<Editor>,
    /// The field-search overlay (`/`); `None` — closed.
    search: Option<SearchState>,
    /// The Choice-field value picker popup (Enter); `None` — closed.
    choice: Option<ChoiceState>,
    /// A snapshot of server statuses (chat/embeddings/impersonation) — chips in the
    /// "Model/server" section. Updated by `app` from the `ServerStatus` event. See spec §11.6.
    statuses: ServerStatuses,
    /// Ids of profiles with a locked scaffold language (the profile has data —
    /// the "Language" field is drawn locked, edits are gated). From the `Settings`
    /// snapshot (computed by the orchestrator). See docs/history/i18n.md.
    language_locked: Vec<uuid::Uuid>,
    /// The MCP-host snapshot (from the `Settings` event): the dynamic tool catalog
    /// (appended to the static `tool_catalog()` for profile toggles) +
    /// server statuses (rows in the "Tools" section, TOFU confirmation).
    /// Empty until servers come up/while MCP is off. See spec §9.6.
    mcp: crate::features::tools::mcp::McpSnapshot,
    /// Providers whose API key is stored on **this** machine (from the `Settings`
    /// snapshot). The "API key" field shows its status from this; the screen doesn't
    /// hold the keys themselves. See `shared::secrets`, docs/research/api-key-storage.md.
    api_keys_present: Vec<crate::shared::config::CloudProvider>,
}

// ---------- submodules (a breakup of a god object: docs/history/refactoring-god-objects.md) ----------

mod apply;
mod catalog;
mod choice;
mod helpers;
mod render;
mod search;
mod spec;

#[cfg(test)]
mod tests;

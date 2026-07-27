//! Global application configuration (`settings.json`). See spec §12.1.
//! Versioned via the `schema_version` field for future migrations.

use serde::{Deserialize, Serialize};

use crate::entities::sampling::SamplingConfig;

/// Current config schema version.
pub const SCHEMA_VERSION: u32 = 1;

/// Inference-engine connection mode. Local (`Managed`/`External`) and cloud
/// providers (`OpenAi`/`Gemini`) are equal-footing variants of a single selector
/// (flat taxonomy, [ADR 0004](decisions/0004-engine-contract-multi-provider.md)).
/// Claude is added in Phase 2 (a separate `/v1/messages` protocol).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerMode {
    /// The app itself launches a child `llama-server` process.
    #[default]
    Managed,
    /// Connecting to an already-running OpenAI-compatible server (any: llama.cpp,
    /// vLLM, LM Studio…). Sampling fields are sent as-is (lenient dialect).
    External,
    /// OpenAI cloud (`platform.openai.com`). Strict OpenAI dialect + Bearer key.
    #[serde(rename = "openai")]
    OpenAi,
    /// Google Gemini cloud via an OpenAI-compatible endpoint. Strict dialect + key.
    Gemini,
    /// Anthropic cloud (`platform.claude.com`). A separate Messages API protocol
    /// (`/v1/messages`), `x-api-key`. See ADR 0004, Phase 2.
    Claude,
}

/// Inference cloud provider. `OpenAi`/`Gemini` speak the OpenAI protocol,
/// `Claude` — the Anthropic Messages API. Carries a default base URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudProvider {
    OpenAi,
    Gemini,
    Claude,
}

impl CloudProvider {
    /// Base URL for **embeddings**/compat access (an OpenAI-compatible
    /// endpoint). For Gemini this is the compat path `…/v1beta/openai` (RAG
    /// embeddings go through it, `OpenAiClient`). Overridable via the `url` field.
    pub fn base_url(self) -> &'static str {
        match self {
            CloudProvider::OpenAi => "https://api.openai.com/v1",
            CloudProvider::Gemini => "https://generativelanguage.googleapis.com/v1beta/openai",
            // The Anthropic client appends `/v1/messages` itself, hence no suffix.
            CloudProvider::Claude => "https://api.anthropic.com",
        }
    }

    /// Base URL for **chat** (the client's native protocol). For Gemini — `…/v1beta`
    /// (the native `GeminiClient` appends `/models/{model}:streamGenerateContent`),
    /// unlike the embeddings compat path ([`base_url`](Self::base_url)). For OpenAI
    /// (Responses) and Claude it matches `base_url`. Overridable via the `url` field.
    pub fn chat_base_url(self) -> &'static str {
        match self {
            CloudProvider::Gemini => "https://generativelanguage.googleapis.com/v1beta",
            CloudProvider::OpenAi | CloudProvider::Claude => self.base_url(),
        }
    }

    /// Stable provider string key — indexes stored API keys
    /// (`AppConfig::api_keys`, see `shared::secrets`). The key is **shared** across
    /// chat, impersonation, and embeddings for this provider. Values persist in
    /// `settings.json` — do not rename.
    pub fn key(self) -> &'static str {
        match self {
            CloudProvider::OpenAi => "openai",
            CloudProvider::Gemini => "gemini",
            CloudProvider::Claude => "claude",
        }
    }
}

impl ServerMode {
    /// Cloud provider for this mode (`None` — local managed/external).
    pub fn cloud_provider(self) -> Option<CloudProvider> {
        match self {
            ServerMode::OpenAi => Some(CloudProvider::OpenAi),
            ServerMode::Gemini => Some(CloudProvider::Gemini),
            ServerMode::Claude => Some(CloudProvider::Claude),
            ServerMode::Managed | ServerMode::External => None,
        }
    }
}

/// Default number of GPU layers (`-ngl`): everything on GPU.
pub const DEFAULT_GPU_LAYERS: i32 = 99;
/// Default context size (`-c`).
pub const DEFAULT_CONTEXT_SIZE: u32 = 8192;

/// FlashAttention mode (`--flash-attn`) for the managed llama.cpp server. `Auto` —
/// the flag isn't passed (llama.cpp decides on its own, its default); `On`/`Off` — forced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FlashAttn {
    #[default]
    Auto,
    On,
    Off,
}

impl FlashAttn {
    /// All variants in UI-cycle order (for the Choice popup and the cycle).
    pub const ALL: [FlashAttn; 3] = [FlashAttn::Auto, FlashAttn::On, FlashAttn::Off];

    /// Value for `--flash-attn`; `None` (Auto) — don't pass the flag.
    pub fn as_arg(self) -> Option<&'static str> {
        match self {
            FlashAttn::Auto => None,
            FlashAttn::On => Some("on"),
            FlashAttn::Off => Some("off"),
        }
    }

    /// UI label (Choice field).
    pub fn label(self) -> &'static str {
        match self {
            FlashAttn::Auto => "auto",
            FlashAttn::On => "on",
            FlashAttn::Off => "off",
        }
    }

    /// Cyclic iteration honoring direction (`dir` = +1/-1).
    pub fn cycle(self, dir: i32) -> Self {
        let idx = Self::ALL.iter().position(|x| *x == self).unwrap_or(0) as i32;
        let n = Self::ALL.len() as i32;
        Self::ALL[(((idx + dir) % n + n) % n) as usize]
    }
}

/// Speculative-decoding type (`--spec-type`) for the managed llama.cpp server.
/// `None` — off (the flag isn't passed). `draft-*` types need a draft model
/// (`-md`) — for MTP models (e.g. `mtp-gemma-4-12B-it.gguf`) that's `draft-mtp`;
/// `ngram-*` need no separate model (the draft comes from the context history).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpecType {
    #[default]
    None,
    DraftSimple,
    DraftEagle3,
    DraftMtp,
    NgramSimple,
    NgramMapK,
    NgramMapK4v,
    NgramMod,
    NgramCache,
}

impl SpecType {
    /// All variants in UI-cycle order (for the Choice popup and the cycle).
    pub const ALL: [SpecType; 9] = [
        SpecType::None,
        SpecType::DraftSimple,
        SpecType::DraftEagle3,
        SpecType::DraftMtp,
        SpecType::NgramSimple,
        SpecType::NgramMapK,
        SpecType::NgramMapK4v,
        SpecType::NgramMod,
        SpecType::NgramCache,
    ];

    /// Value for `--spec-type`; `None` — don't pass the flag (off).
    pub fn as_arg(self) -> Option<&'static str> {
        match self {
            SpecType::None => Option::None,
            SpecType::DraftSimple => Some("draft-simple"),
            SpecType::DraftEagle3 => Some("draft-eagle3"),
            SpecType::DraftMtp => Some("draft-mtp"),
            SpecType::NgramSimple => Some("ngram-simple"),
            SpecType::NgramMapK => Some("ngram-map-k"),
            SpecType::NgramMapK4v => Some("ngram-map-k4v"),
            SpecType::NgramMod => Some("ngram-mod"),
            SpecType::NgramCache => Some("ngram-cache"),
        }
    }

    /// Whether the type needs a separate draft model (`-md`): only `draft-*`.
    pub fn needs_draft_model(self) -> bool {
        matches!(
            self,
            SpecType::DraftSimple | SpecType::DraftEagle3 | SpecType::DraftMtp
        )
    }

    /// UI label (Choice field).
    pub fn label(self) -> &'static str {
        self.as_arg().unwrap_or("none")
    }

    /// Cyclic iteration honoring direction (`dir` = +1/-1).
    pub fn cycle(self, dir: i32) -> Self {
        let idx = Self::ALL.iter().position(|x| *x == self).unwrap_or(0) as i32;
        let n = Self::ALL.len() as i32;
        Self::ALL[(((idx + dir) % n + n) % n) as usize]
    }
}

/// Settings for the local managed `llama-server` (llama.cpp): the app launches
/// it as a child process. Its own sub-section in every engine, so switching modes
/// doesn't lose these values. See docs/install.md §3.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ManagedSettings {
    /// Path to the `llama-server` binary.
    pub binary: Option<String>,
    /// Path to the GGUF model (`-m`).
    pub model_path: Option<String>,
    /// GPU layers (`-ngl`).
    pub gpu_layers: i32,
    /// Context size (`-c`).
    pub context_size: u32,
    /// Use the model's built-in chat template (`--jinja`) — needed for correct
    /// formatting and tool calling.
    pub jinja: bool,
    /// Reasoning format (`--reasoning-format`, e.g. `auto`); `None` — leave unset.
    pub reasoning_format: Option<String>,
    /// Don't use mmap when loading the model (`--no-mmap`): weights load into RAM
    /// entirely. Helps on network/slow disks. Off by default.
    pub no_mmap: bool,
    /// FlashAttention (`--flash-attn`): attention optimization. Default `Auto`.
    pub flash_attn: FlashAttn,
    /// Speculative-decoding type (`--spec-type`). Off by default.
    pub spec_type: SpecType,
    /// Draft model for speculative decoding (`-md`/`--model-draft`).
    /// Needed for `draft-*` types; for MTP models — the path to the matching GGUF.
    pub draft_model: Option<String>,
    /// Draft-model GPU layers (`-ngld`); `None` — auto (flag not passed).
    pub draft_gpu_layers: Option<i32>,
    /// How many tokens to draft per step (`--spec-draft-n-max`); `None` —
    /// llama.cpp's default (3).
    pub draft_n_max: Option<u32>,
    /// Minimum draft tokens per step (`--spec-draft-n-min`); `None` — default (0).
    pub draft_n_min: Option<u32>,
    /// Bind interface (`--host`), e.g. `127.0.0.1` or `0.0.0.0`.
    pub host: String,
    pub port: u16,
}

impl Default for ManagedSettings {
    fn default() -> Self {
        Self {
            binary: None,
            model_path: None,
            gpu_layers: DEFAULT_GPU_LAYERS,
            context_size: DEFAULT_CONTEXT_SIZE,
            jinja: true,
            reasoning_format: None,
            no_mmap: false,
            flash_attn: FlashAttn::default(),
            spec_type: SpecType::default(),
            draft_model: None,
            draft_gpu_layers: None,
            draft_n_max: None,
            draft_n_min: None,
            host: "127.0.0.1".to_string(),
            port: 8000,
        }
    }
}

/// External-mode settings: connecting to an already-running OpenAI-compatible
/// server (any: llama.cpp, vLLM, LM Studio…).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExternalSettings {
    /// Server URL (e.g. `http://127.0.0.1:8000/v1`).
    pub url: Option<String>,
    /// Model name for a multi-model server (optional).
    pub model_name: Option<String>,
    /// Env-variable name carrying a Bearer key (optional) — for an OpenAI-compatible
    /// proxy/gateway that requires authorization. Stores the **name**, not the secret
    /// (ADR 0004). `None`/empty — no authorization (a local `llama-server` doesn't need it).
    pub api_key_env: Option<String>,
}

/// Settings for a single cloud provider (OpenAI/Gemini/Claude). Stored
/// separately per provider so switching providers doesn't lose the other's values.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudSettings {
    /// Provider's model name (`gpt-4o`, `gemini-2.5-pro`, `claude-opus-4-8`).
    pub model_name: Option<String>,
    /// Env-variable name carrying the API key (e.g. `OPENAI_API_KEY`). Stores
    /// the **name**, not the secret itself — the key is read from the environment (ADR 0004).
    pub api_key_env: Option<String>,
    /// Override of the provider's base URL (optional); `None` — the provider's default.
    pub url: Option<String>,
}

/// Chat inference-server settings. A sub-section per mode/provider
/// (managed/external/openai/gemini/claude), so switching modes doesn't lose the
/// other's values. Transport — OpenAI-compatible HTTP (except Claude — Messages
/// API). See docs/install.md §3, ADR 0004.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineSettings {
    pub mode: ServerMode,
    pub managed: ManagedSettings,
    pub external: ExternalSettings,
    pub openai: CloudSettings,
    pub gemini: CloudSettings,
    pub claude: CloudSettings,
}

impl EngineSettings {
    /// Active provider's cloud settings (`None` — local managed/external).
    pub fn cloud(&self) -> Option<&CloudSettings> {
        cloud_ref(
            self.mode.cloud_provider(),
            &self.openai,
            &self.gemini,
            &self.claude,
        )
    }

    /// Active provider's mutable cloud settings (`None` — local).
    pub fn cloud_mut(&mut self) -> Option<&mut CloudSettings> {
        cloud_mut(
            self.mode.cloud_provider(),
            &mut self.openai,
            &mut self.gemini,
            &mut self.claude,
        )
    }

    /// Active model name for the current mode (for the `Message.metadata` snapshot
    /// and the feed caption). Managed — the GGUF's base name without the path/`.gguf`
    /// extension; external/cloud — the configured `model_name`. `None` if unset.
    pub fn active_model_name(&self) -> Option<String> {
        match self.mode {
            ServerMode::Managed => self.managed.model_path.as_deref().and_then(|p| {
                let name = p.rsplit(['/', '\\']).next().unwrap_or(p);
                let name = name.trim_end_matches(".gguf");
                (!name.is_empty()).then(|| name.to_string())
            }),
            ServerMode::External => self.external.model_name.clone().filter(|m| !m.is_empty()),
            ServerMode::OpenAi | ServerMode::Gemini | ServerMode::Claude => self
                .cloud()
                .and_then(|c| c.model_name.clone())
                .filter(|m| !m.is_empty()),
        }
    }
}

/// Active cloud sub-structure by provider (a shared helper for all engines).
fn cloud_ref<'a>(
    provider: Option<CloudProvider>,
    openai: &'a CloudSettings,
    gemini: &'a CloudSettings,
    claude: &'a CloudSettings,
) -> Option<&'a CloudSettings> {
    match provider? {
        CloudProvider::OpenAi => Some(openai),
        CloudProvider::Gemini => Some(gemini),
        CloudProvider::Claude => Some(claude),
    }
}

fn cloud_mut<'a>(
    provider: Option<CloudProvider>,
    openai: &'a mut CloudSettings,
    gemini: &'a mut CloudSettings,
    claude: &'a mut CloudSettings,
) -> Option<&'a mut CloudSettings> {
    match provider? {
        CloudProvider::OpenAi => Some(openai),
        CloudProvider::Gemini => Some(gemini),
        CloudProvider::Claude => Some(claude),
    }
}

/// Impersonation-server mode (writing a message on the user's behalf).
/// Differs from [`ServerMode`] by a third variant, `Shared`. See spec §11.8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImpersonationMode {
    /// Use the same engine as for assistant replies (any mode), but
    /// with sampling from the "Impersonation" subsection.
    #[default]
    Shared,
    /// Bring up a separate child `llama-server` process.
    Managed,
    /// Connect to a separate remote OpenAI-compatible server.
    External,
    /// OpenAI cloud (separate from the assistant).
    #[serde(rename = "openai")]
    OpenAi,
    /// Google Gemini cloud via an OpenAI-compatible endpoint.
    Gemini,
    /// Anthropic cloud (Claude, Messages API).
    Claude,
}

impl ImpersonationMode {
    /// Cloud provider for this mode (`None` — shared/managed/external).
    pub fn cloud_provider(self) -> Option<CloudProvider> {
        match self {
            ImpersonationMode::OpenAi => Some(CloudProvider::OpenAi),
            ImpersonationMode::Gemini => Some(CloudProvider::Gemini),
            ImpersonationMode::Claude => Some(CloudProvider::Claude),
            ImpersonationMode::Shared
            | ImpersonationMode::Managed
            | ImpersonationMode::External => None,
        }
    }
}

/// Default port for the managed impersonation server (a separate instance).
pub const DEFAULT_IMPERSONATION_PORT: u16 = 8002;

/// Impersonation-server settings. Sub-sections are identical to [`EngineSettings`],
/// but the mode is [`ImpersonationMode`] (adds `shared`). In `shared` mode the
/// sub-sections aren't used — the assistant's chat server is used instead. See spec §11.8.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImpersonationEngineSettings {
    pub mode: ImpersonationMode,
    pub managed: ManagedSettings,
    pub external: ExternalSettings,
    pub openai: CloudSettings,
    pub gemini: CloudSettings,
    pub claude: CloudSettings,
}

impl Default for ImpersonationEngineSettings {
    fn default() -> Self {
        Self {
            mode: ImpersonationMode::Shared,
            // A separate managed impersonation instance listens on its own port.
            managed: ManagedSettings {
                port: DEFAULT_IMPERSONATION_PORT,
                ..Default::default()
            },
            external: ExternalSettings::default(),
            openai: CloudSettings::default(),
            gemini: CloudSettings::default(),
            claude: CloudSettings::default(),
        }
    }
}

impl ImpersonationEngineSettings {
    /// Active provider's cloud settings (`None` — shared/managed/external).
    pub fn cloud(&self) -> Option<&CloudSettings> {
        cloud_ref(
            self.mode.cloud_provider(),
            &self.openai,
            &self.gemini,
            &self.claude,
        )
    }

    /// Active provider's mutable cloud settings (`None` — local).
    pub fn cloud_mut(&mut self) -> Option<&mut CloudSettings> {
        cloud_mut(
            self.mode.cloud_provider(),
            &mut self.openai,
            &mut self.gemini,
            &mut self.claude,
        )
    }
}

/// Default embedding-server port.
pub const DEFAULT_EMBED_PORT: u16 = 8001;

/// Managed embedding-server settings: the same `llama-server` with `--embeddings`.
/// The embedding server has no chat-template/host/no_mmap — the supervisor
/// fixes those, hence fewer fields than [`ManagedSettings`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ManagedEmbedSettings {
    /// Path to the `llama-server` binary.
    pub binary: Option<String>,
    /// Path to the GGUF embedding model (`-m`).
    pub model_path: Option<String>,
    /// GPU layers (`-ngl`).
    pub gpu_layers: i32,
    pub port: u16,
}

impl Default for ManagedEmbedSettings {
    fn default() -> Self {
        Self {
            binary: None,
            model_path: None,
            gpu_layers: DEFAULT_GPU_LAYERS,
            port: DEFAULT_EMBED_PORT,
        }
    }
}

/// Settings for the dedicated embedding server used by RAG (ADR 0002). A separate
/// process/port; when unconfigured (`UnavailableEmbedder`) — RAG returns an error.
/// A sub-section per mode/provider (like [`EngineSettings`]); cloud embeddings
/// exist for OpenAI/Gemini (not Anthropic — RAG turns off). See ADR 0004.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedSettings {
    pub mode: ServerMode,
    pub managed: ManagedEmbedSettings,
    pub external: ExternalSettings,
    pub openai: CloudSettings,
    pub gemini: CloudSettings,
    pub claude: CloudSettings,
}

impl EmbedSettings {
    /// Active provider's cloud settings (`None` — local managed/external).
    pub fn cloud(&self) -> Option<&CloudSettings> {
        cloud_ref(
            self.mode.cloud_provider(),
            &self.openai,
            &self.gemini,
            &self.claude,
        )
    }

    /// Active provider's mutable cloud settings (`None` — local).
    pub fn cloud_mut(&mut self) -> Option<&mut CloudSettings> {
        cloud_mut(
            self.mode.cloud_provider(),
            &mut self.openai,
            &mut self.gemini,
            &mut self.claude,
        )
    }

    /// Active embedding model's name for the current mode — **display metadata**
    /// for the "the embedding model changed" message (see
    /// [`crate::shared::embed_identity`]). Mirrors
    /// [`EngineSettings::active_model_name`]; managed embeddings have no
    /// `model_name` field, so the name comes from the GGUF path.
    ///
    /// Never used to *decide* whether the model changed — the canary vector does
    /// that. A name is too easy to leave stale: an external server picks the
    /// model itself, and the same path can come to point at a different file.
    pub fn active_model_name(&self) -> Option<String> {
        match self.mode {
            ServerMode::Managed => self.managed.model_path.as_deref().and_then(|p| {
                let name = p.rsplit(['/', '\\']).next().unwrap_or(p);
                let name = name.trim_end_matches(".gguf");
                (!name.is_empty()).then(|| name.to_string())
            }),
            ServerMode::External => self.external.model_name.clone().filter(|m| !m.is_empty()),
            ServerMode::OpenAi | ServerMode::Gemini | ServerMode::Claude => self
                .cloud()
                .and_then(|c| c.model_name.clone())
                .filter(|m| !m.is_empty()),
        }
    }
}

/// Default sub-agent response token limit (`call_subagent`, spec §9.3.2).
pub const DEFAULT_SUBAGENT_MAX_TOKENS: usize = 1024;
/// Default time limit for one sub-agent call (seconds).
pub const DEFAULT_SUBAGENT_TIMEOUT_SECS: u64 = 60;
/// Default code-execution timeout in the Wasmer sandbox (seconds). More generous
/// than the local interpreter's (10s): WASM interpretation is ~2–5× slower than native.
/// See docs/research/python-wasmer-sandbox.md.
pub const DEFAULT_PYTHON_WASM_TIMEOUT_SECS: u64 = 30;

/// `python_exec` execution mode: an isolated Wasmer/WASIX sandbox (by
/// default — no access to machine files, pre-installed packages) or the local
/// system interpreter (previous behavior). See
/// docs/research/python-wasmer-sandbox.md (Phase 0 → the `wasmer` sidecar).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PythonMode {
    /// An isolated sandbox on the bundled `wasmer` (sidecar). Default.
    #[default]
    Wasmer,
    /// The local system interpreter (`python`/`python3`), no isolation.
    Local,
}

impl PythonMode {
    /// All variants in UI-cycle order (for the Choice popup and the cycle).
    pub const ALL: [PythonMode; 2] = [PythonMode::Wasmer, PythonMode::Local];

    /// Cyclic iteration honoring direction (`dir` = +1/-1).
    pub fn cycle(self, dir: i32) -> Self {
        let idx = Self::ALL.iter().position(|x| *x == self).unwrap_or(0) as i32;
        let n = Self::ALL.len() as i32;
        Self::ALL[(((idx + dir) % n + n) % n) as usize]
    }
}

// UI label — `screens::settings::helpers::python_mode_label` (interface language,
// axis B; mirrors `theme_label` for `Theme` — kept next to the other settings-screen
// label helpers rather than on the type itself).

/// Global "master switches" for external tools (security/privacy,
/// spec §9.4, §13.2). Effective set = `Profile.enabled_tools ∩ globally enabled`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolSettings {
    /// Web search (DuckDuckGo). Enabled by default (read-only).
    pub web_enabled: bool,
    /// Fetch result pages, extract text, and reorder by
    /// relevance (`web_search`, spec §9.3.1). Enabled by default; gives the model
    /// page content, but adds latency (fetching up to `max_results` pages).
    /// The call's `fetch_content` argument overrides this value.
    pub web_fetch_content: bool,
    /// Python execution. Off by default (the tool's master gate).
    pub python_enabled: bool,
    /// `python_exec` execution mode: the Wasmer sandbox (default) or the local
    /// interpreter. See [`PythonMode`].
    pub python_mode: PythonMode,
    /// Path to the Python interpreter (`None` → system `python3`/`python`).
    /// Used only in [`PythonMode::Local`] mode.
    pub python_path: Option<String>,
    /// Allow network access inside the Wasmer sandbox (`--net`). Enabled by default
    /// — the main value of a pre-installed `requests`; but code in the sandbox will
    /// be able to reach the network. [`PythonMode::Local`] mode doesn't use this field
    /// (network is always available there). See docs/research/python-wasmer-sandbox.md §7.
    pub python_net_enabled: bool,
    /// Execution timeout in the Wasmer sandbox (seconds). Local mode keeps its own
    /// (shorter) timeout. See [`DEFAULT_PYTHON_WASM_TIMEOUT_SECS`].
    pub python_wasm_timeout_secs: u64,
    /// Hard OS-level memory limit for the Wasmer sandbox (MB; `None`/0 — no limit).
    /// Protects the host from OOM on a runaway script: exceeding it kills the
    /// `wasmer` process (not graceful — V8 fails with "Fatal out of memory"). **Windows
    /// only** (Job Object); not applied on Unix (rlimit is unreliable with V8, ADR
    /// 0005). Minimum ~1024 (less — the sandbox may fail to start: V8+CPython
    /// needs ~768 MB). Unlimited by default (defense in depth on top of the timeout
    /// and wasm32's ~4 GB).
    pub python_wasm_memory_mb: Option<u64>,
    /// Access to local files (`fs_read`/`fs_write`/`fs_list`). Off by
    /// default (the tool can read/overwrite any file — privacy/
    /// security, like Python). See spec §9.3, §13.2.
    pub fs_enabled: bool,
    /// "Sandbox" directory for file tools (`None` → no restriction).
    /// If set, all paths must lie inside it (protection against `..` escape).
    pub fs_root: Option<String>,
    /// Sub-agent response token limit (`call_subagent`).
    pub subagent_max_tokens: usize,
    /// Time limit for a sub-agent call (seconds).
    pub subagent_timeout_secs: u64,
}

impl Default for ToolSettings {
    fn default() -> Self {
        Self {
            web_enabled: true,
            web_fetch_content: true,
            python_enabled: false,
            python_mode: PythonMode::default(),
            python_path: None,
            python_net_enabled: true,
            python_wasm_timeout_secs: DEFAULT_PYTHON_WASM_TIMEOUT_SECS,
            python_wasm_memory_mb: None,
            fs_enabled: false,
            fs_root: None,
            subagent_max_tokens: DEFAULT_SUBAGENT_MAX_TOKENS,
            subagent_timeout_secs: DEFAULT_SUBAGENT_TIMEOUT_SECS,
        }
    }
}

/// Default target ("soft") RAG chunk size in characters.
pub const DEFAULT_CHUNK_TARGET_CHARS: usize = 800;
/// Default overlap between adjacent RAG chunks in characters.
pub const DEFAULT_CHUNK_OVERLAP_CHARS: usize = 150;
/// Default hard ceiling for an indivisible RAG chunk run in characters.
pub const DEFAULT_CHUNK_MAX_CHARS: usize = 1200;

/// Knowledge-base (RAG) chunking settings. Affect slicing at indexing time
/// (`/rag add`, the `rag_add` tool) and reindexing (`/rag rebuild`). Sizes are in
/// characters (not bytes — correct for Cyrillic/Unicode). See spec §9.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RagSettings {
    /// Target ("soft") chunk size: units are packed up to it.
    pub chunk_target_chars: usize,
    /// Overlap between adjacent chunks: the previous one's tail repeats at the start of the next.
    pub chunk_overlap_chars: usize,
    /// Hard ceiling for an indivisible run (a very long word/line with no punctuation).
    pub chunk_max_chars: usize,
}

impl Default for RagSettings {
    fn default() -> Self {
        Self {
            chunk_target_chars: DEFAULT_CHUNK_TARGET_CHARS,
            chunk_overlap_chars: DEFAULT_CHUNK_OVERLAP_CHARS,
            chunk_max_chars: DEFAULT_CHUNK_MAX_CHARS,
        }
    }
}

/// Default per-file inline budget for chat attachments, in estimated tokens.
/// Above it the file switches to by-reference mode (metadata + excerpt) instead
/// of being refused. Conservative on purpose: silently overflowing an 8k local
/// model's context is a far worse failure than showing an excerpt.
pub const DEFAULT_ATTACH_MAX_FILE_TOKENS: usize = 4000;
/// Default total inline budget for one chat's attachments, in estimated tokens.
pub const DEFAULT_ATTACH_MAX_TOTAL_TOKENS: usize = 8000;
/// Default excerpt size shown for a by-reference attachment, in estimated tokens.
pub const DEFAULT_ATTACH_EXCERPT_TOKENS: usize = 300;
/// Default page size for `attachment_read`, in estimated tokens. Big enough to be
/// worth a round trip, small enough that a few pages don't blow an 8k context.
pub const DEFAULT_ATTACH_PAGE_TOKENS: usize = 1500;

/// Chat file-attachment settings (`/file attach`, docs/file-attachments.md).
/// Budgets are in **estimated tokens** (`shared::tokens`) — characters mislead
/// across scripts, and tokens are what the status bar shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AttachmentSettings {
    /// Per-file inline budget: a bigger file goes by reference.
    pub max_file_tokens: usize,
    /// Total inline budget for one chat: once it is used up, further files go
    /// by reference (already-attached ones are never demoted).
    pub max_total_tokens: usize,
    /// How much of a by-reference file's head to show in the pinned block.
    pub excerpt_tokens: usize,
    /// Page size for the `attachment_read` tool — how much of a by-reference
    /// file one call returns.
    pub page_tokens: usize,
}

impl Default for AttachmentSettings {
    fn default() -> Self {
        Self {
            max_file_tokens: DEFAULT_ATTACH_MAX_FILE_TOKENS,
            max_total_tokens: DEFAULT_ATTACH_MAX_TOTAL_TOKENS,
            excerpt_tokens: DEFAULT_ATTACH_EXCERPT_TOKENS,
            page_tokens: DEFAULT_ATTACH_PAGE_TOKENS,
        }
    }
}

/// Default storage ceiling for the self-model narrative (insights).
pub const DEFAULT_SELF_MODEL_MAX_NARRATIVE: usize = 50;
/// Default number of fresh insights to inject into the system prompt.
pub const DEFAULT_SELF_MODEL_NARRATIVE_IN_PROMPT: usize = 3;
/// Default character ceiling for rendering the self-model into the system prompt.
pub const DEFAULT_SELF_MODEL_PROMPT_CAP: usize = 1200;
/// Whether to inject a persona-neutral "self-model maintenance protocol" by default.
pub const DEFAULT_SELF_MODEL_MAINTENANCE_PROTOCOL: bool = true;
/// Default number of closed goals to keep in the structure (the oldest beyond this
/// are folded into a narrative scar and removed — the closed-goal ceiling).
pub const DEFAULT_SELF_MODEL_MAX_CLOSED_GOALS: usize = 10;
/// Default target size of the self-description (summary) in characters. Beyond it
/// tools and the maintenance protocol softly suggest shortening the description (this
/// is a gate, not a ceiling — data isn't truncated). See docs/summary-as-snapshot.md (stage 2).
pub const DEFAULT_SELF_MODEL_SUMMARY_TARGET: usize = 1000;

/// Self-model settings (SelfModel): narrative sizes and the injection volume into
/// the system prompt. See [docs/history/self-model-mvp.md] and spec §9.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SelfModelSettings {
    /// How many insights to keep in the narrative (old ones are evicted on add).
    pub max_narrative: usize,
    /// How many fresh insights to inject into the system prompt.
    pub narrative_in_prompt: usize,
    /// Character ceiling for the compact model render injected into the system prompt.
    pub prompt_cap: usize,
    /// How many closed (completed/no-longer-relevant) goals to keep in the structure;
    /// the oldest beyond this are folded into a narrative scar and removed.
    pub max_closed_goals: usize,
    /// Target size of the self-description (summary) in characters: beyond it, tools and
    /// the maintenance protocol softly suggest shortening the description. A gate, not a
    /// ceiling — data isn't truncated. See docs/summary-as-snapshot.md (stage 2).
    pub summary_target_chars: usize,
    /// Auto-reflection: run background reflection every N assistant replies in a
    /// chat (the model updates the "self-model" itself). `0` — off (default).
    /// Fires only in profiles with self-model tools enabled.
    pub auto_reflect_every: usize,
    /// Self-model auto-consolidation ("sleep"): run background consolidation every
    /// N assistant replies in a chat (the model merges duplicate observations, shrinks
    /// a bloated description, links contradictions on its own). `0` — off (default).
    /// Separate from `auto_reflect_every` (its own toggle is more precise — self-model
    /// and notes gates/data are already kept apart). Fires only in profiles with
    /// self-model tools enabled. See docs/history/self-model-consolidation.md (stage A1).
    pub auto_consolidate_every: usize,
    /// Whether to inject a persona-neutral "self-model maintenance protocol" into the
    /// system prompt (when to record changes, "transient — into observations",
    /// "accuracy over agreeableness"). Stabilizes tool usage independent of the
    /// profile's persona. On by default; applies only when the profile has enabled
    /// self-model tools.
    pub maintenance_protocol: bool,
}

impl Default for SelfModelSettings {
    fn default() -> Self {
        Self {
            max_narrative: DEFAULT_SELF_MODEL_MAX_NARRATIVE,
            narrative_in_prompt: DEFAULT_SELF_MODEL_NARRATIVE_IN_PROMPT,
            prompt_cap: DEFAULT_SELF_MODEL_PROMPT_CAP,
            max_closed_goals: DEFAULT_SELF_MODEL_MAX_CLOSED_GOALS,
            summary_target_chars: DEFAULT_SELF_MODEL_SUMMARY_TARGET,
            auto_reflect_every: 0,
            auto_consolidate_every: 0,
            maintenance_protocol: DEFAULT_SELF_MODEL_MAINTENANCE_PROTOCOL,
        }
    }
}

/// Notes settings: auto-consolidation ("sleep"). See docs/history/notes-connectivity.md (Tier 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NotesSettings {
    /// Auto-consolidation: run background "sleep" consolidation every N assistant
    /// replies in a chat (the model merges duplicates / rewrites stale ones /
    /// links related ones on its own). `0` — off (default). Fires only in
    /// profiles with note tools enabled.
    pub auto_consolidate_every: usize,
    /// Whether to show "about self" (`@self`) observations in general `note_recall` —
    /// marked `[about self]`. **Off** by default: memory about oneself ≠ memory about
    /// the interlocutor (Tier 1 decision). The toggle gives "full output mixing"
    /// (Tier 3, Path 2) to validate whether that's safe; when off, self-notes stay
    /// hidden, as before. See docs/history/narrative-as-notes.md (Tier 3, Path 2).
    pub recall_includes_self: bool,
}

/// TUI theme. Real application in widgets — at M9 (`shared/theme.rs`);
/// here it just stores the user's choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Follow the system setting (default).
    #[default]
    Auto,
    Dark,
    Light,
}

/// Interface settings (theme, spellcheck, dictionaries). See spec §11.6.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InterfaceSettings {
    pub theme: Theme,
    /// Whether input spellcheck is enabled (dictionaries load from `dictionaries/`).
    pub spellcheck_enabled: bool,
    /// Base names of the selected dictionaries (e.g. `en_US`, `ru_RU`). Empty —
    /// use all found in the directory.
    pub selected_dictionaries: Vec<String>,
    /// Ask for confirmation before regenerating (`Ctrl+R`) and deleting the
    /// last exchange (`Ctrl+E`) — both operations are irreversible in the UI. Off
    /// by default (the shortcuts fire immediately). See spec §11.7.
    pub confirm_destructive_keys: bool,
    /// Compatibility mode for old terminal emulators (conhost Windows 10
    /// and the like): instead of emoji and rare Unicode characters — glyphs from a
    /// safe set (WGL4/ASCII), straight borders instead of rounded, an ASCII spinner,
    /// dimming popup backgrounds by color instead of `DIM`. Off by default.
    /// See spec §11.6 and [`crate::shared::theme::GlyphSet`].
    pub terminal_compat: bool,
    /// Horizontal separators between Markdown-table rows in the feed
    /// (`├───┼───┤`, a "grid" look). Off by default (a compact look — a separator
    /// only below the header); enabling it gives a "grid" look. See spec §11.4.
    pub table_row_separators: bool,
    /// Render ```mermaid blocks in the feed as a diagram (Unicode/ASCII graphics,
    /// the `mermaid-text` crate) instead of the source. Only flowchart/sequence
    /// (whitelist); on any failure (didn't parse / didn't fit the width / a type
    /// outside the whitelist) — a hard fallback to the source as a code block, same
    /// as with the toggle off. Thanks to the fallback, **on** by default (the worst
    /// case = prior behavior). See spec §11.4 and
    /// docs/research/mermaid-ascii-rendering.md.
    pub render_mermaid: bool,
    /// **Interface** language (axis B, docs/i18n-ui.md) — text for the human
    /// (status bar, settings, help, feed role headers). **Independent** of the
    /// agent language (`Profile.language`, axis A): a Russian UI + English-speaking
    /// agents is a legitimate combination. Defaults to `Ru` (an old `settings.json`
    /// with no field); on a fresh install — from `defaults.json` (`main.rs`). Reuses
    /// `i18n::Lang` (the UI language is "which bundle to read `ui.*` keys from").
    pub language: crate::shared::i18n::Lang,
}

impl Default for InterfaceSettings {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            spellcheck_enabled: true,
            selected_dictionaries: Vec::new(),
            confirm_destructive_keys: false,
            terminal_compat: false,
            table_row_separators: false,
            render_mermaid: true,
            language: crate::shared::i18n::Lang::default(),
        }
    }
}

/// Default timeout for one MCP-server tool call (seconds).
pub const DEFAULT_MCP_TOOL_TIMEOUT_SECS: u64 = 60;
/// Default character ceiling for an MCP-tool result (clipping prompt input — a
/// precedent from Claude Code: a cap of ~25k tokens). See docs/research/plugin-system.md §4.4.
pub const DEFAULT_MCP_MAX_RESULT_CHARS: usize = 20_000;

/// Configuration of a single MCP server (a stdio subprocess,
/// docs/research/plugin-system.md §4.4). Servers are added by editing
/// `settings.json` (decision point R6); the settings UI shows statuses and toggles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServerConfig {
    /// Short identifier (slug `[a-z0-9-]`, ≤32) — part of the tool id
    /// `mcp__<id>__<tool>`. Empty/invalid — the server doesn't start.
    pub id: String,
    /// The launch command. `.bat`/`.cmd` are forbidden (BatBadBut, CVE-2024-24576);
    /// `npx` servers on Windows — `cmd /c npx …` or a direct exe path.
    pub command: String,
    /// Command arguments.
    pub args: Vec<String>,
    /// Child environment: variable → **name** of the source variable in the app's
    /// own environment (the secret itself isn't written to `settings.json` — the
    /// `api_key_env` precedent, decision point R8). A missing source — a warn in the log, skipped.
    pub env: std::collections::BTreeMap<String, String>,
    /// Whether the server is enabled (a disabled one doesn't start, its tools are unavailable).
    pub enabled: bool,
    /// Timeout for one tool call (seconds). The startup handshake keeps its own
    /// timeout (a client constant).
    pub tool_timeout_secs: u64,
    /// Tool-result clip (characters) — a limit on prompt input.
    pub max_result_chars: usize,
    /// TOFU pin of the tool catalog (sha256 over names+descriptions+schemas): set
    /// automatically on the server's first startup; on a **catalog change**
    /// (a rug-pull detector, tool poisoning), tools aren't registered until
    /// the user reconfirms the new catalog in settings. Written by the
    /// app (not editable in the UI); manually removing the field = resetting trust.
    /// See docs/research/plugin-system.md §4.5 (R7).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned_catalog: Option<String>,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            command: String::new(),
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
            enabled: true,
            tool_timeout_secs: DEFAULT_MCP_TOOL_TIMEOUT_SECS,
            max_result_chars: DEFAULT_MCP_MAX_RESULT_CHARS,
            pinned_catalog: None,
        }
    }
}

/// MCP-host settings (plugin tools, docs/research/plugin-system.md §4).
/// The master switch is **off by default** (like Python): an MCP server is an
/// arbitrary user-privileged program; enabling it is a deliberate opt-in,
/// and tools are additionally opt-in per profile (double opt-in, decision point R7).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct McpSettings {
    /// MCP-host master switch.
    pub enabled: bool,
    /// List of servers (edited in `settings.json`).
    pub servers: Vec<McpServerConfig>,
}

/// Default OpenAI speech model (the current dedicated TTS; `tts-1*` is
/// legacy). See docs/research/tts.md §3.1.
pub const DEFAULT_TTS_OPENAI_MODEL: &str = "gpt-4o-mini-tts";
/// Default OpenAI voice. `onyx` — a deep male voice, **verified by a live spike**
/// (docs/research/tts.md §13.9): correct stress, no drift; `marin`/`cedar` are
/// recommended expressive alternatives (not auditioned in the spike).
pub const DEFAULT_TTS_OPENAI_VOICE: &str = "onyx";
/// Default Gemini speech model (Gemini has no GA TTS models — all are
/// preview; we take flash: cheaper and has a free tier). See docs/research/tts.md §3.2.
pub const DEFAULT_TTS_GEMINI_MODEL: &str = "gemini-2.5-flash-preview-tts";
/// Default Gemini voice (from 30 prebuilt voices).
pub const DEFAULT_TTS_GEMINI_VOICE: &str = "Kore";

/// Speech (TTS) mode — an independent "server slot", like embeddings
/// (ADR 0002): Anthropic has no TTS at all, so the speech provider is
/// configured separately from the chat engine. A local engine (managed sidecar) is
/// **future work**: a live spike (docs/research/tts.md §13) showed local engines are
/// either NO-GO for Russian (Qwen3-TTS/Supertonic 3) or need their own Rust
/// frontend (vosk-tts); the primary path is the cloud (OpenAI), external for offline.
/// See docs/research/tts.md §8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TtsMode {
    /// OpenAI cloud (`POST /v1/audio/speech`).
    #[default]
    OpenAi,
    /// Google Gemini cloud (native `generateContent` with `responseModalities:["AUDIO"]`).
    Gemini,
    /// Any local/third-party OpenAI-compatible TTS server (Kokoro-FastAPI,
    /// speaches, LocalAI, …). See docs/research/tts.md §3.4.
    External,
}

impl TtsMode {
    /// All variants in UI-cycle order (Choice field).
    pub const ALL: [TtsMode; 3] = [TtsMode::OpenAi, TtsMode::Gemini, TtsMode::External];

    /// UI label (Choice field).
    pub fn label(self) -> &'static str {
        match self {
            TtsMode::OpenAi => "openai",
            TtsMode::Gemini => "gemini",
            TtsMode::External => "external",
        }
    }

    /// The mode's cloud provider (`None` — external). Indexes the shared
    /// stored API key (ADR 0008): a key entered for chat is available to TTS too.
    pub fn cloud_provider(self) -> Option<CloudProvider> {
        match self {
            TtsMode::OpenAi => Some(CloudProvider::OpenAi),
            TtsMode::Gemini => Some(CloudProvider::Gemini),
            TtsMode::External => None,
        }
    }

    /// Cyclic iteration honoring direction (`dir` = +1/-1).
    pub fn cycle(self, dir: i32) -> Self {
        let idx = Self::ALL.iter().position(|x| *x == self).unwrap_or(0) as i32;
        let n = Self::ALL.len() as i32;
        Self::ALL[(((idx + dir) % n + n) % n) as usize]
    }
}

/// Cloud speech-provider settings (OpenAI/Gemini). Stored separately
/// per provider so switching modes doesn't lose the other's values (like
/// [`CloudSettings`] for the engine).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TtsCloudSettings {
    /// The provider's TTS model name.
    pub model_name: Option<String>,
    /// The assistant's voice (names differ per provider).
    pub voice: Option<String>,
    /// The **user's** voice for multi-message speech (`/tts all`, `/tts N`):
    /// when set, user turns are read in it, and the assistant's — in `voice`.
    /// `None` → all turns in one voice `voice` (the default behavior).
    pub user_voice: Option<String>,
    /// Natural-language instructions on tone/language/speed. For OpenAI this is
    /// the `instructions` field (and the only working way to set speed —
    /// `speed` is de facto ignored by `gpt-4o-mini-tts`); for Gemini — a prefix
    /// to the request text. See docs/research/tts.md §3.
    pub instructions: Option<String>,
    /// Env-variable name carrying the API key (a fallback if no key was entered in settings).
    pub api_key_env: Option<String>,
    /// Override of the provider's base URL (optional).
    pub url: Option<String>,
}

/// Settings for an external (local/third-party) OpenAI-compatible TTS server.
/// The common-denominator parameter set for such servers is `model`+`input`+`voice`+
/// `response_format`+`speed`, where `voice` differs per server and many
/// ignore `model` → we send only what's set. See docs/research/tts.md §3.4.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TtsExternalSettings {
    /// Server URL (e.g. `http://127.0.0.1:8880/v1`).
    pub url: Option<String>,
    /// Model name (optional — many servers ignore it).
    pub model_name: Option<String>,
    /// Assistant's voice — a free-text field (names depend on the server).
    pub voice: Option<String>,
    /// The **user's** voice for multi-message speech (see the identically-named field
    /// [`TtsCloudSettings::user_voice`]). `None` → one voice `voice`.
    pub user_voice: Option<String>,
    /// Env-variable name carrying a Bearer key (optional; a local server doesn't need it).
    pub api_key_env: Option<String>,
}

/// Settings for speaking chat messages aloud (the `/tts` command, spec §11.9).
/// All via `#[serde(default)]` — old `settings.json` files read without migration.
/// See docs/research/tts.md §8.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TtsSettings {
    /// Speech provider.
    pub mode: TtsMode,
    pub openai: TtsCloudSettings,
    pub gemini: TtsCloudSettings,
    pub external: TtsExternalSettings,
    /// Speech speed (where supported). Ignored by `gpt-4o-mini-tts` — there
    /// speed is requested via words in `instructions`.
    pub speed: f32,
    /// Speak role prefixes ("User."/"Assistant.") — in **all**
    /// command variants, including a bare `/tts` (user's decision, R6).
    pub speak_roles: bool,
    /// Stop speech when switching chats.
    pub stop_on_chat_switch: bool,
    /// Stop speech when a reply starts generating.
    pub stop_on_generation_start: bool,
}

impl Default for TtsSettings {
    fn default() -> Self {
        Self {
            mode: TtsMode::default(),
            openai: TtsCloudSettings {
                model_name: Some(DEFAULT_TTS_OPENAI_MODEL.into()),
                voice: Some(DEFAULT_TTS_OPENAI_VOICE.into()),
                ..Default::default()
            },
            gemini: TtsCloudSettings {
                model_name: Some(DEFAULT_TTS_GEMINI_MODEL.into()),
                voice: Some(DEFAULT_TTS_GEMINI_VOICE.into()),
                ..Default::default()
            },
            external: TtsExternalSettings::default(),
            speed: 1.0,
            speak_roles: false,
            // Stop on chat switch — yes; on generation start — no
            // (user's decision, R8).
            stop_on_chat_switch: true,
            stop_on_generation_start: false,
        }
    }
}

impl TtsSettings {
    /// Active cloud provider's settings (`None` — external).
    pub fn cloud(&self) -> Option<&TtsCloudSettings> {
        match self.mode.cloud_provider()? {
            CloudProvider::OpenAi => Some(&self.openai),
            CloudProvider::Gemini => Some(&self.gemini),
            CloudProvider::Claude => None,
        }
    }

    /// Active mode's voices: `(assistant, user)`. Blank fields → `None`.
    /// The user's voice is used by multi-message speech (`/tts all`); at
    /// `None`, user turns are read in the assistant's voice.
    pub fn active_voices(&self) -> (Option<&str>, Option<&str>) {
        fn nonblank(v: &Option<String>) -> Option<&str> {
            v.as_deref().map(str::trim).filter(|s| !s.is_empty())
        }
        let (voice, user) = match self.cloud() {
            Some(c) => (&c.voice, &c.user_voice),
            None => (&self.external.voice, &self.external.user_voice),
        };
        (nonblank(voice), nonblank(user))
    }

    /// Active cloud provider's mutable settings (`None` — external).
    pub fn cloud_mut(&mut self) -> Option<&mut TtsCloudSettings> {
        match self.mode.cloud_provider()? {
            CloudProvider::OpenAi => Some(&mut self.openai),
            CloudProvider::Gemini => Some(&mut self.gemini),
            CloudProvider::Claude => None,
        }
    }
}

/// What to include when copying the whole chat conversation to the clipboard (`F5`,
/// spec §11.2). By default only message text is copied (`Default` — all flags
/// `false`); optionally "thoughts" (CoT), tool-call parameters
/// (name + arguments), and their results are added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CopySettings {
    /// Include the assistant's "thoughts" (CoT) block.
    pub copy_thoughts: bool,
    /// Include tool-call parameters (tool name + arguments).
    pub copy_tool_calls: bool,
    /// Include tool-call results (responses).
    pub copy_tool_results: bool,
}

/// An impersonation profile — the **user** persona the model writes a reply on
/// behalf of (`Ctrl+U`, spec §11.8): its own name and system message. Assistant
/// profiles reference one by id ([`crate::entities::profile::Profile::impersonation_profile_id`]);
/// an unset/dangling reference falls back to the shared default text.
///
/// Lives in `settings.json` (not `profiles.json`) — impersonation is configured
/// globally, next to its engine (`impersonation_engine`) and sampling
/// (`impersonation_sampling`); the list is edited on the settings screen through
/// the usual config-save path, with no separate storage artifact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImpersonationProfile {
    pub id: uuid::Uuid,
    pub name: String,
    /// The system message describing the user persona. Empty → the shared default.
    #[serde(default)]
    pub system_message: String,
}

impl ImpersonationProfile {
    /// A new impersonation profile with a fresh id.
    pub fn new(name: impl Into<String>, system_message: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            name: name.into(),
            system_message: system_message.into(),
        }
    }
}

/// Global application configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub schema_version: u32,
    pub default_sampling: SamplingConfig,
    /// Sampling for impersonation mode (writing a message on the user's behalf).
    /// Applies in every impersonation-server mode (including `shared`). See spec §11.8.
    pub impersonation_sampling: SamplingConfig,
    /// Chat inference-server settings (managed llama.cpp or any OpenAI external).
    pub engine: EngineSettings,
    /// Impersonation-server settings (shared/managed/external). See spec §11.8.
    pub impersonation_engine: ImpersonationEngineSettings,
    /// Impersonation profiles (the user personas) — each with its own name and
    /// system message; an assistant profile picks one by id. Empty → every
    /// impersonation uses the shared default text. See spec §11.8.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub impersonation_profiles: Vec<ImpersonationProfile>,
    /// Dedicated embedding-server settings (RAG, ADR 0002).
    pub embed: EmbedSettings,
    /// Round limit for the client-side agentic loop (spec §6.3).
    pub max_tool_rounds: u32,
    /// Global switches for external tools.
    pub tools: ToolSettings,
    /// Knowledge-base (RAG) chunking settings.
    pub rag: RagSettings,
    /// Chat file-attachment budgets (`/file attach`).
    pub attachments: AttachmentSettings,
    /// Self-model settings (narrative, prompt-injection volume).
    pub self_model: SelfModelSettings,
    /// Notes settings (auto-consolidation "sleep").
    pub notes: NotesSettings,
    /// Interface settings (theme, spellcheck, dictionaries).
    pub interface: InterfaceSettings,
    /// What to include when copying the chat conversation to the clipboard (`F5`).
    pub copy: CopySettings,
    /// MCP host: plugin tools via external MCP servers (stdio).
    pub mcp: McpSettings,
    /// Speaking chat messages aloud (the `/tts` command, spec §11.9).
    pub tts: TtsSettings,
    /// Last-open chat — restored on the next launch. Written by the
    /// orchestrator (not editable via the settings screen). `None` — no memory
    /// (first launch/the chat was deleted) → the most recent one opens.
    pub last_active_chat: Option<uuid::Uuid>,
    /// Stored API keys for cloud providers — **one entry per machine**,
    /// encrypted with a machine key (Windows DPAPI / Linux HKDF(machine-id)+AEAD).
    /// The config stays portable: a foreign entry won't decrypt (the key gets
    /// re-entered via its own entry), returning to the original machine re-reads its entry.
    /// Written by the orchestrator (`AppCommand::SetApiKey`), not editable in the UI —
    /// the settings screen sends the key itself, not this structure. See `shared::secrets`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub api_keys: Vec<crate::shared::secrets::ApiKeyEntry>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            default_sampling: SamplingConfig {
                max_tokens: Some(2048),
                thinking: Some(true),
                ..Default::default()
            },
            // Impersonation writes a short reply on the user's behalf — "thoughts"
            // would only eat the budget — a modest token limit.
            impersonation_sampling: SamplingConfig {
                max_tokens: Some(1024),
                thinking: Some(false),
                ..Default::default()
            },
            engine: EngineSettings::default(),
            impersonation_engine: ImpersonationEngineSettings::default(),
            impersonation_profiles: Vec::new(),
            embed: EmbedSettings::default(),
            max_tool_rounds: 8,
            tools: ToolSettings::default(),
            rag: RagSettings::default(),
            attachments: AttachmentSettings::default(),
            self_model: SelfModelSettings::default(),
            notes: NotesSettings::default(),
            interface: InterfaceSettings::default(),
            copy: CopySettings::default(),
            mcp: McpSettings::default(),
            tts: TtsSettings::default(),
            last_active_chat: None,
            api_keys: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_current_schema_version() {
        assert_eq!(AppConfig::default().schema_version, SCHEMA_VERSION);
        assert_eq!(AppConfig::default().max_tool_rounds, 8);
    }

    #[test]
    fn tts_active_voices_reads_mode_and_treats_blank_as_unset() {
        let mut tts = TtsSettings {
            mode: TtsMode::OpenAi,
            openai: TtsCloudSettings {
                voice: Some("onyx".into()),
                user_voice: Some("nova".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(tts.active_voices(), (Some("onyx"), Some("nova")));
        // Blank user voice → None (doesn't spin up a second engine).
        tts.openai.user_voice = Some("  ".into());
        assert_eq!(tts.active_voices(), (Some("onyx"), None));
        // Active mode external — reads its own fields, not openai's.
        tts.mode = TtsMode::External;
        tts.external.voice = Some("bella".into());
        assert_eq!(tts.active_voices(), (Some("bella"), None));
    }

    #[test]
    fn serde_roundtrip() {
        let c = AppConfig::default();
        let json = serde_json::to_string_pretty(&c).unwrap();
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn active_model_name_by_mode() {
        // Managed — the GGUF's base name without the path or extension.
        let mut e = EngineSettings {
            mode: ServerMode::Managed,
            managed: ManagedSettings {
                model_path: Some("/models/gemma-4-it.gguf".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(e.active_model_name().as_deref(), Some("gemma-4-it"));
        // No path set → None.
        e.managed.model_path = None;
        assert_eq!(e.active_model_name(), None);
        // External — the model name as-is.
        e.mode = ServerMode::External;
        e.external.model_name = Some("qwen-3.6".into());
        assert_eq!(e.active_model_name().as_deref(), Some("qwen-3.6"));
        // Cloud — the name from the active cloud subsection.
        e.mode = ServerMode::OpenAi;
        e.openai.model_name = Some("gpt-4o".into());
        assert_eq!(e.active_model_name().as_deref(), Some("gpt-4o"));
        // An empty name is treated as unset.
        e.openai.model_name = Some(String::new());
        assert_eq!(e.active_model_name(), None);
    }

    #[test]
    fn mcp_server_config_roundtrip_and_partial_defaults() {
        // A partial server entry (as in a real settings.json) gets filled with
        // defaults: enabled=true, timeout/clip — constants.
        let c: AppConfig = serde_json::from_str(
            r#"{"mcp":{"enabled":true,"servers":[{
                "id":"fs","command":"cmd","args":["/c","npx","-y","srv"],
                "env":{"TOKEN":"MINDFORK_FS_TOKEN"}}]}}"#,
        )
        .unwrap();
        assert!(c.mcp.enabled);
        let s = &c.mcp.servers[0];
        assert_eq!(s.id, "fs");
        assert_eq!(s.command, "cmd");
        assert_eq!(s.args, vec!["/c", "npx", "-y", "srv"]);
        assert_eq!(
            s.env.get("TOKEN").map(String::as_str),
            Some("MINDFORK_FS_TOKEN")
        );
        assert!(s.enabled);
        assert_eq!(s.tool_timeout_secs, DEFAULT_MCP_TOOL_TIMEOUT_SECS);
        assert_eq!(s.max_result_chars, DEFAULT_MCP_MAX_RESULT_CHARS);
        // Round-trip: serializing → reading gives the same value.
        let json = serde_json::to_string(&c.mcp).unwrap();
        let back: McpSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c.mcp);
    }

    #[test]
    fn partial_json_fills_defaults() {
        let c: AppConfig = serde_json::from_str(r#"{"max_tool_rounds":4}"#).unwrap();
        assert_eq!(c.max_tool_rounds, 4);
        assert_eq!(c.schema_version, SCHEMA_VERSION);
        assert_eq!(c.engine.managed.port, 8000);
        assert_eq!(c.engine.managed.gpu_layers, DEFAULT_GPU_LAYERS);
        assert!(c.engine.managed.jinja);
        // New sections get filled with defaults when absent from the file.
        assert_eq!(c.embed.managed.port, DEFAULT_EMBED_PORT);
        assert_eq!(c.tools.subagent_max_tokens, DEFAULT_SUBAGENT_MAX_TOKENS);
        assert_eq!(c.tools.subagent_timeout_secs, DEFAULT_SUBAGENT_TIMEOUT_SECS);
        // File tools are off by default (like Python).
        assert!(!c.tools.fs_enabled);
        assert_eq!(c.tools.fs_root, None);
        // Python: the tool is off, mode — the Wasmer sandbox, network in the
        // sandbox is on, the sandbox timeout is the default.
        assert!(!c.tools.python_enabled);
        assert_eq!(c.tools.python_mode, PythonMode::Wasmer);
        assert!(c.tools.python_net_enabled);
        assert_eq!(
            c.tools.python_wasm_timeout_secs,
            DEFAULT_PYTHON_WASM_TIMEOUT_SECS
        );
        // The sandbox memory limit is off by default (opt-in, Windows only).
        assert_eq!(c.tools.python_wasm_memory_mb, None);
        assert_eq!(c.rag.chunk_target_chars, DEFAULT_CHUNK_TARGET_CHARS);
        assert_eq!(c.self_model.max_narrative, DEFAULT_SELF_MODEL_MAX_NARRATIVE);
        assert_eq!(
            c.self_model.narrative_in_prompt,
            DEFAULT_SELF_MODEL_NARRATIVE_IN_PROMPT
        );
        assert_eq!(c.self_model.prompt_cap, DEFAULT_SELF_MODEL_PROMPT_CAP);
        assert_eq!(
            c.self_model.max_closed_goals,
            DEFAULT_SELF_MODEL_MAX_CLOSED_GOALS
        );
        assert_eq!(
            c.self_model.summary_target_chars,
            DEFAULT_SELF_MODEL_SUMMARY_TARGET
        );
        // The self-model maintenance protocol is on by default, auto-reflection and
        // self-model auto-consolidation are not.
        assert!(c.self_model.maintenance_protocol);
        assert_eq!(c.self_model.auto_reflect_every, 0);
        assert_eq!(c.self_model.auto_consolidate_every, 0);
        // Notes: auto-consolidation off, self-notes are hidden from recall (Tier 3, Path 2).
        assert_eq!(c.notes.auto_consolidate_every, 0);
        assert!(!c.notes.recall_includes_self);
        assert_eq!(c.rag.chunk_overlap_chars, DEFAULT_CHUNK_OVERLAP_CHARS);
        assert_eq!(c.rag.chunk_max_chars, DEFAULT_CHUNK_MAX_CHARS);
        assert!(c.interface.spellcheck_enabled);
        assert_eq!(c.interface.theme, Theme::Auto);
        // Old-terminal compatibility mode is off by default.
        assert!(!c.interface.terminal_compat);
        // Markdown-table row separators are off by default.
        assert!(!c.interface.table_row_separators);
        // Mermaid diagram rendering is on by default (a hard fallback to the source
        // makes enabling it safe: the worst case = prior behavior).
        assert!(c.interface.render_mermaid);
        // Interface language (axis B) defaults to Russian (an old settings.json with
        // no field; a fresh install sets it from defaults.json in main.rs).
        assert_eq!(c.interface.language, crate::shared::i18n::Lang::Ru);
        // Copying the conversation: text only by default (all flags off).
        assert!(!c.copy.copy_thoughts);
        assert!(!c.copy.copy_tool_calls);
        assert!(!c.copy.copy_tool_results);
        // MCP host: master switch off, no servers (double opt-in, R7).
        assert!(!c.mcp.enabled);
        assert!(c.mcp.servers.is_empty());
        // Speech (TTS): the default mode is OpenAI with a sensible model and
        // voice ("not configured" = no key), speed 1.0; of behavior only
        // stopping on chat switch is enabled (R6/R8).
        assert_eq!(c.tts.mode, TtsMode::OpenAi);
        assert_eq!(
            c.tts.openai.model_name.as_deref(),
            Some(DEFAULT_TTS_OPENAI_MODEL)
        );
        assert_eq!(
            c.tts.openai.voice.as_deref(),
            Some(DEFAULT_TTS_OPENAI_VOICE)
        );
        assert_eq!(
            c.tts.gemini.model_name.as_deref(),
            Some(DEFAULT_TTS_GEMINI_MODEL)
        );
        assert_eq!(c.tts.speed, 1.0);
        assert!(!c.tts.speak_roles);
        assert!(c.tts.stop_on_chat_switch);
        assert!(!c.tts.stop_on_generation_start);
        // Impersonation is filled with defaults when absent from the file.
        assert_eq!(c.impersonation_engine.mode, ImpersonationMode::Shared);
        assert_eq!(
            c.impersonation_engine.managed.port,
            DEFAULT_IMPERSONATION_PORT
        );
        assert_eq!(c.impersonation_sampling.thinking, Some(false));
        // Memory of the last-open chat: empty by default.
        assert_eq!(c.last_active_chat, None);
    }

    #[test]
    fn impersonation_sections_roundtrip() {
        let c = AppConfig {
            impersonation_engine: ImpersonationEngineSettings {
                mode: ImpersonationMode::Managed,
                managed: ManagedSettings {
                    binary: Some("llama-server".into()),
                    model_path: Some("persona.gguf".into()),
                    port: 8002,
                    ..Default::default()
                },
                ..Default::default()
            },
            impersonation_sampling: SamplingConfig {
                temperature: Some(0.8),
                max_tokens: Some(256),
                ..Default::default()
            },
            ..Default::default()
        };
        let json = serde_json::to_string_pretty(&c).unwrap();
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn impersonation_profiles_default_empty_and_roundtrip() {
        // Additive (`#[serde(default)]` + skip-if-empty): an old settings.json without
        // the field reads fine, and an empty list doesn't clutter the JSON.
        let old: AppConfig = serde_json::from_str(r#"{"max_tool_rounds":4}"#).unwrap();
        assert!(old.impersonation_profiles.is_empty());
        assert!(
            !serde_json::to_string(&old)
                .unwrap()
                .contains("impersonation_profiles")
        );

        let c = AppConfig {
            impersonation_profiles: vec![ImpersonationProfile::new("Юзер", "Ты — Владимир.")],
            ..Default::default()
        };
        let back: AppConfig = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn impersonation_managed_default_port_is_8002() {
        let s = ImpersonationEngineSettings::default();
        assert_eq!(s.managed.port, DEFAULT_IMPERSONATION_PORT);
    }

    #[test]
    fn flash_attn_arg_and_cycle() {
        assert_eq!(FlashAttn::default(), FlashAttn::Auto);
        assert_eq!(FlashAttn::Auto.as_arg(), None);
        assert_eq!(FlashAttn::On.as_arg(), Some("on"));
        assert_eq!(FlashAttn::Off.as_arg(), Some("off"));
        // Cyclic iteration in both directions.
        assert_eq!(FlashAttn::Auto.cycle(1), FlashAttn::On);
        assert_eq!(FlashAttn::Auto.cycle(-1), FlashAttn::Off);
    }

    #[test]
    fn python_mode_default_cycle_and_serde() {
        assert_eq!(PythonMode::default(), PythonMode::Wasmer);
        // Cyclic iteration (two variants).
        assert_eq!(PythonMode::Wasmer.cycle(1), PythonMode::Local);
        assert_eq!(PythonMode::Local.cycle(1), PythonMode::Wasmer);
        assert_eq!(PythonMode::Wasmer.cycle(-1), PythonMode::Local);
        // serde — lowercase, round-trip.
        assert_eq!(
            serde_json::to_string(&PythonMode::Local).unwrap(),
            "\"local\""
        );
        let m: PythonMode = serde_json::from_str("\"wasmer\"").unwrap();
        assert_eq!(m, PythonMode::Wasmer);
        assert!(PythonMode::ALL.contains(&PythonMode::Local));
    }

    #[test]
    fn spec_type_arg_serde_and_draft_need() {
        assert_eq!(SpecType::default(), SpecType::None);
        assert_eq!(SpecType::None.as_arg(), None);
        assert_eq!(SpecType::DraftMtp.as_arg(), Some("draft-mtp"));
        assert_eq!(SpecType::NgramMapK4v.as_arg(), Some("ngram-map-k4v"));
        // The serde name matches the CLI value (kebab-case).
        assert_eq!(
            serde_json::to_string(&SpecType::DraftMtp).unwrap(),
            "\"draft-mtp\""
        );
        assert_eq!(
            serde_json::to_string(&SpecType::NgramMapK4v).unwrap(),
            "\"ngram-map-k4v\""
        );
        // A draft model is only needed by draft-* types.
        assert!(SpecType::DraftMtp.needs_draft_model());
        assert!(!SpecType::NgramSimple.needs_draft_model());
        assert!(!SpecType::None.needs_draft_model());
    }

    #[test]
    fn managed_spec_fields_roundtrip() {
        let c = AppConfig {
            engine: EngineSettings {
                mode: ServerMode::Managed,
                managed: ManagedSettings {
                    binary: Some("llama-server".into()),
                    model_path: Some("mtp-gemma-4-12B-it.gguf".into()),
                    flash_attn: FlashAttn::On,
                    spec_type: SpecType::DraftMtp,
                    draft_model: Some("mtp-gemma-4-12B-it.gguf".into()),
                    draft_gpu_layers: Some(99),
                    draft_n_max: Some(5),
                    draft_n_min: Some(1),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let json = serde_json::to_string_pretty(&c).unwrap();
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn per_provider_cloud_settings_are_independent() {
        // Each provider stores its own fields — switching modes doesn't lose the other's.
        let mut e = EngineSettings {
            mode: ServerMode::OpenAi,
            openai: CloudSettings {
                model_name: Some("gpt-4o".into()),
                api_key_env: Some("OPENAI_API_KEY".into()),
                ..Default::default()
            },
            gemini: CloudSettings {
                model_name: Some("gemini-2.5-pro".into()),
                api_key_env: Some("GEMINI_API_KEY".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        // Active provider — OpenAI.
        assert_eq!(e.cloud().unwrap().model_name.as_deref(), Some("gpt-4o"));
        // Switching to Gemini exposes its own fields, OpenAI stays intact.
        e.mode = ServerMode::Gemini;
        assert_eq!(
            e.cloud().unwrap().model_name.as_deref(),
            Some("gemini-2.5-pro")
        );
        assert_eq!(e.openai.model_name.as_deref(), Some("gpt-4o"));
        // Local modes — no cloud.
        e.mode = ServerMode::Managed;
        assert!(e.cloud().is_none());
    }

    #[test]
    fn impersonation_mode_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&ImpersonationMode::External).unwrap(),
            "\"external\""
        );
        assert_eq!(
            serde_json::to_string(&ImpersonationMode::Shared).unwrap(),
            "\"shared\""
        );
    }

    #[test]
    fn extended_sections_roundtrip() {
        let c = AppConfig {
            engine: EngineSettings {
                mode: ServerMode::Managed,
                managed: ManagedSettings {
                    binary: Some("llama-server".into()),
                    model_path: Some("gemma.gguf".into()),
                    gpu_layers: 50,
                    context_size: 4096,
                    jinja: true,
                    reasoning_format: Some("auto".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
            embed: EmbedSettings {
                mode: ServerMode::External,
                external: ExternalSettings {
                    url: Some("http://127.0.0.1:8001/v1".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
            tools: ToolSettings {
                python_enabled: true,
                subagent_max_tokens: 512,
                subagent_timeout_secs: 30,
                ..Default::default()
            },
            interface: InterfaceSettings {
                theme: Theme::Dark,
                spellcheck_enabled: false,
                selected_dictionaries: vec!["en_US".into(), "ru_RU".into()],
                confirm_destructive_keys: true,
                terminal_compat: true,
                table_row_separators: true,
                render_mermaid: false,
                language: crate::shared::i18n::Lang::Ru,
            },
            ..Default::default()
        };
        let json = serde_json::to_string_pretty(&c).unwrap();
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn theme_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Theme::Dark).unwrap(), "\"dark\"");
    }

    #[test]
    fn cloud_modes_serialize_and_map_to_provider() {
        assert_eq!(
            serde_json::to_string(&ServerMode::OpenAi).unwrap(),
            "\"openai\""
        );
        assert_eq!(
            serde_json::to_string(&ServerMode::Gemini).unwrap(),
            "\"gemini\""
        );
        assert_eq!(
            ServerMode::OpenAi.cloud_provider(),
            Some(CloudProvider::OpenAi)
        );
        assert_eq!(
            ServerMode::Gemini.cloud_provider(),
            Some(CloudProvider::Gemini)
        );
        assert_eq!(ServerMode::Managed.cloud_provider(), None);
        assert_eq!(ServerMode::External.cloud_provider(), None);
        // Claude — cloud, but not the OpenAI protocol.
        assert_eq!(
            serde_json::to_string(&ServerMode::Claude).unwrap(),
            "\"claude\""
        );
        assert_eq!(
            ServerMode::Claude.cloud_provider(),
            Some(CloudProvider::Claude)
        );
        assert!(CloudProvider::Claude.base_url().contains("anthropic"));
        // Impersonation: the same cloud providers, other modes — None.
        assert_eq!(
            ImpersonationMode::OpenAi.cloud_provider(),
            Some(CloudProvider::OpenAi)
        );
        assert_eq!(ImpersonationMode::Shared.cloud_provider(), None);
        // Providers' base URLs.
        assert!(
            CloudProvider::OpenAi
                .base_url()
                .starts_with("https://api.openai.com")
        );
        assert!(
            CloudProvider::Gemini
                .base_url()
                .contains("generativelanguage")
        );
    }

    #[test]
    fn cloud_engine_settings_roundtrip() {
        let c = AppConfig {
            engine: EngineSettings {
                mode: ServerMode::OpenAi,
                openai: CloudSettings {
                    model_name: Some("gpt-4o".into()),
                    api_key_env: Some("OPENAI_API_KEY".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let json = serde_json::to_string_pretty(&c).unwrap();
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
        // Sub-sections get filled with defaults when absent from the file.
        let old: AppConfig = serde_json::from_str(r#"{"engine":{"mode":"managed"}}"#).unwrap();
        assert_eq!(old.engine.openai.model_name, None);
        assert_eq!(old.engine.openai.api_key_env, None);
        assert_eq!(old.engine.managed.binary, None);
    }
}

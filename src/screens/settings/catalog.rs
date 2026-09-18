//! Settings screen — the field catalog: the constructor, field builders for
//! sections and subsections (model/sampling/tools/memory/interface/profiles), and
//! availability gates. Part of the [`super`] module; split out of settings.rs.

use super::helpers::*;
use super::*;
use crate::shared::config::WebProvider;
use crate::shared::secrets::SearchSlot;

impl SettingsScreen {
    /// Creates the screen from a settings snapshot (config + visible profiles + profiles
    /// with a locked scaffold language).
    pub fn new(
        config: AppConfig,
        profiles: Vec<Profile>,
        language_locked: Vec<uuid::Uuid>,
    ) -> Self {
        Self {
            config,
            profiles,
            section_idx: 0,
            field_idx: 0,
            focus: Focus::Menu,
            profile_idx: 0,
            imp_profile_idx: 0,
            mcp_server_idx: 0,
            pending_profile_select: false,
            model_sub: ModelTab::Assistant,
            sampling_sub: Subsection::Assistant,
            profile_sub: Subsection::Assistant,
            editor: None,
            search: None,
            choice: None,
            picker: None,
            catalogues: Vec::new(),
            asked: Vec::new(),
            statuses: ServerStatuses {
                chat: ServerStatus::NotConfigured,
                embed: ServerStatus::NotConfigured,
                impersonation: ServerStatus::NotConfigured,
            },
            engine_slots: None,
            engine_sampling_fields: None,
            language_locked,
            mcp: Default::default(),
            secrets_present: Vec::new(),
            mcp_import_result: None,
            undo: Vec::new(),
            redo: Vec::new(),
            menu_scroll: ListScroll::default(),
            fields_scroll: ListScroll::default(),
        }
    }

    /// Updates the server-status snapshot (chips in the "Model/server" section). Called
    /// by `app` when creating the screen and on the `ServerStatus` event.
    pub fn set_server_statuses(&mut self, statuses: ServerStatuses) {
        self.statuses = statuses;
    }

    /// Updates what the engine said about its slot count (the hint next to the
    /// `sessions` field, spec §11.6). Called by `app` when creating the screen
    /// and on the `EngineSlots` event; `None` — the engine cannot say.
    /// What the endpoint's catalogue published about the configured model's
    /// sampling fields (`AppEvent::EngineSamplingFields`). `None` — it said
    /// nothing, and the sampling group shows what it always did
    /// (docs/history/gateway-capabilities.md §4, G3).
    pub fn set_engine_sampling_fields(&mut self, fields: Option<std::sync::Arc<[String]>>) {
        self.engine_sampling_fields = fields;
    }

    pub fn set_engine_slots(&mut self, slots: Option<u32>) {
        self.engine_slots = slots;
    }

    /// Updates the working copy from a settings re-emit (after create/delete of a
    /// profile or an echoed edit). Navigation and the active editor are preserved.
    pub fn refresh(
        &mut self,
        config: AppConfig,
        profiles: Vec<Profile>,
        language_locked: Vec<uuid::Uuid>,
    ) {
        // A profile we asked to create arrives with this snapshot — select it, so it's
        // editable right away (the orchestrator owns the list; the screen can only
        // recognise the new entry by comparing ids). One-shot: the flag is cleared
        // whatever the snapshot brings, so a later unrelated re-emit can't hijack the
        // selection.
        let new_idx = std::mem::take(&mut self.pending_profile_select)
            .then(|| {
                profiles
                    .iter()
                    .position(|p| !self.profiles.iter().any(|old| old.id == p.id))
            })
            .flatten();
        self.config = config;
        self.profiles = profiles;
        self.language_locked = language_locked;
        if let Some(idx) = new_idx {
            self.profile_idx = idx;
        }
        if !self.profiles.is_empty() && self.profile_idx >= self.profiles.len() {
            self.profile_idx = self.profiles.len() - 1;
        }
        self.clamp_imp_profile_idx();
        self.clamp_mcp_server_idx();
    }

    /// Keeps the impersonation-profile selection inside the list (it can shrink from a
    /// deletion here or a config re-emit).
    pub(super) fn clamp_imp_profile_idx(&mut self) {
        let len = self.config.impersonation_profiles.len();
        if len > 0 && self.imp_profile_idx >= len {
            self.imp_profile_idx = len - 1;
        }
    }

    pub(super) fn section(&self) -> Section {
        SECTIONS[self.section_idx]
    }

    /// The catalog of all known tools (for profile toggles) — including optional ones
    /// (off by default) and **dynamic** MCP-server tools (a snapshot from the
    /// `Settings` event, appended at the end — the static part's `PTool` indices stay
    /// stable). Metadata (group/label/gate) is taken from the tools themselves (a single
    /// source — the `Tool` trait). See spec §9.3.
    pub(super) fn tool_catalog(&self) -> Vec<ToolInfo> {
        let mut catalog = crate::features::tools::tool_catalog();
        catalog.extend(self.mcp.tools.iter().cloned());
        catalog
    }

    /// Updates the MCP-host snapshot (from the `Settings` event: the dynamic tool
    /// catalog + server statuses; empty until servers come up/while MCP is off).
    pub fn set_mcp(&mut self, mcp: crate::features::tools::mcp::McpSnapshot) {
        self.mcp = mcp;
    }

    /// Updates which secrets are stored on this machine (from the `Settings`
    /// snapshot) — every secret field shows its status from this. The secrets
    /// themselves never reach the UI. See `shared::secrets`.
    pub fn set_secrets_present(&mut self, present: Vec<SecretKey>) {
        self.secrets_present = present;
    }

    /// Shows the outcome of an MCP import on the import row
    /// (`AppEvent::McpImportResult`).
    pub fn set_mcp_import_result(&mut self, text: String) {
        self.mcp_import_result = Some(text);
    }

    /// Whether this secret is stored on this machine (a secret field's status).
    pub(super) fn secret_present(&self, key: Option<&SecretKey>) -> bool {
        key.is_some_and(|k| self.secrets_present.contains(k))
    }

    /// The keyed search provider's API-key pair (the key itself and the variable
    /// it may come from instead) — or nothing under `FreeOnly`, which is the
    /// setting that says the key must not be spent. An unused key row reading as
    /// a live one is how someone ends up believing a provider is configured when
    /// nothing will ever call it.
    fn keyed_search_rows(&self, loc: &'static Locale) -> Vec<FieldRow> {
        let t = &self.config.tools;
        if t.web_provider == WebProvider::FreeOnly {
            return Vec::new();
        }
        let name = SearchSlot::Tavily.display_name();
        vec![
            secret_row(
                FieldId::TWebTavilyKey,
                self.secret_field_present(FieldId::TWebTavilyKey),
                &api_key_label(Some(name), loc),
                DESC_SEARCH_API_KEY,
                loc,
            ),
            text_row(
                FieldId::TWebTavilyKeyEnv,
                &api_key_env_opt_label(name, loc),
                &t.web_tavily_key_env,
            )
            .describe(loc.t("ui.settings.desc.search_api_key_env")),
        ]
    }

    /// Whether the secret a given field addresses is stored on this machine (that
    /// field's status). Goes through [`Self::secret_field_key`], so the row a user
    /// reads and the value an edit replaces are the same secret by construction —
    /// in a cloud mode the provider's key, in `external` mode that slot's own.
    pub(super) fn secret_field_present(&self, id: FieldId) -> bool {
        self.secret_present(self.secret_field_key(id).as_ref())
    }

    /// Which stored secret an input field addresses, or `None` — not a secret
    /// field. A **cloud** key is shared across chat/impersonation/embeddings of one
    /// provider, so it is the provider that matters, not the slot; an **external**
    /// server's key is the slot's own, since its URL is whatever the user typed; an
    /// MCP value is addressed by (server, variable). The first two distinctions are
    /// the settings struct's own answer (`secret_key()`) rather than this screen's,
    /// so a row cannot address a different secret than the orchestrator resolves.
    /// See docs/research/api-key-storage.md, docs/history/external-api-key.md §5.2,
    /// docs/history/mcp-server-editor.md §9.
    pub(super) fn secret_field_key(&self, id: FieldId) -> Option<SecretKey> {
        match id {
            FieldId::XApiKey => self.config.engine.secret_key(),
            FieldId::IxApiKey => self.config.impersonation_engine.secret_key(),
            FieldId::EApiKey => self.config.embed.secret_key(),
            FieldId::TtsApiKey => self.config.tts.secret_key(),
            // The video slot has no mode of its own — only Gemini takes video
            // (spec §9.9), so this row always addresses the Gemini key.
            FieldId::VideoApiKey => Some(SecretKey::Provider(CloudProvider::Gemini)),
            // A search slot, like an external one, is its own address: these are
            // not inference providers and must never resolve through the
            // engine's key lookup (see `SecretKey::Search`).
            FieldId::TWebTavilyKey => Some(SecretKey::Search(SearchSlot::Tavily)),
            FieldId::BackupPassword => Some(SecretKey::BackupPassword),
            FieldId::McpEnvSecret(idx) => {
                let srv = self.config.mcp.servers.get(self.mcp_server_idx)?;
                let var = srv.env.keys().nth(idx)?;
                Some(SecretKey::McpEnv {
                    server: srv.id.clone(),
                    var: var.clone(),
                })
            }
            _ => None,
        }
    }

    // ---------- building the current section's fields ----------

    pub(super) fn fields(&self) -> Vec<FieldRow> {
        match self.section() {
            Section::Model => self.model_fields(),
            Section::Sampling => self.sampling_fields(),
            Section::Tools => self.tool_fields(),
            Section::Plugins => self.plugin_fields(),
            Section::Memory => self.memory_fields(),
            Section::Data => self.data_fields(),
            Section::Profiles => self.profile_fields(),
            Section::Interface => self.interface_fields(),
        }
    }

    pub(super) fn model_fields(&self) -> Vec<FieldRow> {
        self.model_fields_for(self.model_sub)
    }

    /// The "Model" section's fields for a given subsection (for enumeration during
    /// search — [`SettingsScreen::model_fields`] builds them for the active subsection).
    pub(super) fn model_fields_for(&self, model_sub: ModelTab) -> Vec<FieldRow> {
        let loc = self.loc();
        // The subsection selector (tab strip) — always field 0; not drawn as a list row.
        let sub = row(
            FieldId::ModelSub,
            loc.t("ui.settings.field.subsection"),
            FieldKind::Choice(model_sub.label(loc)),
        )
        .describe(loc.t(DESC_SUBSECTION));
        match model_sub {
            ModelTab::Assistant => {
                let x = &self.config.engine;
                let mut rows = vec![
                    sub,
                    row(
                        FieldId::XMode,
                        loc.t("ui.settings.field.mode"),
                        FieldKind::Choice(mode_label(x.mode)),
                    )
                    .describe(loc.t(DESC_MODE)),
                ];
                // Field visibility depends on mode (ADR 0004): for cloud we show only
                // model/key/opt. base URL, for managed — llama-server parameters.
                match x.mode {
                    ServerMode::Managed => {
                        rows.extend(managed_rows(&x.managed, ASSISTANT_MANAGED_IDS, loc))
                    }
                    ServerMode::External => rows.extend(grouped(
                        loc.t("ui.settings.group.server"),
                        vec![
                            text_row(FieldId::XUrl, "URL (external)", &x.external.url),
                            text_row(
                                FieldId::XModelName,
                                loc.t("ui.settings.field.model_opt"),
                                &x.external.model_name,
                            )
                            .describe(loc.t(DESC_MODEL_NAME_EXTERNAL)),
                            ext_api_key_row(
                                FieldId::XApiKey,
                                self.secret_field_present(FieldId::XApiKey),
                                loc,
                            ),
                            text_row(
                                FieldId::XApiKeyEnv,
                                loc.t("ui.settings.field.api_key_env_opt"),
                                &x.external.api_key_env,
                            )
                            .describe(loc.t(DESC_EXT_API_KEY_ENV)),
                        ],
                    )),
                    ServerMode::OpenAi
                    | ServerMode::Gemini
                    | ServerMode::Claude
                    | ServerMode::Grok => rows.extend(grouped(
                        loc.t("ui.settings.group.provider"),
                        cloud_rows(
                            x.cloud().zip(x.mode.cloud_provider()),
                            FieldId::XModelName,
                            FieldId::XApiKey,
                            FieldId::XApiKeyEnv,
                            FieldId::XUrl,
                            self.secret_field_present(FieldId::XApiKey),
                            loc,
                        ),
                    )),
                }
                // Parallel sessions — the assistant engine only (spec §11.6): the
                // value lives in the active mode's own section, and the slot
                // count a `llama-server` reported is a hint next to the field,
                // never written into it. A cloud has no slots to report.
                let (sessions, concurrent, slots_hint) = match x.mode {
                    ServerMode::Managed => (
                        x.managed.sessions,
                        x.managed.concurrent_calls,
                        self.engine_slots,
                    ),
                    ServerMode::External => (
                        x.external.sessions,
                        x.external.concurrent_calls,
                        self.engine_slots,
                    ),
                    ServerMode::OpenAi
                    | ServerMode::Gemini
                    | ServerMode::Claude
                    | ServerMode::Grok => (
                        x.cloud().map_or(1, |c| c.sessions),
                        x.cloud().map_or(1, |c| c.concurrent_calls),
                        None,
                    ),
                };
                let mut desc = loc.t("ui.settings.desc.sessions").to_string();
                if let Some(n) = slots_hint {
                    desc.push(' ');
                    desc.push_str(
                        &loc.tf("ui.settings.desc.sessions_slots", &[("n", &n.to_string())]),
                    );
                }
                // The concurrent group's width (spec §6.3). Its hint names the
                // tools the number covers **from the catalog's own marks**, so
                // it cannot advertise a tool the rule does not run together
                // (docs/lessons.md §4; docs/research/concurrent-tools.md §4.4).
                let marked: Vec<String> = crate::features::tools::tool_catalog()
                    .into_iter()
                    .filter(|t| t.concurrent)
                    .map(|t| t.id)
                    .collect();
                let concurrent_desc = format!(
                    "{} {}",
                    loc.t("ui.settings.desc.concurrent_calls"),
                    loc.tf(
                        "ui.settings.desc.concurrent_calls_tools",
                        &[("tools", &marked.join(", "))],
                    )
                );
                rows.extend(grouped(
                    loc.t("ui.settings.group.sessions"),
                    vec![
                        row(
                            FieldId::XSessions,
                            loc.t("ui.settings.field.sessions"),
                            FieldKind::Text(sessions.to_string()),
                        )
                        .describe(desc),
                        row(
                            FieldId::XConcurrent,
                            loc.t("ui.settings.field.concurrent_calls"),
                            FieldKind::Text(concurrent.to_string()),
                        )
                        .describe(concurrent_desc),
                    ],
                ));
                rows
            }
            ModelTab::Impersonation => {
                let x = &self.config.impersonation_engine;
                let mut rows = vec![
                    sub,
                    row(
                        FieldId::IxMode,
                        loc.t("ui.settings.field.mode"),
                        FieldKind::Choice(imp_mode_label(x.mode)),
                    )
                    .describe(loc.t(DESC_IMP_MODE)),
                ];
                match x.mode {
                    // Shared reuses the assistant's engine — no fields of its own.
                    ImpersonationMode::Shared => {}
                    ImpersonationMode::Managed => {
                        rows.extend(managed_rows(&x.managed, IMP_MANAGED_IDS, loc))
                    }
                    ImpersonationMode::External => rows.extend(grouped(
                        loc.t("ui.settings.group.server"),
                        vec![
                            text_row(FieldId::IxUrl, "URL (external)", &x.external.url),
                            text_row(
                                FieldId::IxModelName,
                                loc.t("ui.settings.field.model_opt"),
                                &x.external.model_name,
                            )
                            .describe(loc.t(DESC_MODEL_NAME_EXTERNAL)),
                            ext_api_key_row(
                                FieldId::IxApiKey,
                                self.secret_field_present(FieldId::IxApiKey),
                                loc,
                            ),
                            text_row(
                                FieldId::IxApiKeyEnv,
                                loc.t("ui.settings.field.api_key_env_opt"),
                                &x.external.api_key_env,
                            )
                            .describe(loc.t(DESC_EXT_API_KEY_ENV)),
                        ],
                    )),
                    ImpersonationMode::OpenAi
                    | ImpersonationMode::Gemini
                    | ImpersonationMode::Claude
                    | ImpersonationMode::Grok => rows.extend(grouped(
                        loc.t("ui.settings.group.provider"),
                        cloud_rows(
                            x.cloud().zip(x.mode.cloud_provider()),
                            FieldId::IxModelName,
                            FieldId::IxApiKey,
                            FieldId::IxApiKeyEnv,
                            FieldId::IxUrl,
                            self.secret_field_present(FieldId::IxApiKey),
                            loc,
                        ),
                    )),
                }
                rows
            }
            ModelTab::Embeddings => {
                // Embeddings — a dedicated server (memory/RAG). Fields by mode
                // (managed → llama-server; external/cloud → URL/model/key).
                let e = &self.config.embed;
                let mut rows = vec![
                    sub,
                    row(
                        FieldId::EMode,
                        loc.t("ui.settings.field.mode"),
                        FieldKind::Choice(mode_label(e.mode)),
                    )
                    .describe(loc.t(DESC_MODE)),
                ];
                match e.mode {
                    ServerMode::Managed => rows.extend(grouped(
                        loc.t("ui.settings.group.server"),
                        vec![
                            text_row(
                                FieldId::EBinary,
                                loc.t("ui.settings.field.binary"),
                                &e.managed.binary,
                            ),
                            text_row(
                                FieldId::EModel,
                                loc.t("ui.settings.field.gguf"),
                                &e.managed.model_path,
                            ),
                            num_field(
                                FieldId::EPort,
                                loc.t("ui.settings.field.port"),
                                e.managed.port,
                            ),
                        ],
                    )),
                    ServerMode::External => rows.extend(grouped(
                        loc.t("ui.settings.group.server"),
                        vec![
                            text_row(FieldId::EUrl, "URL (external)", &e.external.url),
                            text_row(
                                FieldId::EModelName,
                                loc.t("ui.settings.field.model_opt"),
                                &e.external.model_name,
                            )
                            .describe(loc.t(DESC_MODEL_NAME_EXTERNAL)),
                            ext_api_key_row(
                                FieldId::EApiKey,
                                self.secret_field_present(FieldId::EApiKey),
                                loc,
                            ),
                            text_row(
                                FieldId::EApiKeyEnv,
                                loc.t("ui.settings.field.api_key_env_opt"),
                                &e.external.api_key_env,
                            )
                            .describe(loc.t(DESC_EXT_API_KEY_ENV)),
                        ],
                    )),
                    // Claude and Grok show fields, but neither Anthropic nor xAI does
                    // embeddings — the supervisor will return "unavailable" (RAG turns
                    // off). ADR 0004.
                    ServerMode::OpenAi
                    | ServerMode::Gemini
                    | ServerMode::Claude
                    | ServerMode::Grok => {
                        let none = CloudSettings::default();
                        let c = e.cloud().unwrap_or(&none);
                        let name = e.mode.cloud_provider().map(CloudProvider::display_name);
                        rows.extend(grouped(
                            loc.t("ui.settings.group.provider"),
                            vec![
                                text_row(
                                    FieldId::EModelName,
                                    loc.t("ui.settings.field.model"),
                                    &c.model_name,
                                )
                                .describe(loc.t(DESC_MODEL_NAME)),
                                api_key_row(
                                    FieldId::EApiKey,
                                    self.secret_field_present(FieldId::EApiKey),
                                    name,
                                    loc,
                                ),
                                text_row(
                                    FieldId::EApiKeyEnv,
                                    &api_key_env_label(name, loc),
                                    &c.api_key_env,
                                )
                                .describe(loc.t(DESC_API_KEY_ENV)),
                                text_row(
                                    FieldId::EUrl,
                                    loc.t("ui.settings.field.base_url"),
                                    &c.url,
                                ),
                            ],
                        ))
                    }
                }
                // Independent of the mode: the input convention is a property of
                // the *model*, not of where it runs (research
                // docs/research/embedding-input-prefixes.md).
                rows.extend(grouped(
                    loc.t("ui.settings.group.embed_input"),
                    vec![
                        row(
                            FieldId::EConvention,
                            loc.t("ui.settings.field.embed_convention"),
                            FieldKind::Choice(e.convention.id().to_string()),
                        )
                        .describe(loc.t("ui.settings.desc.embed_convention")),
                    ],
                ));
                rows
            }
            ModelTab::Tts => {
                // Speech — an independent slot (Anthropic has no TTS): its own
                // provider, its own model/voice. The cloud key is shared with chat
                // (ADR 0008), no need to enter it again. See spec §11.9.
                let t = &self.config.tts;
                let mut rows = vec![
                    sub,
                    row(
                        FieldId::TtsMode,
                        loc.t("ui.settings.field.mode"),
                        FieldKind::Choice(t.mode.label().to_string()),
                    )
                    .describe(loc.t("ui.settings.desc.tts_mode")),
                ];
                let mut engine = match t.mode {
                    TtsMode::External => vec![
                        text_row(FieldId::TtsUrl, "URL (external)", &t.external.url),
                        text_row(
                            FieldId::TtsModelName,
                            loc.t("ui.settings.field.model_opt"),
                            &t.external.model_name,
                        )
                        .describe(loc.t("ui.settings.desc.tts_model")),
                        text_row(
                            FieldId::TtsVoice,
                            loc.t("ui.settings.field.voice"),
                            &t.external.voice,
                        )
                        .describe(loc.t("ui.settings.desc.tts_voice")),
                        text_row(
                            FieldId::TtsUserVoice,
                            loc.t("ui.settings.field.tts_user_voice"),
                            &t.external.user_voice,
                        )
                        .describe(loc.t("ui.settings.desc.tts_user_voice")),
                        ext_api_key_row(
                            FieldId::TtsApiKey,
                            self.secret_field_present(FieldId::TtsApiKey),
                            loc,
                        ),
                        text_row(
                            FieldId::TtsApiKeyEnv,
                            loc.t("ui.settings.field.api_key_env_opt"),
                            &t.external.api_key_env,
                        )
                        .describe(loc.t(DESC_EXT_API_KEY_ENV)),
                    ],
                    _ => {
                        let none = TtsCloudSettings::default();
                        let c = t.cloud().unwrap_or(&none);
                        let name = t.mode.cloud_provider().map(CloudProvider::display_name);
                        vec![
                            text_row(
                                FieldId::TtsModelName,
                                loc.t("ui.settings.field.model"),
                                &c.model_name,
                            )
                            .describe(loc.t("ui.settings.desc.tts_model")),
                            text_row(
                                FieldId::TtsVoice,
                                loc.t("ui.settings.field.voice"),
                                &c.voice,
                            )
                            .describe(loc.t("ui.settings.desc.tts_voice")),
                            text_row(
                                FieldId::TtsUserVoice,
                                loc.t("ui.settings.field.tts_user_voice"),
                                &c.user_voice,
                            )
                            .describe(loc.t("ui.settings.desc.tts_user_voice")),
                            text_row(
                                FieldId::TtsInstructions,
                                loc.t("ui.settings.field.tts_instructions"),
                                &c.instructions,
                            )
                            .describe(loc.t("ui.settings.desc.tts_instructions")),
                            api_key_row(
                                FieldId::TtsApiKey,
                                self.secret_field_present(FieldId::TtsApiKey),
                                name,
                                loc,
                            ),
                            text_row(
                                FieldId::TtsApiKeyEnv,
                                &api_key_env_label(name, loc),
                                &c.api_key_env,
                            )
                            .describe(loc.t(DESC_API_KEY_ENV)),
                            text_row(FieldId::TtsUrl, loc.t("ui.settings.field.base_url"), &c.url),
                        ]
                    }
                };
                engine.push(
                    row(
                        FieldId::TtsSpeed,
                        loc.t("ui.settings.field.tts_speed"),
                        FieldKind::Text(t.speed.to_string()),
                    )
                    .describe(loc.t("ui.settings.desc.tts_speed")),
                );
                rows.extend(grouped(loc.t("ui.settings.group.engine"), engine));
                rows.extend(grouped(
                    loc.t("ui.settings.group.behavior"),
                    vec![
                        row(
                            FieldId::TtsSpeakRoles,
                            loc.t("ui.settings.field.tts_speak_roles"),
                            FieldKind::Toggle(t.speak_roles),
                        )
                        .describe(loc.t("ui.settings.desc.tts_speak_roles")),
                        row(
                            FieldId::TtsStopOnSwitch,
                            loc.t("ui.settings.field.tts_stop_switch"),
                            FieldKind::Toggle(t.stop_on_chat_switch),
                        )
                        .describe(loc.t("ui.settings.desc.tts_stop_switch")),
                        row(
                            FieldId::TtsStopOnGeneration,
                            loc.t("ui.settings.field.tts_stop_generation"),
                            FieldKind::Toggle(t.stop_on_generation_start),
                        )
                        .describe(loc.t("ui.settings.desc.tts_stop_generation")),
                    ],
                ));
                rows
            }
        }
    }

    pub(super) fn sampling_fields(&self) -> Vec<FieldRow> {
        self.sampling_fields_for(self.sampling_sub)
    }

    /// The "Sampling" section's fields for a given subsection (for enumeration during search).
    pub(super) fn sampling_fields_for(&self, sampling_sub: Subsection) -> Vec<FieldRow> {
        let loc = self.loc();
        let mut rows = vec![
            row(
                FieldId::SamplingSub,
                loc.t("ui.settings.field.subsection"),
                FieldKind::Choice(sampling_sub.label(loc)),
            )
            .describe(loc.t(DESC_SUBSECTION)),
        ];
        let (s, imp) = match sampling_sub {
            Subsection::Assistant => (&self.config.default_sampling, false),
            Subsection::Impersonation => (&self.config.impersonation_sampling, true),
        };
        let mk = |p: SamplingParam| if imp { FieldId::IS(p) } else { FieldId::S(p) };
        // In cloud mode we show only parameters the provider actually accepts
        // (llama.cpp extensions and reasoning fields are hidden — ADR 0004).
        // Hidden parameters' values are preserved and will work on a local model.
        let provider = self.sampling_cloud_provider(imp);
        // …and in `external` mode, only the parameters the endpoint's own
        // catalogue lists for the configured model, when it lists any: a gateway
        // drops the llama.cpp extensions on the way, and `repeat_penalty` worst
        // of all — it is spelled `repetition_penalty` there, so the knob looked
        // set and did nothing (docs/history/gateway-capabilities.md §1).
        let available = crate::entities::sampling::available_sampling_fields(
            provider,
            self.engine_sampling_fields.as_deref(),
        );
        rows.extend(
            SAMPLING_PARAMS
                .iter()
                .filter(|&&p| available.contains(&p.field_name()))
                .map(|&p| {
                    let mut r = sampling_row(mk(p), p, s, loc);
                    r.group = p.group(loc);
                    r
                }),
        );
        rows
    }

    pub(super) fn tool_fields(&self) -> Vec<FieldRow> {
        let loc = self.loc();
        let t = &self.config.tools;
        let mut rows = grouped(
            loc.t("ui.settings.group.agentic"),
            vec![
                row(
                    FieldId::MaxToolRounds,
                    loc.t("ui.settings.field.max_tool_rounds"),
                    FieldKind::Text(self.config.max_tool_rounds.to_string()),
                )
                .describe(loc.t("ui.settings.desc.max_tool_rounds")),
                row(
                    FieldId::TConfirmDangerous,
                    loc.t("ui.settings.field.confirm_dangerous"),
                    FieldKind::Toggle(t.confirm_dangerous),
                )
                .describe(loc.t("ui.settings.desc.confirm_dangerous")),
                row(
                    FieldId::TSubMaxTokens,
                    loc.t("ui.settings.field.sub_max_tokens"),
                    FieldKind::Text(t.subagent_max_tokens.to_string()),
                )
                .describe(loc.t("ui.settings.desc.sub_max_tokens")),
                row(
                    FieldId::TSubTimeout,
                    loc.t("ui.settings.field.sub_timeout"),
                    FieldKind::Text(t.subagent_run_timeout_secs.to_string()),
                )
                .describe(loc.t("ui.settings.desc.sub_timeout")),
                row(
                    FieldId::TSubParallel,
                    loc.t("ui.settings.field.sub_parallel"),
                    FieldKind::Text(t.subagent_parallel.to_string()),
                )
                .describe(loc.t("ui.settings.desc.sub_parallel")),
                row(
                    FieldId::TSubBackground,
                    loc.t("ui.settings.field.sub_background"),
                    FieldKind::Toggle(t.subagent_background),
                )
                .describe(loc.t("ui.settings.desc.sub_background")),
                row(
                    FieldId::TSubBackgroundWake,
                    loc.t("ui.settings.field.sub_background_wake"),
                    FieldKind::Toggle(t.subagent_background_wake),
                )
                .describe(loc.t("ui.settings.desc.sub_background_wake")),
                row(
                    FieldId::TSubBackgroundMax,
                    loc.t("ui.settings.field.sub_background_max"),
                    FieldKind::Text(t.subagent_background_max.to_string()),
                )
                .describe(loc.t("ui.settings.desc.sub_background_max")),
                row(
                    FieldId::TDialogueTimeout,
                    loc.t("ui.settings.field.dialogue_timeout"),
                    FieldKind::Text(t.dialogue_run_timeout_secs.to_string()),
                )
                .describe(loc.t("ui.settings.desc.dialogue_timeout")),
                // Empty — wait until every task has landed
                // (docs/research/quit-settle-roll-and-cap.md §3.2).
                row(
                    FieldId::TQuitSettle,
                    loc.t("ui.settings.field.quit_settle"),
                    FieldKind::Text(
                        t.quit_settle_secs
                            .map_or_else(String::new, |secs| secs.to_string()),
                    ),
                )
                .describe(loc.t("ui.settings.desc.quit_settle")),
            ],
        );
        rows.extend(grouped(
            loc.t("ui.settings.group.websearch"),
            vec![
                row(
                    FieldId::TWeb,
                    loc.t("ui.settings.field.web_search"),
                    FieldKind::Toggle(t.web_enabled),
                )
                .describe(loc.t("ui.settings.desc.web_search")),
                row(
                    FieldId::TWebFetch,
                    loc.t("ui.settings.field.web_fetch"),
                    FieldKind::Toggle(t.web_fetch_content),
                )
                .describe(loc.t("ui.settings.desc.web_fetch")),
                row(
                    FieldId::TWebAllowPrivate,
                    loc.t("ui.settings.field.web_allow_private"),
                    FieldKind::Toggle(t.web_allow_private),
                )
                .describe(loc.t("ui.settings.desc.web_allow_private")),
                row(
                    FieldId::TWebProvider,
                    loc.t("ui.settings.field.web_provider"),
                    FieldKind::Choice(web_provider_label(t.web_provider, loc)),
                )
                .describe(loc.t("ui.settings.desc.web_provider")),
            ]
            .into_iter()
            // A key row only where the key can be spent. `Auto` shows both,
            // a named provider only its own, and `FreeOnly` neither — the
            // same "a sub-section per mode" shape the engine sections have,
            // and it keeps an unused key from reading as a live one.
            .chain(self.keyed_search_rows(loc))
            .collect::<Vec<_>>(),
        ));
        rows.extend(grouped("Python", {
            let mut py = vec![
                row(
                    FieldId::TPythonMode,
                    loc.t("ui.settings.field.python_mode"),
                    FieldKind::Choice(python_mode_label(t.python_mode, loc)),
                )
                .describe(loc.t("ui.settings.desc.python_mode")),
                row(
                    FieldId::TPython,
                    loc.t("ui.settings.field.python"),
                    FieldKind::Toggle(t.python_enabled),
                )
                .describe(loc.t("ui.settings.desc.python")),
                // Above the mode split, with the switches that mean the same in both:
                // the local interpreter answers the same contract as the sandbox and
                // collects the same `out/`, so what it draws is withheld by the same
                // flag (docs/history/sandbox-file-exchange.md §14 V1).
                row(
                    FieldId::TPythonImages,
                    loc.t("ui.settings.field.python_images"),
                    FieldKind::Toggle(t.python_images),
                )
                .describe(loc.t("ui.settings.desc.python_images")),
            ];
            match t.python_mode {
                PythonMode::Local => {
                    py.push(
                        text_row(
                            FieldId::TPythonPath,
                            loc.t("ui.settings.field.python_path"),
                            &t.python_path,
                        )
                        .describe(loc.t("ui.settings.desc.python_path")),
                    );
                    // Its own row and field, not the sandbox's: the floors are far apart
                    // (V8 and CPython need ~768 MB to start, native CPython tens of MB).
                    py.push(
                        num_row(
                            FieldId::TPythonLocalMemory,
                            loc.t("ui.settings.field.python_memory"),
                            t.python_local_memory_mb,
                        )
                        .describe(loc.t("ui.settings.desc.python_local_memory")),
                    );
                }
                PythonMode::Wasmer => {
                    py.push(
                        row(
                            FieldId::TPythonNet,
                            loc.t("ui.settings.field.python_net"),
                            FieldKind::Toggle(t.python_net_enabled),
                        )
                        .describe(loc.t("ui.settings.desc.python_net")),
                    );
                    py.push(
                        num_field(
                            FieldId::TPythonWasmTimeout,
                            loc.t("ui.settings.field.python_timeout"),
                            t.python_wasm_timeout_secs,
                        )
                        .describe(loc.t("ui.settings.desc.python_timeout")),
                    );
                    py.push(
                        num_row(
                            FieldId::TPythonWasmMemory,
                            loc.t("ui.settings.field.python_memory"),
                            t.python_wasm_memory_mb,
                        )
                        .describe(loc.t("ui.settings.desc.python_memory")),
                    );
                }
            }
            py
        }));
        rows.extend(grouped(
            loc.t("ui.settings.group.video"),
            vec![
                text_row(
                    FieldId::VideoModel,
                    loc.t("ui.settings.field.model"),
                    &self.config.video.model_name,
                )
                .describe(loc.t("ui.settings.desc.video_model")),
                row(
                    FieldId::VideoResolution,
                    loc.t("ui.settings.field.video_resolution"),
                    FieldKind::Choice(video_resolution_label(
                        self.config.video.media_resolution,
                        loc,
                    )),
                )
                .describe(loc.t("ui.settings.desc.video_resolution")),
                num_field(
                    FieldId::VideoMaxMinutes,
                    loc.t("ui.settings.field.video_max_minutes"),
                    self.config.video.max_minutes,
                )
                .describe(loc.t("ui.settings.desc.video_max_minutes")),
                secret_row(
                    FieldId::VideoApiKey,
                    self.secret_field_present(FieldId::VideoApiKey),
                    &api_key_label(Some(CloudProvider::Gemini.display_name()), loc),
                    DESC_VIDEO_API_KEY,
                    loc,
                ),
                text_row(
                    FieldId::VideoApiKeyEnv,
                    &api_key_env_opt_label(CloudProvider::Gemini.display_name(), loc),
                    &self.config.video.api_key_env,
                )
                .describe(loc.t("ui.settings.desc.video_api_key_env")),
            ],
        ));
        rows.extend(grouped(
            loc.t("ui.settings.group.files"),
            vec![
                row(
                    FieldId::TFs,
                    loc.t("ui.settings.field.fs"),
                    FieldKind::Toggle(t.fs_enabled),
                )
                .describe(loc.t("ui.settings.desc.fs")),
                text_row(
                    FieldId::TFsRoot,
                    loc.t("ui.settings.field.fs_root"),
                    &t.fs_root,
                )
                .describe(loc.t("ui.settings.desc.fs_root")),
            ],
        ));
        // The code workspace (spec §9.12). A group in "Tools" rather than a
        // section of its own (design fork F9): three numbers do not make a
        // section, and the capability's own gate is a *project*, which the user
        // attaches from the chat — there is nothing to switch on here.
        let ws = &self.config.workspace;
        rows.extend(grouped(
            loc.t("ui.settings.group.workspace"),
            vec![
                num_field(
                    FieldId::WsTimeout,
                    loc.t("ui.settings.field.workspace_timeout"),
                    ws.command_timeout_secs,
                )
                .describe(loc.t("ui.settings.desc.workspace_timeout")),
                num_field(
                    FieldId::WsOutput,
                    loc.t("ui.settings.field.workspace_output"),
                    ws.output_limit_chars,
                )
                .describe(loc.t("ui.settings.desc.workspace_output")),
                num_field(
                    FieldId::WsMaxRounds,
                    loc.t("ui.settings.field.workspace_max_rounds"),
                    ws.max_rounds,
                )
                .describe(loc.t("ui.settings.desc.workspace_max_rounds")),
            ],
        ));
        rows
    }

    /// The "Plugins" section: the MCP host — the master switch, the server
    /// inventory (an editor for the selected server) and the live status of the
    /// ones that are running. Its own section rather than a group in "Tools"
    /// (docs/history/mcp-server-editor.md F1): "Tools" is a list of gates for
    /// built-in tools, while this is an inventory of external programs with
    /// per-server settings and a lifecycle. See spec §9.6.
    pub(super) fn plugin_fields(&self) -> Vec<FieldRow> {
        let loc = self.loc();
        let mut rows = grouped(
            loc.t("ui.settings.group.mcp_host"),
            vec![
                row(
                    FieldId::TMcpEnabled,
                    loc.t("ui.settings.field.mcp_enabled"),
                    FieldKind::Toggle(self.config.mcp.enabled),
                )
                .describe(loc.t("ui.settings.desc.mcp_enabled")),
                // Next to the master switch rather than in "Tools": it is about what a
                // *server* may hand the model, and it is the second thing to reach for
                // after deciding to run one at all (spec §9.10).
                row(
                    FieldId::TMcpImages,
                    loc.t("ui.settings.field.mcp_images"),
                    FieldKind::Toggle(self.config.tools.mcp_images),
                )
                .describe(loc.t("ui.settings.desc.mcp_images")),
            ],
        );
        rows.extend(grouped(
            loc.t("ui.settings.group.mcp_server"),
            self.mcp_server_fields(),
        ));
        // Import from another client's config. A file path rather than a pasted
        // blob: the file carries live tokens, and pasting one would leave it
        // visible on screen and in the editor undo buffer (§9, S6). The row shows
        // the last outcome as its value.
        rows.extend(grouped(
            loc.t("ui.settings.group.mcp_import"),
            vec![
                row(
                    FieldId::McpImport,
                    loc.t("ui.settings.field.mcp_import"),
                    FieldKind::Text(
                        self.mcp_import_result
                            .clone()
                            .unwrap_or_else(|| "—".to_string()),
                    ),
                )
                .describe(loc.t("ui.settings.desc.mcp_import")),
            ],
        ));
        // Server-status rows (read-only): ready/connecting/failure reason;
        // "catalog changed" is highlighted as a warning. Enter does what the row
        // needs — confirm the new catalog (TOFU) or reconnect (spec §9.6). Only
        // servers the host actually tried to start have one, so a disabled server
        // is absent here and edited above.
        if !self.mcp.servers.is_empty() {
            rows.extend(grouped(
                loc.t("ui.settings.group.mcp_status"),
                self.mcp
                    .servers
                    .iter()
                    .enumerate()
                    .map(|(idx, srv)| {
                        let status = match &srv.status {
                            // "ready · tools: N · in profile: K" — a server can
                            // be up while the model still sees nothing, because
                            // MCP tools are opt-in per profile (double opt-in,
                            // ADR 0007 R7). Saying only "ready" is how a user
                            // ends up adding a server and finding it does not
                            // work (docs/history/mcp-server-editor.md §5).
                            ServerStatus::Ready => loc.tf(
                                "ui.settings.mcp.ready",
                                &[
                                    ("n", &srv.tool_count.to_string()),
                                    ("k", &self.enabled_mcp_tools(&srv.id).to_string()),
                                ],
                            ),
                            ServerStatus::Connecting => loc.t("ui.settings.mcp.connecting").into(),
                            ServerStatus::NotConfigured => {
                                loc.t("ui.settings.mcp.not_configured").into()
                            }
                            ServerStatus::Disconnected(reason) => reason.clone(),
                        };
                        let mut r = row(FieldId::TMcpServer(idx), &srv.id, FieldKind::Text(status))
                            .describe(loc.t("ui.settings.desc.mcp_server"));
                        if srv.pending_catalog {
                            r.warn = true;
                            r.hint = Some(loc.t("ui.settings.mcp.confirm_hint"));
                        } else if srv.status == ServerStatus::Ready
                            && srv.tool_count > 0
                            && self.enabled_mcp_tools(&srv.id) == 0
                        {
                            // Up, and invisible to the model — say what is missing
                            // rather than let the user discover it in a chat.
                            r.hint = Some(loc.t("ui.settings.mcp.tools_off_hint"));
                        }
                        r
                    })
                    .collect(),
            ));
        }
        rows
    }

    /// The selected MCP server's fields. An empty inventory shows only the
    /// selector's placeholder — `Ctrl+N` creates the first entry (the
    /// impersonation-persona shape, spec §11.8).
    fn mcp_server_fields(&self) -> Vec<FieldRow> {
        let loc = self.loc();
        let Some(srv) = self.config.mcp.servers.get(self.mcp_server_idx) else {
            return vec![
                row(
                    FieldId::McpSelect,
                    loc.t("ui.settings.field.mcp_select"),
                    FieldKind::Choice(loc.t("ui.settings.value.no_servers").to_string()),
                )
                .describe(loc.t("ui.settings.desc.mcp_select")),
            ];
        };
        let mut rows = vec![
            row(
                FieldId::McpSelect,
                loc.t("ui.settings.field.mcp_select"),
                FieldKind::Choice(srv.id.clone()),
            )
            .describe(loc.t("ui.settings.desc.mcp_select")),
            row(
                FieldId::McpId,
                loc.t("ui.settings.field.mcp_id"),
                FieldKind::Text(srv.id.clone()),
            )
            .describe(loc.t("ui.settings.desc.mcp_id")),
            row(
                FieldId::McpCommand,
                loc.t("ui.settings.field.mcp_command"),
                FieldKind::Text(srv.command.clone()),
            )
            .describe(loc.t("ui.settings.desc.mcp_command")),
            row(
                FieldId::McpArgs,
                loc.t("ui.settings.field.mcp_args"),
                FieldKind::Text(join_args(&srv.args)),
            )
            .describe(loc.t("ui.settings.desc.mcp_args")),
            row(
                FieldId::McpEnv,
                loc.t("ui.settings.field.mcp_env"),
                FieldKind::Text(join_env_map(&srv.env)),
            )
            .describe(loc.t("ui.settings.desc.mcp_env")),
        ];
        // Where each declared variable's value comes from, and whether it is
        // there — right below the declaration that names it.
        rows.extend(self.mcp_env_value_rows(srv));
        rows.extend([
            row(
                FieldId::McpEnabled,
                loc.t("ui.settings.field.mcp_server_enabled"),
                FieldKind::Toggle(srv.enabled),
            )
            .describe(loc.t("ui.settings.desc.mcp_server_enabled")),
            row(
                FieldId::McpTimeout,
                loc.t("ui.settings.field.mcp_timeout"),
                FieldKind::Text(srv.tool_timeout_secs.to_string()),
            )
            .describe(loc.t("ui.settings.desc.mcp_timeout")),
            row(
                FieldId::McpMaxResult,
                loc.t("ui.settings.field.mcp_max_result"),
                FieldKind::Text(srv.max_result_chars.to_string()),
            )
            .describe(loc.t("ui.settings.desc.mcp_max_result")),
        ]);
        rows
    }

    /// One row per variable the selected server declares, in the map's order,
    /// saying where that variable's value comes from **and whether it is there**:
    ///
    /// * declared by name alone → an editable secret row (masked, machine-bound
    ///   storage, ADR 0008): "configured (this computer)" / "not set";
    /// * declared with a source (`API_KEY=OTHER_NAME`) → a **read-only** status:
    ///   whether that OS variable exists. No value field — its origin is already
    ///   given, and offering to store one too would be two answers to one
    ///   question (docs/history/mcp-server-editor.md §9.5b). It still gets a row,
    ///   because otherwise that route is mute and the only symptom of a missing
    ///   variable is the server not working (§9.5c).
    ///
    /// The index is the position in the whole map either way, so
    /// `secret_field_key` keeps resolving it.
    ///
    /// The label is the variable name — user data, so it can be longer than
    /// `LABEL_CAP`; the value column then grows to the cap and no further, and
    /// the name itself is clipped with "…" there (`render_field_line`) so the
    /// values keep their single vertical.
    fn mcp_env_value_rows(&self, srv: &McpServerConfig) -> Vec<FieldRow> {
        let loc = self.loc();
        srv.env
            .iter()
            .enumerate()
            .map(|(idx, (var, source))| {
                if source.is_empty() {
                    let key = SecretKey::McpEnv {
                        server: srv.id.clone(),
                        var: var.clone(),
                    };
                    return secret_row(
                        FieldId::McpEnvSecret(idx),
                        self.secret_present(Some(&key)),
                        var,
                        "ui.settings.desc.mcp_env_secret",
                        loc,
                    );
                }
                // The application's **own** environment — the one it was started
                // with, which is also what the child inherits. A variable set
                // after launch reads as missing until a restart, and the
                // description says so.
                let found = std::env::var(source).is_ok();
                let key = if found {
                    "ui.settings.value.mcp_source_found"
                } else {
                    "ui.settings.value.mcp_source_missing"
                };
                let mut r = row(
                    FieldId::McpEnvSource(idx),
                    var,
                    FieldKind::Text(loc.tf(key, &[("src", source)])),
                )
                .describe(loc.tf("ui.settings.desc.mcp_env_source", &[("src", source)]));
                r.warn = !found;
                r
            })
            .collect()
    }

    /// How many of the server's tools the **selected** profile has enabled. The
    /// count is per profile because that is what `effective_tool_ids` reads;
    /// with several profiles it follows the one being edited in "Profiles".
    fn enabled_mcp_tools(&self, server: &str) -> usize {
        let prefix = format!("mcp__{server}__");
        let Some(profile) = self.profiles.get(self.profile_idx) else {
            return 0;
        };
        profile
            .enabled_tools
            .iter()
            .filter(|t| t.starts_with(&prefix))
            .count()
    }

    /// Keeps the MCP-server selection inside the list (it can shrink from a
    /// re-emit or an undo).
    fn clamp_mcp_server_idx(&mut self) {
        let n = self.config.mcp.servers.len();
        if n > 0 && self.mcp_server_idx >= n {
            self.mcp_server_idx = n - 1;
        }
    }

    /// The "Memory" section: knowledge-base chunking (RAG), notes, "self-model".
    /// The "Data" section: settings about the data root itself rather than about
    /// the agent — currently the backup password (spec §12.3). Its own section
    /// because none of the others is about stored data, and a security setting
    /// filed under an unrelated heading is a setting nobody finds.
    pub(super) fn data_fields(&self) -> Vec<FieldRow> {
        let loc = self.loc();
        grouped(
            loc.t("ui.settings.group.backup"),
            vec![secret_row(
                FieldId::BackupPassword,
                self.secret_present(Some(&SecretKey::BackupPassword)),
                loc.t("ui.settings.field.backup_password"),
                "ui.settings.desc.backup_password",
                loc,
            )],
        )
    }

    pub(super) fn memory_fields(&self) -> Vec<FieldRow> {
        let loc = self.loc();
        // "Context" first: history compression is about the **current**
        // conversation, ahead of the long-term memory groups that follow.
        let mut rows = grouped(
            loc.t("ui.settings.group.context"),
            vec![
                row(
                    FieldId::CompactEnabled,
                    loc.t("ui.settings.field.compact_enabled"),
                    FieldKind::Toggle(self.config.compaction.enabled),
                )
                .describe(loc.t("ui.settings.desc.compact_enabled")),
                num_field(
                    FieldId::CompactWords,
                    loc.t("ui.settings.field.compact_words"),
                    self.config.compaction.summary_words,
                )
                .describe(loc.t("ui.settings.desc.compact_words")),
                num_field(
                    FieldId::CompactTail,
                    loc.t("ui.settings.field.compact_tail"),
                    self.config.compaction.tail_tokens,
                )
                .describe(loc.t("ui.settings.desc.compact_tail")),
                num_field(
                    FieldId::CompactThreshold,
                    loc.t("ui.settings.field.compact_threshold"),
                    self.config.compaction.threshold_pct as usize,
                )
                .describe(loc.t("ui.settings.desc.compact_threshold")),
                num_field(
                    FieldId::CompactContext,
                    loc.t("ui.settings.field.compact_context"),
                    self.config.compaction.context_tokens.unwrap_or(0),
                )
                .describe(loc.t("ui.settings.desc.compact_context")),
                num_field(
                    FieldId::CompactPage,
                    loc.t("ui.settings.field.compact_page"),
                    self.config.compaction.page_tokens,
                )
                .describe(loc.t("ui.settings.desc.compact_page")),
            ],
        );
        rows.extend(grouped(
            loc.t("ui.settings.group.rag"),
            vec![
                row(
                    FieldId::RagTarget,
                    loc.t("ui.settings.field.rag_target"),
                    FieldKind::Text(self.config.rag.chunk_target_chars.to_string()),
                )
                .describe(loc.t("ui.settings.desc.rag_target")),
                row(
                    FieldId::RagOverlap,
                    loc.t("ui.settings.field.rag_overlap"),
                    FieldKind::Text(self.config.rag.chunk_overlap_chars.to_string()),
                )
                .describe(loc.t("ui.settings.desc.rag_overlap")),
                row(
                    FieldId::RagMax,
                    loc.t("ui.settings.field.rag_max"),
                    FieldKind::Text(self.config.rag.chunk_max_chars.to_string()),
                )
                .describe(loc.t("ui.settings.desc.rag_max")),
            ],
        ));
        rows.extend(grouped(
            loc.t("ui.settings.group.attachments"),
            vec![
                row(
                    FieldId::AttachMaxFile,
                    loc.t("ui.settings.field.attach_max_file"),
                    FieldKind::Text(self.config.attachments.max_file_tokens.to_string()),
                )
                .describe(loc.t("ui.settings.desc.attach_max_file")),
                row(
                    FieldId::AttachMaxTotal,
                    loc.t("ui.settings.field.attach_max_total"),
                    FieldKind::Text(self.config.attachments.max_total_tokens.to_string()),
                )
                .describe(loc.t("ui.settings.desc.attach_max_total")),
                row(
                    FieldId::AttachExcerpt,
                    loc.t("ui.settings.field.attach_excerpt"),
                    FieldKind::Text(self.config.attachments.excerpt_tokens.to_string()),
                )
                .describe(loc.t("ui.settings.desc.attach_excerpt")),
                row(
                    FieldId::AttachPage,
                    loc.t("ui.settings.field.attach_page"),
                    FieldKind::Text(self.config.attachments.page_tokens.to_string()),
                )
                .describe(loc.t("ui.settings.desc.attach_page")),
            ],
        ));
        // Image attachments get their own group rather than joining the one above:
        // those budgets are counted in estimated tokens, these in images, megabytes
        // and pixels, and one header over both would read as a single scale.
        rows.extend(grouped(
            loc.t("ui.settings.group.images"),
            vec![
                row(
                    FieldId::ImageMaxCount,
                    loc.t("ui.settings.field.image_max_count"),
                    FieldKind::Text(self.config.images.max_count.to_string()),
                )
                .describe(loc.t("ui.settings.desc.image_max_count")),
                // Shown in MB, stored in bytes (`mb_to_bytes`/`bytes_to_mb`).
                row(
                    FieldId::ImageMaxBytes,
                    loc.t("ui.settings.field.image_max_bytes"),
                    FieldKind::Text(bytes_to_mb(self.config.images.max_bytes).to_string()),
                )
                .describe(loc.t("ui.settings.desc.image_max_bytes")),
                row(
                    FieldId::ImageDownscale,
                    loc.t("ui.settings.field.image_downscale"),
                    FieldKind::Text(self.config.images.downscale_px.to_string()),
                )
                .describe(loc.t("ui.settings.desc.image_downscale")),
            ],
        ));
        rows.extend(grouped(
            loc.t("ui.settings.group.notes"),
            vec![
                row(
                    FieldId::NotesAutoConsolidate,
                    loc.t("ui.settings.field.notes_auto_consolidate"),
                    FieldKind::Text(self.config.notes.auto_consolidate_every.to_string()),
                )
                .describe(loc.t("ui.settings.desc.notes_auto_consolidate")),
                row(
                    FieldId::NotesRecallIncludesSelf,
                    loc.t("ui.settings.field.notes_recall_self"),
                    FieldKind::Toggle(self.config.notes.recall_includes_self),
                )
                .describe(loc.t("ui.settings.desc.notes_recall_self")),
            ],
        ));
        rows.extend(grouped(
            loc.t("ui.settings.group.self_model"),
            vec![
                row(
                    FieldId::SmMaxNarrative,
                    loc.t("ui.settings.field.sm_max_narrative"),
                    FieldKind::Text(self.config.self_model.max_narrative.to_string()),
                )
                .describe(loc.t("ui.settings.desc.sm_max_narrative")),
                row(
                    FieldId::SmNarrativeInPrompt,
                    loc.t("ui.settings.field.sm_narrative_in_prompt"),
                    FieldKind::Text(self.config.self_model.narrative_in_prompt.to_string()),
                )
                .describe(loc.t("ui.settings.desc.sm_narrative_in_prompt")),
                row(
                    FieldId::SmPromptCap,
                    loc.t("ui.settings.field.sm_prompt_cap"),
                    FieldKind::Text(self.config.self_model.prompt_cap.to_string()),
                )
                .describe(loc.t("ui.settings.desc.sm_prompt_cap")),
                row(
                    FieldId::SmSummaryTarget,
                    loc.t("ui.settings.field.sm_summary_target"),
                    FieldKind::Text(self.config.self_model.summary_target_chars.to_string()),
                )
                .describe(loc.t("ui.settings.desc.sm_summary_target")),
                row(
                    FieldId::SmAutoReflect,
                    loc.t("ui.settings.field.sm_auto_reflect"),
                    FieldKind::Text(self.config.self_model.auto_reflect_every.to_string()),
                )
                .describe(loc.t("ui.settings.desc.sm_auto_reflect")),
                row(
                    FieldId::SmAutoConsolidate,
                    loc.t("ui.settings.field.sm_auto_consolidate"),
                    FieldKind::Text(self.config.self_model.auto_consolidate_every.to_string()),
                )
                .describe(loc.t("ui.settings.desc.sm_auto_consolidate")),
                row(
                    FieldId::SmProtocol,
                    loc.t("ui.settings.field.sm_protocol"),
                    FieldKind::Toggle(self.config.self_model.maintenance_protocol),
                )
                .describe(loc.t("ui.settings.desc.sm_protocol")),
            ],
        ));
        rows
    }

    pub(super) fn interface_fields(&self) -> Vec<FieldRow> {
        let loc = self.loc();
        let i = &self.config.interface;
        let mut rows = grouped(
            loc.t("ui.settings.group.appearance"),
            vec![
                row(
                    FieldId::ITheme,
                    loc.t("ui.settings.field.theme"),
                    FieldKind::Choice(theme_label(i.theme, loc)),
                )
                .describe(loc.t("ui.settings.desc.theme")),
                row(
                    FieldId::ILanguage,
                    loc.t("ui.settings.field.language"),
                    FieldKind::Choice(i.language.label().to_string()),
                )
                .describe(loc.t("ui.settings.desc.language")),
                row(
                    FieldId::ICompat,
                    loc.t("ui.settings.field.compat"),
                    FieldKind::Toggle(i.terminal_compat),
                )
                .describe(loc.t("ui.settings.desc.compat")),
                row(
                    FieldId::ITableSeparators,
                    loc.t("ui.settings.field.table_separators"),
                    FieldKind::Toggle(i.table_row_separators),
                )
                .describe(loc.t("ui.settings.desc.table_separators")),
                row(
                    FieldId::IMermaid,
                    loc.t("ui.settings.field.mermaid"),
                    FieldKind::Toggle(i.render_mermaid),
                )
                .describe(loc.t("ui.settings.desc.mermaid")),
                row(
                    FieldId::IModelName,
                    loc.t("ui.settings.field.model_name"),
                    FieldKind::Toggle(i.show_model_name),
                )
                // NOT `desc.model_name` — that key describes the *cloud model
                // name* field: reusing it here duplicated the key in the
                // bundles, and JSON parsing silently keeps the later entry, so
                // both fields showed this toggle's text. The bundle gate
                // `builtin_bundles_have_no_duplicate_keys` guards the class.
                .describe(loc.t("ui.settings.desc.show_model_name")),
                row(
                    FieldId::IClipboardOsc52,
                    loc.t("ui.settings.field.osc52"),
                    FieldKind::Choice(osc52_label(i.clipboard_osc52, loc)),
                )
                .describe(loc.t("ui.settings.desc.osc52")),
                row(
                    FieldId::ISmNoteOrder,
                    loc.t("ui.settings.field.sm_note_order"),
                    FieldKind::Choice(note_order_label(i.self_model_note_order, loc)),
                )
                .describe(loc.t("ui.settings.desc.sm_note_order")),
            ],
        );
        rows.extend(grouped(
            loc.t("ui.settings.group.spelling"),
            vec![
                row(
                    FieldId::ISpell,
                    loc.t("ui.settings.field.spell"),
                    FieldKind::Toggle(i.spellcheck_enabled),
                ),
                row(
                    FieldId::IDicts,
                    loc.t("ui.settings.field.dicts"),
                    FieldKind::Text(if i.selected_dictionaries.is_empty() {
                        loc.t("ui.settings.value.all").to_string()
                    } else {
                        i.selected_dictionaries.join(", ")
                    }),
                ),
            ],
        ));
        rows.extend(grouped(
            loc.t("ui.settings.group.behavior"),
            vec![
                row(
                    FieldId::IConfirmKeys,
                    loc.t("ui.settings.field.confirm_keys"),
                    FieldKind::Toggle(i.confirm_destructive_keys),
                )
                .describe(loc.t("ui.settings.desc.confirm_keys")),
                row(
                    FieldId::IAutoTitle,
                    loc.t("ui.settings.field.auto_title"),
                    FieldKind::Choice(auto_title_label(i.auto_title, loc)),
                )
                .describe(loc.t("ui.settings.desc.auto_title")),
            ],
        ));
        // Labels read as a continuation of the group header ("Copy conversation … —
        // with thoughts"): the shared "Copy" prefix moved into the header so
        // long names don't push away the value column (see LABEL_CAP).
        rows.extend(grouped(
            loc.t("ui.settings.group.copy"),
            vec![
                row(
                    FieldId::ICopyThoughts,
                    loc.t("ui.settings.field.copy_thoughts"),
                    FieldKind::Toggle(self.config.copy.copy_thoughts),
                )
                .describe(loc.t("ui.settings.desc.copy_thoughts")),
                row(
                    FieldId::ICopyToolCalls,
                    loc.t("ui.settings.field.copy_tool_calls"),
                    FieldKind::Toggle(self.config.copy.copy_tool_calls),
                )
                .describe(loc.t("ui.settings.desc.copy_tool_calls")),
                row(
                    FieldId::ICopyToolResults,
                    loc.t("ui.settings.field.copy_tool_results"),
                    FieldKind::Toggle(self.config.copy.copy_tool_results),
                )
                .describe(loc.t("ui.settings.desc.copy_tool_results")),
            ],
        ));
        rows
    }

    pub(super) fn profile_fields(&self) -> Vec<FieldRow> {
        self.profile_fields_for(self.profile_sub)
    }

    /// The "Profiles" section's fields for a given subsection (for enumeration during search).
    ///
    /// The two subsections edit **different lists**: "Assistant" — the AI-interlocutor
    /// profiles (`profiles.json`), "Impersonation" — the user personas
    /// (`config.impersonation_profiles`); each has its own selector, name, and system
    /// message. The assistant profile ties the two together with the
    /// [`FieldId::PImpProfile`] reference. See spec §11.8.
    pub(super) fn profile_fields_for(&self, profile_sub: Subsection) -> Vec<FieldRow> {
        let loc = self.loc();
        // ProfileSub — the subsection selector (tab strip, field 0, not drawn as a list row).
        let mut rows = vec![
            row(
                FieldId::ProfileSub,
                loc.t("ui.settings.field.subsection"),
                FieldKind::Choice(profile_sub.label(loc)),
            )
            .describe(loc.t(DESC_SUBSECTION)),
        ];
        if profile_sub == Subsection::Impersonation {
            rows.extend(self.impersonation_profile_fields());
            return rows;
        }
        let Some(p) = self.profiles.get(self.profile_idx) else {
            rows.push(row(
                FieldId::PSelect,
                loc.t("ui.settings.field.profile"),
                FieldKind::Choice(loc.t("ui.settings.value.no_profiles").to_string()),
            ));
            return rows;
        };
        rows.extend([
            row(
                FieldId::PSelect,
                loc.t("ui.settings.field.profile"),
                FieldKind::Choice(p.name.clone()),
            ),
            row(
                FieldId::PName,
                loc.t("ui.settings.field.name"),
                FieldKind::Text(p.name.clone()),
            ),
        ]);
        match profile_sub {
            Subsection::Assistant => {
                // Scaffold language (axis A): Choice; locked once the profile has
                // data (`language_locked`). See docs/history/i18n.md.
                let lang_locked = self.language_locked.contains(&p.id);
                let mut lang_row = row(
                    FieldId::PLanguage,
                    loc.t("ui.settings.field.profile_language"),
                    FieldKind::Choice(p.language.label().to_string()),
                )
                .describe(loc.t(DESC_PROFILE_LANGUAGE));
                if lang_locked {
                    lang_row.warn = true;
                    lang_row.hint = Some(loc.t("ui.settings.hint.language_locked"));
                }
                rows.extend(grouped(
                    loc.t("ui.settings.group.persona"),
                    vec![
                        lang_row,
                        row(
                            FieldId::PSystem,
                            loc.t("ui.settings.field.system_message"),
                            FieldKind::Text(p.default_system_message.clone()),
                        ),
                        row(
                            FieldId::PGreeting,
                            loc.t("ui.settings.field.greeting"),
                            FieldKind::Text(p.greeting.clone().unwrap_or_default()),
                        ),
                        row(
                            FieldId::PUserName,
                            loc.t("ui.settings.field.user_name"),
                            FieldKind::Text(p.character_names.user.clone()),
                        )
                        .describe(loc.t("ui.settings.desc.user_name")),
                        row(
                            FieldId::PAssistantName,
                            loc.t("ui.settings.field.assistant_name"),
                            FieldKind::Text(p.character_names.assistant.clone()),
                        )
                        .describe(loc.t("ui.settings.desc.assistant_name")),
                        row(
                            FieldId::PImpProfile,
                            loc.t("ui.settings.field.imp_profile"),
                            FieldKind::Choice(self.imp_profile_label(p, loc)),
                        )
                        .describe(loc.t("ui.settings.desc.imp_profile")),
                    ],
                ));
                // Tool toggles: laid out by semantic group
                // (`ToolInfo.group`), with a short description and an honest gate.
                // The `PTool` index is the position in `tool_catalog()` (the source of
                // truth for `toggle_profile_tool`); we group the DISPLAY order by a stable
                // sort on `ToolGroup` (Ord), without touching indices.
                let mut indexed: Vec<(usize, ToolInfo)> =
                    self.tool_catalog().into_iter().enumerate().collect();
                indexed.sort_by_key(|(_, info)| info.group);
                for (idx, info) in indexed {
                    let on = p.enabled_tools.iter().any(|t| t == &info.id);
                    let gate = info.gate;
                    let gated_off = on && gate.is_some_and(|g| self.gate_disabled(g));
                    let mut r = row(FieldId::PTool(idx), &info.id, FieldKind::Toggle(on));
                    r.group = loc.get(info.group.i18n_key()).unwrap_or(info.group.title());
                    r.warn = gated_off;
                    r.warn_note = gated_off
                        .then(|| gate.map(|g| gate_warn_note(g, loc)))
                        .flatten();
                    r.hint = if gated_off {
                        gate.map(|g| gate_hint(g, loc))
                    } else {
                        (!info.label.is_empty()).then_some(
                            loc.get(&format!("ui.tool.label.{}", info.id))
                                .unwrap_or(info.label),
                        )
                    };
                    // The full description of an MCP tool (the server's own text) — in
                    // the bottom panel when focused: mandatory description visibility is an
                    // antidote to tool-poisoning (spec §9.6).
                    r.description = info.description.clone();
                    rows.push(r);
                }
            }
            // Handled above (a different list entirely) — kept exhaustive.
            Subsection::Impersonation => {}
        }
        rows
    }

    /// The "Impersonation" subsection's fields: the user-persona list
    /// (`config.impersonation_profiles`) — selector, name, system message. No tools
    /// (spec §11.8). An empty list shows only the selector's placeholder; `Ctrl+N`
    /// creates the first entry.
    fn impersonation_profile_fields(&self) -> Vec<FieldRow> {
        let loc = self.loc();
        let Some(ip) = self.config.impersonation_profiles.get(self.imp_profile_idx) else {
            return vec![
                row(
                    FieldId::IpSelect,
                    loc.t("ui.settings.field.profile"),
                    FieldKind::Choice(loc.t("ui.settings.value.no_profiles").to_string()),
                )
                .describe(loc.t("ui.settings.desc.imp_profile_select")),
            ];
        };
        let mut rows = vec![
            row(
                FieldId::IpSelect,
                loc.t("ui.settings.field.profile"),
                FieldKind::Choice(ip.name.clone()),
            )
            .describe(loc.t("ui.settings.desc.imp_profile_select")),
            row(
                FieldId::IpName,
                loc.t("ui.settings.field.name"),
                FieldKind::Text(ip.name.clone()),
            ),
        ];
        rows.extend(grouped(
            loc.t("ui.settings.group.persona"),
            vec![
                row(
                    FieldId::IpSystem,
                    loc.t("ui.settings.field.system_message"),
                    FieldKind::Text(ip.system_message.clone()),
                )
                .describe(loc.t("ui.settings.desc.imp_system_message")),
            ],
        ));
        rows
    }

    /// The label of the impersonation profile an assistant profile references
    /// (a dangling id reads as "not set" — the shared default text applies).
    pub(super) fn imp_profile_label(&self, p: &Profile, loc: &'static Locale) -> String {
        p.impersonation_profile_id
            .and_then(|id| {
                self.config
                    .impersonation_profiles
                    .iter()
                    .find(|ip| ip.id == id)
            })
            .map(|ip| ip.name.clone())
            .unwrap_or_else(|| loc.t("ui.settings.value.imp_profile_none").to_string())
    }

    /// Whether the tool's global gate is off (then the tool is unavailable to the
    /// model, even if enabled in the profile).
    pub(super) fn gate_disabled(&self, gate: ToolGate) -> bool {
        match gate {
            ToolGate::Web => !self.config.tools.web_enabled,
            ToolGate::Python => !self.config.tools.python_enabled,
            ToolGate::Fs => !self.config.tools.fs_enabled,
            ToolGate::Mcp => !self.config.mcp.enabled,
            ToolGate::Background => !self.config.tools.subagent_background,
        }
    }

    /// The subsection's sampling cloud provider (`None` — a local engine). For
    /// impersonation in `shared` mode, the effective provider is the assistant's engine.
    pub(super) fn sampling_cloud_provider(&self, imp: bool) -> Option<CloudProvider> {
        if imp {
            match self.config.impersonation_engine.mode {
                ImpersonationMode::Shared => self.config.engine.mode.cloud_provider(),
                m => m.cloud_provider(),
            }
        } else {
            self.config.engine.mode.cloud_provider()
        }
    }
}

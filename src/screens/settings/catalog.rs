//! Экран настроек — каталог полей: конструктор, построители полей секций и
//! подсекций (модель/семплинг/инструменты/память/интерфейс/профили), гейты
//! доступности. Часть модуля [`super`]; разбито из settings.rs.

use super::helpers::*;
use super::*;

impl SettingsScreen {
    /// Создаёт экран из снимка настроек (конфиг + видимые профили + профили с
    /// заблокированным языком каркаса).
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
            model_sub: ModelTab::Assistant,
            sampling_sub: Subsection::Assistant,
            profile_sub: Subsection::Assistant,
            editor: None,
            search: None,
            choice: None,
            statuses: ServerStatuses {
                chat: ServerStatus::NotConfigured,
                embed: ServerStatus::NotConfigured,
                impersonation: ServerStatus::NotConfigured,
            },
            language_locked,
            mcp: Default::default(),
            api_keys_present: Vec::new(),
        }
    }

    /// Обновляет снимок статусов серверов (чипы в секции «Модель/сервер»). Вызывается
    /// `app` при создании экрана и по событию `ServerStatus`.
    pub fn set_server_statuses(&mut self, statuses: ServerStatuses) {
        self.statuses = statuses;
    }

    /// Обновляет рабочую копию из переэмита настроек (после create/delete профиля
    /// или эха правки). Навигация и активный редактор сохраняются.
    pub fn refresh(
        &mut self,
        config: AppConfig,
        profiles: Vec<Profile>,
        language_locked: Vec<uuid::Uuid>,
    ) {
        self.config = config;
        self.profiles = profiles;
        self.language_locked = language_locked;
        if !self.profiles.is_empty() && self.profile_idx >= self.profiles.len() {
            self.profile_idx = self.profiles.len() - 1;
        }
    }

    pub(super) fn section(&self) -> Section {
        SECTIONS[self.section_idx]
    }

    /// Каталог всех известных инструментов (для тумблеров в профиле) — включая
    /// опциональные (по умолчанию выключенные) и **динамические** инструменты
    /// MCP-серверов (снимок из события `Settings`, дописываются в конец — индексы
    /// `PTool` статической части стабильны). Метаданные (группа/лейбл/гейт)
    /// снимаются с самих инструментов (единый источник — трейт `Tool`). См. spec §9.3.
    pub(super) fn tool_catalog(&self) -> Vec<ToolInfo> {
        let mut catalog = crate::features::tools::tool_catalog();
        catalog.extend(self.mcp.tools.iter().cloned());
        catalog
    }

    /// Обновляет снимок MCP-хоста (из события `Settings`: динамический каталог
    /// инструментов + статусы серверов; пуст, пока серверы не поднялись/выключены).
    pub fn set_mcp(&mut self, mcp: crate::features::tools::mcp::McpSnapshot) {
        self.mcp = mcp;
    }

    /// Обновляет список провайдеров с сохранённым на этой машине API-ключом (из
    /// снимка `Settings`) — по нему поле «API-ключ» показывает статус. Сами ключи в
    /// UI не передаются. См. `shared::secrets`.
    pub fn set_api_keys_present(&mut self, present: Vec<CloudProvider>) {
        self.api_keys_present = present;
    }

    /// Сохранён ли на этой машине ключ провайдера (для статуса поля «API-ключ»).
    pub(super) fn api_key_present(&self, provider: Option<CloudProvider>) -> bool {
        provider.is_some_and(|p| self.api_keys_present.contains(&p))
    }

    /// Провайдер, к которому относится поле ввода API-ключа (по активному режиму
    /// соответствующего движка), либо `None` — это не поле ключа. Ключ общий для
    /// чата/имперсонации/эмбеддингов одного провайдера, поэтому важен именно
    /// провайдер, а не слот. См. docs/research/api-key-storage.md.
    pub(super) fn api_key_field_provider(&self, id: FieldId) -> Option<CloudProvider> {
        match id {
            FieldId::XApiKey => self.config.engine.mode.cloud_provider(),
            FieldId::IxApiKey => self.config.impersonation_engine.mode.cloud_provider(),
            FieldId::EApiKey => self.config.embed.mode.cloud_provider(),
            FieldId::TtsApiKey => self.config.tts.mode.cloud_provider(),
            _ => None,
        }
    }

    // ---------- построение полей текущей секции ----------

    pub(super) fn fields(&self) -> Vec<FieldRow> {
        match self.section() {
            Section::Model => self.model_fields(),
            Section::Sampling => self.sampling_fields(),
            Section::Tools => self.tool_fields(),
            Section::Memory => self.memory_fields(),
            Section::Profiles => self.profile_fields(),
            Section::Interface => self.interface_fields(),
        }
    }

    pub(super) fn model_fields(&self) -> Vec<FieldRow> {
        self.model_fields_for(self.model_sub)
    }

    /// Поля секции «Модель» для заданной подсекции (для перечисления при поиске —
    /// [`SettingsScreen::model_fields`] строит их для активной подсекции).
    pub(super) fn model_fields_for(&self, model_sub: ModelTab) -> Vec<FieldRow> {
        let loc = self.loc();
        // Селектор подсекции (таб-стрип) — всегда поле 0; в списке он не рисуется.
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
                // Видимость полей зависит от режима (ADR 0004): для облака показываем
                // лишь модель/ключ/опц. base URL, для managed — параметры llama-server.
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
                            .describe(loc.t(DESC_MODEL_NAME)),
                            text_row(
                                FieldId::XApiKeyEnv,
                                loc.t("ui.settings.field.api_key_env_opt"),
                                &x.external.api_key_env,
                            )
                            .describe(loc.t(DESC_EXT_API_KEY_ENV)),
                        ],
                    )),
                    ServerMode::OpenAi | ServerMode::Gemini | ServerMode::Claude => {
                        rows.extend(grouped(
                            loc.t("ui.settings.group.provider"),
                            cloud_rows(
                                x.cloud(),
                                FieldId::XModelName,
                                FieldId::XApiKey,
                                FieldId::XApiKeyEnv,
                                FieldId::XUrl,
                                self.api_key_present(x.mode.cloud_provider()),
                                loc,
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
                        loc.t("ui.settings.field.mode"),
                        FieldKind::Choice(imp_mode_label(x.mode)),
                    )
                    .describe(loc.t(DESC_IMP_MODE)),
                ];
                match x.mode {
                    // Shared переиспользует движок ассистента — собственных полей нет.
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
                            .describe(loc.t(DESC_MODEL_NAME)),
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
                    | ImpersonationMode::Claude => rows.extend(grouped(
                        loc.t("ui.settings.group.provider"),
                        cloud_rows(
                            x.cloud(),
                            FieldId::IxModelName,
                            FieldId::IxApiKey,
                            FieldId::IxApiKeyEnv,
                            FieldId::IxUrl,
                            self.api_key_present(x.mode.cloud_provider()),
                            loc,
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
                            .describe(loc.t(DESC_MODEL_NAME)),
                            text_row(
                                FieldId::EApiKeyEnv,
                                loc.t("ui.settings.field.api_key_env_opt"),
                                &e.external.api_key_env,
                            )
                            .describe(loc.t(DESC_EXT_API_KEY_ENV)),
                        ],
                    )),
                    // Claude поля показывает, но Anthropic не умеет embeddings —
                    // супервайзер вернёт «недоступно» (RAG отключится). ADR 0004.
                    ServerMode::OpenAi | ServerMode::Gemini | ServerMode::Claude => {
                        let none = CloudSettings::default();
                        let c = e.cloud().unwrap_or(&none);
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
                                    self.api_key_present(e.mode.cloud_provider()),
                                    loc,
                                ),
                                text_row(
                                    FieldId::EApiKeyEnv,
                                    loc.t("ui.settings.field.api_key_env"),
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
                rows
            }
            ModelTab::Tts => {
                // Озвучивание — независимый слот (у Anthropic TTS нет): свой
                // провайдер, свои модель/голос. Ключ облака — общий с чатом
                // (ADR 0008), вводить заново не нужно. См. spec §11.9.
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
                            FieldId::TtsApiKeyEnv,
                            loc.t("ui.settings.field.api_key_env_opt"),
                            &t.external.api_key_env,
                        )
                        .describe(loc.t(DESC_EXT_API_KEY_ENV)),
                    ],
                    _ => {
                        let none = TtsCloudSettings::default();
                        let c = t.cloud().unwrap_or(&none);
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
                                FieldId::TtsInstructions,
                                loc.t("ui.settings.field.tts_instructions"),
                                &c.instructions,
                            )
                            .describe(loc.t("ui.settings.desc.tts_instructions")),
                            api_key_row(
                                FieldId::TtsApiKey,
                                self.api_key_present(t.mode.cloud_provider()),
                                loc,
                            ),
                            text_row(
                                FieldId::TtsApiKeyEnv,
                                loc.t("ui.settings.field.api_key_env"),
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

    /// Поля секции «Семплинг» для заданной подсекции (для перечисления при поиске).
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
                    FieldId::TSubMaxTokens,
                    loc.t("ui.settings.field.sub_max_tokens"),
                    FieldKind::Text(t.subagent_max_tokens.to_string()),
                )
                .describe(loc.t("ui.settings.desc.sub_max_tokens")),
                row(
                    FieldId::TSubTimeout,
                    loc.t("ui.settings.field.sub_timeout"),
                    FieldKind::Text(t.subagent_timeout_secs.to_string()),
                )
                .describe(loc.t("ui.settings.desc.sub_timeout")),
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
            ],
        ));
        rows.extend(grouped("Python", {
            let mut py = vec![
                row(
                    FieldId::TPythonMode,
                    loc.t("ui.settings.field.python_mode"),
                    FieldKind::Choice(t.python_mode.label().to_string()),
                )
                .describe(loc.t("ui.settings.desc.python_mode")),
                row(
                    FieldId::TPython,
                    loc.t("ui.settings.field.python"),
                    FieldKind::Toggle(t.python_enabled),
                )
                .describe(loc.t("ui.settings.desc.python")),
            ];
            match t.python_mode {
                PythonMode::Local => py.push(
                    text_row(
                        FieldId::TPythonPath,
                        loc.t("ui.settings.field.python_path"),
                        &t.python_path,
                    )
                    .describe(loc.t("ui.settings.desc.python_path")),
                ),
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
        rows.extend(grouped(loc.t("ui.tool.group.plugins"), {
            let mut mcp_rows = vec![
                row(
                    FieldId::TMcpEnabled,
                    loc.t("ui.settings.field.mcp_enabled"),
                    FieldKind::Toggle(self.config.mcp.enabled),
                )
                .describe(loc.t("ui.settings.desc.mcp_enabled")),
            ];
            // Строки статусов серверов (read-only): готов/подключение/причина
            // отказа; «каталог изменился» подсвечивается предупреждением, Enter
            // подтверждает новый каталог (TOFU-переподтверждение, spec §9.6).
            for (idx, srv) in self.mcp.servers.iter().enumerate() {
                let status = match &srv.status {
                    ServerStatus::Ready => loc.tf(
                        "ui.settings.mcp.ready",
                        &[("n", &srv.tool_count.to_string())],
                    ),
                    ServerStatus::Connecting => loc.t("ui.settings.mcp.connecting").into(),
                    ServerStatus::NotConfigured => loc.t("ui.settings.mcp.not_configured").into(),
                    ServerStatus::Disconnected(reason) => reason.clone(),
                };
                let mut r = row(FieldId::TMcpServer(idx), &srv.id, FieldKind::Text(status))
                    .describe(loc.t("ui.settings.desc.mcp_server"));
                if srv.pending_catalog {
                    r.warn = true;
                    r.hint = Some(loc.t("ui.settings.mcp.confirm_hint"));
                }
                mcp_rows.push(r);
            }
            mcp_rows
        }));
        rows
    }

    /// Секция «Память»: чанкинг базы знаний (RAG), заметки, «модель себя».
    pub(super) fn memory_fields(&self) -> Vec<FieldRow> {
        let loc = self.loc();
        let mut rows = grouped(
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
        );
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
                ),
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
            ],
        ));
        // Подписи читаются как продолжение заголовка группы («Копирование … —
        // с «мыслями»»): общий префикс «Копировать» ушёл в заголовок, чтобы
        // длинные имена не отгоняли колонку значений (см. LABEL_CAP).
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

    /// Поля секции «Профили» для заданной подсекции (для перечисления при поиске).
    pub(super) fn profile_fields_for(&self, profile_sub: Subsection) -> Vec<FieldRow> {
        let loc = self.loc();
        let Some(p) = self.profiles.get(self.profile_idx) else {
            return vec![row(
                FieldId::PSelect,
                loc.t("ui.settings.field.profile"),
                FieldKind::Choice(loc.t("ui.settings.value.no_profiles").to_string()),
            )];
        };
        // ProfileSub — селектор подсекции (таб-стрип, поле 0, в списке не рисуется);
        // выбор профиля и имя — секционные (общие для обеих подсекций).
        let mut rows = vec![
            row(
                FieldId::ProfileSub,
                loc.t("ui.settings.field.subsection"),
                FieldKind::Choice(profile_sub.label(loc)),
            )
            .describe(loc.t(DESC_SUBSECTION)),
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
        ];
        match profile_sub {
            Subsection::Assistant => {
                // Язык служебного каркаса (ось A): Choice; блокируется, когда у
                // профиля появились данные (`language_locked`). См. docs/history/i18n.md.
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
                    ],
                ));
                // Тумблеры инструментов: раскладываем по смысловым группам
                // (`ToolInfo.group`), с коротким описанием и честным гейтом.
                // Индекс `PTool` — позиция в `tool_catalog()` (источник истины для
                // `toggle_profile_tool`); порядок ПОКАЗА группируем стабильной
                // сортировкой по `ToolGroup` (Ord), не трогая индексы.
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
                    r.hint = if gated_off {
                        gate.map(|g| gate_hint(g, loc))
                    } else {
                        (!info.label.is_empty()).then_some(
                            loc.get(&format!("ui.tool.label.{}", info.id))
                                .unwrap_or(info.label),
                        )
                    };
                    // Полное описание MCP-инструмента (текст сервера) — в нижнюю
                    // панель при фокусе: обязательная видимость описаний — антидот
                    // tool-poisoning (spec §9.6).
                    r.description = info.description.clone();
                    rows.push(r);
                }
            }
            // В имперсонации инструментов нет (spec §11.8) — только сис. сообщение.
            Subsection::Impersonation => {
                rows.extend(grouped(
                    loc.t("ui.settings.group.persona"),
                    vec![row(
                        FieldId::PImpSystem,
                        loc.t("ui.settings.field.system_message"),
                        FieldKind::Text(p.impersonation_system_message.clone()),
                    )],
                ));
            }
        }
        rows
    }

    /// Выключен ли глобальный гейт инструмента (тогда инструмент недоступен модели,
    /// даже если включён в профиле).
    pub(super) fn gate_disabled(&self, gate: ToolGate) -> bool {
        match gate {
            ToolGate::Web => !self.config.tools.web_enabled,
            ToolGate::Python => !self.config.tools.python_enabled,
            ToolGate::Fs => !self.config.tools.fs_enabled,
            ToolGate::Mcp => !self.config.mcp.enabled,
        }
    }

    /// Облачный провайдер сэмплинга подсекции (`None` — локальный движок). Для
    /// имперсонации в режиме `shared` эффективный провайдер — движок ассистента.
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

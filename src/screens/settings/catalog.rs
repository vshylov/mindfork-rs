//! Экран настроек — каталог полей: конструктор, построители полей секций и
//! подсекций (модель/семплинг/инструменты/память/интерфейс/профили), гейты
//! доступности. Часть модуля [`super`]; разбито из settings.rs.

use super::helpers::*;
use super::*;

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
            statuses: ServerStatuses {
                chat: ServerStatus::NotConfigured,
                embed: ServerStatus::NotConfigured,
                impersonation: ServerStatus::NotConfigured,
            },
        }
    }

    /// Обновляет снимок статусов серверов (чипы в секции «Модель/сервер»). Вызывается
    /// `app` при создании экрана и по событию `ServerStatus`.
    pub fn set_server_statuses(&mut self, statuses: ServerStatuses) {
        self.statuses = statuses;
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

    pub(super) fn section(&self) -> Section {
        SECTIONS[self.section_idx]
    }

    /// Каталог всех известных инструментов (для тумблеров в профиле) — включая
    /// опциональные (по умолчанию выключенные). Метаданные (группа/лейбл/гейт)
    /// снимаются с самих инструментов (единый источник — трейт `Tool`). См. spec §9.3.
    pub(super) fn tool_catalog() -> Vec<ToolInfo> {
        crate::features::tools::tool_catalog()
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

    pub(super) fn sampling_fields(&self) -> Vec<FieldRow> {
        self.sampling_fields_for(self.sampling_sub)
    }

    /// Поля секции «Семплинг» для заданной подсекции (для перечисления при поиске).
    pub(super) fn sampling_fields_for(&self, sampling_sub: Subsection) -> Vec<FieldRow> {
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

    pub(super) fn tool_fields(&self) -> Vec<FieldRow> {
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
    pub(super) fn memory_fields(&self) -> Vec<FieldRow> {
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
                    "«О себе» в note_recall",
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

    pub(super) fn interface_fields(&self) -> Vec<FieldRow> {
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
                    "Режим старого терминала",
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
        // Подписи читаются как продолжение заголовка группы («Копирование … —
        // с «мыслями»»): общий префикс «Копировать» ушёл в заголовок, чтобы
        // длинные имена не отгоняли колонку значений (см. LABEL_CAP).
        rows.extend(grouped(
            "Копирование переписки (F5)",
            vec![
                row(
                    FieldId::ICopyThoughts,
                    "С «мыслями»",
                    FieldKind::Toggle(self.config.copy.copy_thoughts),
                ),
                row(
                    FieldId::ICopyToolCalls,
                    "С параметрами инструментов",
                    FieldKind::Toggle(self.config.copy.copy_tool_calls),
                ),
                row(
                    FieldId::ICopyToolResults,
                    "С ответами инструментов",
                    FieldKind::Toggle(self.config.copy.copy_tool_results),
                ),
            ],
        ));
        rows
    }

    pub(super) fn profile_fields(&self) -> Vec<FieldRow> {
        self.profile_fields_for(self.profile_sub)
    }

    /// Поля секции «Профили» для заданной подсекции (для перечисления при поиске).
    pub(super) fn profile_fields_for(&self, profile_sub: Subsection) -> Vec<FieldRow> {
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
                // (`ToolInfo.group`), с коротким описанием и честным гейтом.
                // Индекс `PTool` — позиция в `tool_catalog()` (источник истины для
                // `toggle_profile_tool`); порядок ПОКАЗА группируем стабильной
                // сортировкой по `ToolGroup` (Ord), не трогая индексы.
                let mut indexed: Vec<(usize, ToolInfo)> =
                    Self::tool_catalog().into_iter().enumerate().collect();
                indexed.sort_by_key(|(_, info)| info.group);
                for (idx, info) in indexed {
                    let on = p.enabled_tools.iter().any(|t| t == &info.id);
                    let gate = info.gate;
                    let gated_off = on && gate.is_some_and(|g| self.gate_disabled(g));
                    let mut r = row(FieldId::PTool(idx), &info.id, FieldKind::Toggle(on));
                    r.group = info.group.title();
                    r.warn = gated_off;
                    r.hint = if gated_off {
                        gate.map(gate_hint)
                    } else {
                        (!info.label.is_empty()).then_some(info.label)
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
    pub(super) fn gate_disabled(&self, gate: ToolGate) -> bool {
        match gate {
            ToolGate::Web => !self.config.tools.web_enabled,
            ToolGate::Python => !self.config.tools.python_enabled,
            ToolGate::Fs => !self.config.tools.fs_enabled,
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

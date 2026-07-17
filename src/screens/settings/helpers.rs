//! Экран настроек — свободные функции: построители строк полей, описания,
//! раскладка/усечение, циклы enum-значений, парсеры значений. Часть модуля
//! [super]; разбито из монолита settings.rs (см. docs/history/refactoring-god-objects.md).

use super::*;
// ---------- свободные функции ----------

/// Входит ли параметр сэмплинга в подмножество, принимаемое облачным провайдером.
/// Gemini (строгий OpenAI-диалект, `restrict_to_strict`): temperature/top_p/
/// penalties/seed/max_tokens. OpenAI — то же **без `temperature`/`top_p`**: их
/// принимало лишь семейство GPT 5.4, а GPT 5.5/5.6 отвергают. Anthropic (Claude):
/// **только `max_tokens`** — новейшие
/// модели 4.x «зафиксировали» сэмплинг и отвергают `temperature`/`top_p`/`top_k` как
/// deprecated, поэтому их не шлём (см. `anthropic::wire`). Остальные — расширения
/// llama.cpp и reasoning-поля — облако не принимает. См. ADR 0004.
pub(super) fn cloud_supported_param(provider: CloudProvider, p: SamplingParam) -> bool {
    // Единый источник истины с инструментами get_sampling/set_sampling — набор
    // полей, принимаемых движком провайдера (зеркало wire-диалекта).
    crate::entities::sampling::supported_sampling_fields(Some(provider)).contains(&p.field_name())
}

// ---------- i18n-ключи описаний полей (прикрепляются к строкам при построении,
// см. `FieldRow::describe`) ----------
//
// Описания хранятся в бандлах `locales/*.json` (ось B, docs/i18n-ui.md). Здесь —
// только ключи, общие для нескольких мест построения строк (единый источник ключа).
// Специфичные для одного поля — инлайн-ключом у места постройки (catalog.rs /
// managed_rows). Ngl/Jinja различаются у ассистента и имперсонации — их ключи несёт
// `ManagedFieldIds`. Тексты резолвит `loc.t(...)` у места отрисовки.

/// Ключ: режим движка (ассистента/эмбеддингов).
pub(super) const DESC_MODE: &str = "ui.settings.desc.mode";
/// Ключ: режим движка имперсонации (добавляет `shared`).
pub(super) const DESC_IMP_MODE: &str = "ui.settings.desc.imp_mode";
/// Ключ: имя env-переменной с API-ключом (X/Ix/E).
pub(super) const DESC_API_KEY_ENV: &str = "ui.settings.desc.api_key_env";
/// Ключ: имя облачной модели (X/Ix/E).
pub(super) const DESC_MODEL_NAME: &str = "ui.settings.desc.model_name";
/// Ключ: имя env-переменной с ключом для external-сервера (опционально).
pub(super) const DESC_EXT_API_KEY_ENV: &str = "ui.settings.desc.ext_api_key_env";
/// Ключ: селектор подсекции (таб-стрип Модель/Семплинг/Профили).
pub(super) const DESC_SUBSECTION: &str = "ui.settings.desc.subsection";
/// Ключ: язык служебного каркаса профиля (ось A).
pub(super) const DESC_PROFILE_LANGUAGE: &str = "ui.settings.desc.profile_language";
/// Ключ: -ngl у движка ассистента (текст отличается от имперсонации).
pub(super) const DESC_NGL_ASSISTANT: &str = "ui.settings.desc.ngl_assistant";
/// Ключ: -ngl у движка имперсонации.
pub(super) const DESC_NGL_IMP: &str = "ui.settings.desc.ngl_imp";
/// Ключ: --jinja у движка ассистента.
pub(super) const DESC_JINJA_ASSISTANT: &str = "ui.settings.desc.jinja_assistant";
/// Ключ: --jinja у движка имперсонации.
pub(super) const DESC_JINJA_IMP: &str = "ui.settings.desc.jinja_imp";

pub(super) fn row(id: FieldId, label: &str, kind: FieldKind) -> FieldRow {
    FieldRow {
        id,
        label: label.to_string(),
        kind,
        group: "",
        hint: None,
        description: None,
        warn: false,
    }
}

/// Проставляет группу всем строкам батча — секции строятся как серии
/// `grouped("Группа", vec![...])`, а заголовок группы UI ставит на переходе.
pub(super) fn grouped(group: &'static str, mut rows: Vec<FieldRow>) -> Vec<FieldRow> {
    for r in &mut rows {
        r.group = group;
    }
    rows
}

/// Текстовая строка из `Option<String>` (пусто → «—»).
pub(super) fn text_row(id: FieldId, label: &str, value: &Option<String>) -> FieldRow {
    row(
        id,
        label,
        FieldKind::Text(value.clone().unwrap_or_else(|| "—".to_string())),
    )
}

/// Строка из обязательного числового значения (рендерится как текст).
pub(super) fn num_field<T: ToString>(id: FieldId, label: &str, value: T) -> FieldRow {
    row(id, label, FieldKind::Text(value.to_string()))
}

/// Идентификаторы полей managed-сервера для одного движка (ассистент/имперсонация).
/// Группируем в структуру, чтобы [`managed_rows`] не разрастался списком аргументов.
#[derive(Clone, Copy)]
pub(super) struct ManagedFieldIds {
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
    /// Описания `-ngl`/`--jinja` — различаются у ассистента и имперсонации, поэтому
    /// несутся здесь, а не инлайн в `managed_rows` (общей для обоих движков).
    ngl_desc: &'static str,
    jinja_desc: &'static str,
}

/// Набор FieldId для модели/сервера ассистента.
pub(super) const ASSISTANT_MANAGED_IDS: ManagedFieldIds = ManagedFieldIds {
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
    ngl_desc: DESC_NGL_ASSISTANT,
    jinja_desc: DESC_JINJA_ASSISTANT,
};

/// Набор FieldId для модели/сервера имперсонации.
pub(super) const IMP_MANAGED_IDS: ManagedFieldIds = ManagedFieldIds {
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
    ngl_desc: DESC_NGL_IMP,
    jinja_desc: DESC_JINJA_IMP,
};

/// Поля managed-сервера `llama-server` (общие для движка ассистента/имперсонации),
/// разложенные по смысловым группам: Сервер / Модель / Производительность /
/// Спекулятивное декодирование.
pub(super) fn managed_rows(
    m: &ManagedSettings,
    ids: ManagedFieldIds,
    loc: &'static Locale,
) -> Vec<FieldRow> {
    let mut rows = grouped(
        loc.t("ui.settings.group.server"),
        vec![
            text_row(ids.binary, loc.t("ui.settings.field.binary"), &m.binary),
            row(ids.host, "Host", FieldKind::Text(m.host.clone())),
            num_field(ids.port, loc.t("ui.settings.field.port"), m.port),
        ],
    );
    rows.extend(grouped(
        loc.t("ui.settings.group.model"),
        vec![
            text_row(ids.model, loc.t("ui.settings.field.gguf"), &m.model_path),
            num_field(ids.ctx, loc.t("ui.settings.field.context"), m.context_size),
            row(
                ids.jinja,
                loc.t("ui.settings.field.jinja"),
                FieldKind::Toggle(m.jinja),
            )
            .describe(loc.t(ids.jinja_desc)),
        ],
    ));
    rows.extend(grouped(
        loc.t("ui.settings.group.performance"),
        vec![
            num_field(ids.ngl, loc.t("ui.settings.field.ngl"), m.gpu_layers)
                .describe(loc.t(ids.ngl_desc)),
            row(
                ids.flash_attn,
                "FlashAttn (--flash-attn)",
                FieldKind::Choice(m.flash_attn.label().to_string()),
            )
            .describe(loc.t("ui.settings.desc.flash_attn")),
            row(
                ids.no_mmap,
                "No-mmap (--no-mmap)",
                FieldKind::Toggle(m.no_mmap),
            )
            .describe(loc.t("ui.settings.desc.no_mmap")),
        ],
    ));
    let mut spec = vec![
        row(
            ids.spec_type,
            loc.t("ui.settings.field.spec_type"),
            FieldKind::Choice(m.spec_type.label().to_string()),
        )
        .describe(loc.t("ui.settings.desc.spec_type")),
    ];
    // Поля черновой модели показываем только для типов draft-* (им нужна модель);
    // ngram-* и none их не используют — не загромождаем секцию.
    if m.spec_type.needs_draft_model() {
        spec.push(
            text_row(
                ids.draft_model,
                loc.t("ui.settings.field.draft_model"),
                &m.draft_model,
            )
            .describe(loc.t("ui.settings.desc.draft_model")),
        );
        spec.push(
            num_row(
                ids.draft_ngl,
                loc.t("ui.settings.field.draft_ngl"),
                m.draft_gpu_layers,
            )
            .describe(loc.t("ui.settings.desc.draft_ngl")),
        );
        spec.push(
            num_row(
                ids.draft_n_max,
                loc.t("ui.settings.field.draft_n_max"),
                m.draft_n_max,
            )
            .describe(loc.t("ui.settings.desc.draft_n_max")),
        );
        spec.push(
            num_row(
                ids.draft_n_min,
                loc.t("ui.settings.field.draft_n_min"),
                m.draft_n_min,
            )
            .describe(loc.t("ui.settings.desc.draft_n_min")),
        );
    }
    rows.extend(grouped(loc.t("ui.settings.group.spec"), spec));
    rows
}

/// Поля облачного провайдера (модель/API-ключ-env/base URL). `cloud` — настройки
/// активного провайдера (`None` маловероятен в облачном режиме — тогда пустые поля).
pub(super) fn cloud_rows(
    cloud: Option<&CloudSettings>,
    model_name: FieldId,
    api_key_env: FieldId,
    url: FieldId,
    loc: &'static Locale,
) -> Vec<FieldRow> {
    let none = CloudSettings::default();
    let c = cloud.unwrap_or(&none);
    vec![
        text_row(model_name, loc.t("ui.settings.field.model"), &c.model_name)
            .describe(loc.t(DESC_MODEL_NAME)),
        text_row(
            api_key_env,
            loc.t("ui.settings.field.api_key_env"),
            &c.api_key_env,
        )
        .describe(loc.t(DESC_API_KEY_ENV)),
        text_row(url, loc.t("ui.settings.field.base_url"), &c.url),
    ]
}

/// Парсит редактируемое значение опционального числа: пусто → `None` (очистить),
/// корректное → `Some`, нечисло → оставить прежнее значение (как у обязательных полей).
pub(super) fn parse_opt_num<T: std::str::FromStr + Copy>(
    text: &str,
    current: Option<T>,
) -> Option<T> {
    if text.is_empty() {
        None
    } else {
        text.parse().ok().or(current)
    }
}

/// Числовая строка из `Option<T>` (None → «—»).
pub(super) fn num_row<T: ToString>(id: FieldId, label: &str, value: Option<T>) -> FieldRow {
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
pub(super) fn sampling_row(
    id: FieldId,
    p: SamplingParam,
    s: &SamplingConfig,
    loc: &'static Locale,
) -> FieldRow {
    use SamplingParam::*;
    let label = p.label(loc);
    let mut r = match p {
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
        Thinking => row(
            id,
            label,
            FieldKind::Choice(opt_bool_label(s.thinking, loc)),
        ),
        Reasoning => row(
            id,
            label,
            FieldKind::Choice(reasoning_label(s.reasoning_effort)),
        ),
        Verbosity => row(id, label, FieldKind::Choice(verbosity_label(s.verbosity))),
    };
    // Описание параметра — единый источник `SamplingParam::description` (одинаково для
    // обеих подсекций Ассистент/Имперсонация).
    r.description = p.description(loc).map(String::from);
    r
}

/// Применяет текст редактора к числовому параметру семплинга. `Thinking`/`Reasoning`
/// — Choice-поля (редактируются ←/→), текстом не правятся.
pub(super) fn apply_sampling_text(s: &mut SamplingConfig, p: SamplingParam, trimmed: &str) {
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
        Thinking | Reasoning | Verbosity => {}
    }
}

/// Ширина подписи в терминальных колонках (кириллица/латиница = 1, CJK/эмодзи = 2).
pub(super) fn label_width(label: &str) -> usize {
    crate::shared::wrap::display_width(&label.chars().collect::<Vec<_>>())
}

/// Пол колонки значений: у секции из одних коротких подписей значения не
/// прижимаются к самому левому краю (стабильный минимум между секциями).
pub(super) const MIN_LABEL_COL: usize = 20;
/// Потолок участия подписи в выравнивании: более длинная подпись не отгоняет
/// колонку значений всей секции — её значение встаёт сразу после неё самой
/// (локальное переполнение). Все текущие подписи укладываются в потолок
/// (тест `all_labels_fit_alignment_cap`) — это страховка на будущее.
pub(super) const LABEL_CAP: usize = 28;

/// Единая колонка значений секции: самая длинная подпись среди видимых полей,
/// с полом [`MIN_LABEL_COL`] и потолком [`LABEL_CAP`]. Одна колонка на секцию
/// (а не на группу): значения всех групп стоят на общей вертикали — колонка на
/// группу давала «пилу» из разных стопов от группы к группе. Селектор подсекции
/// рисуется таб-стрипом, не строкой списка — из выравнивания исключён.
pub(super) fn section_label_col(fields: &[FieldRow]) -> usize {
    fields
        .iter()
        .filter(|f| !is_subsection(f.id))
        .map(|f| label_width(&f.label))
        .filter(|&w| w <= LABEL_CAP)
        .max()
        .unwrap_or(0)
        .max(MIN_LABEL_COL)
}

/// Инлайн-подсказка для инструмента, выключенного глобальным гейтом («выкл.
/// глобально: <выключатель>»). Показывается цветом предупреждения.
pub(super) fn gate_hint(gate: ToolGate, loc: &'static Locale) -> &'static str {
    loc.t(match gate {
        ToolGate::Web => "ui.settings.gate.web",
        ToolGate::Python => "ui.settings.gate.python",
        ToolGate::Fs => "ui.settings.gate.fs",
        ToolGate::Mcp => "ui.settings.gate.mcp",
    })
}

/// Ширина спана в колонках терминала (для правого выравнивания чипа статуса).
pub(super) fn span_width(s: &Span) -> usize {
    crate::shared::wrap::display_width(&s.content.chars().collect::<Vec<_>>())
}

/// Чип статуса сервера для секции «Модель/сервер»: глиф + метка (+ причина, если
/// сервер недоступен/не настроен — на экране настроек её видеть важно). Глифы/цвета
/// зеркалят строку статуса (`widgets::status_bar`): `●` готов, `◐` подключение,
/// `✕` нет связи/не настроен. Ширина глифа — 1 колонка (WGL4/GlyphSet, компат-безопасно).
pub(super) fn server_status_chip(
    status: &ServerStatus,
    label: &str,
    loc: &'static Locale,
    palette: &Palette,
) -> Vec<Span<'static>> {
    let glyphs = palette.glyphs();
    let (glyph, color, text) = match status {
        ServerStatus::Ready => (
            "●",
            palette.success,
            loc.tf("ui.settings.chip.ready", &[("label", label)]),
        ),
        ServerStatus::Connecting => (
            glyphs.status_connecting,
            palette.warning,
            loc.tf("ui.settings.chip.connecting", &[("label", label)]),
        ),
        ServerStatus::NotConfigured => (
            glyphs.status_off,
            palette.muted,
            loc.tf("ui.settings.chip.notconfigured", &[("label", label)]),
        ),
        ServerStatus::Disconnected(why) => (
            glyphs.status_off,
            palette.error,
            loc.tf(
                "ui.settings.chip.disconnected",
                &[("label", label), ("why", why)],
            ),
        ),
    };
    vec![
        Span::styled(glyph, Style::new().fg(color).bold()),
        Span::styled(format!(" {text} "), Style::new().fg(color)),
    ]
}

/// Локализованные подписи вкладок секции «Модель» (порядок = [`ModelTab`]).
pub(super) fn model_tab_labels(loc: &'static Locale) -> Vec<&'static str> {
    MODEL_TAB_KEYS.iter().map(|&k| loc.t(k)).collect()
}

/// Локализованные подписи вкладок подсекции «Ассистент»/«Имперсонация».
pub(super) fn sub_tab_labels(loc: &'static Locale) -> Vec<&'static str> {
    SUB_TAB_KEYS.iter().map(|&k| loc.t(k)).collect()
}

/// Является ли поле селектором подсекции (рисуется таб-стрипом, а не строкой списка).
pub(super) fn is_subsection(id: FieldId) -> bool {
    matches!(
        id,
        FieldId::ModelSub | FieldId::SamplingSub | FieldId::ProfileSub
    )
}

/// Отображаемое значение поля (для крошки поиска).
pub(super) fn value_text(kind: &FieldKind, loc: &'static Locale) -> String {
    match kind {
        FieldKind::Toggle(on) => loc
            .t(if *on {
                "ui.settings.bool.on"
            } else {
                "ui.settings.bool.off"
            })
            .to_string(),
        FieldKind::Choice(v) => v.clone(),
        FieldKind::Text(v) => v.clone(),
    }
}

/// Добавляет поля секции/подсекции в индекс поиска (пропуская селектор подсекции).
/// `sub_label` — подпись подсекции (в крошку и ловушку), чтобы одинаковые поля
/// разных вкладок различались.
pub(super) fn collect_hits(
    out: &mut Vec<SearchHit>,
    section_idx: usize,
    section: Section,
    subsection: Option<usize>,
    sub_label: Option<&str>,
    fields: Vec<FieldRow>,
    loc: &'static Locale,
) {
    let head = match sub_label {
        Some(sub) => format!("{} · {}", section.title(loc), sub),
        None => section.title(loc).to_string(),
    };
    for (fi, f) in fields.iter().enumerate() {
        if is_subsection(f.id) {
            continue;
        }
        let desc = f.description.as_deref().unwrap_or("");
        let crumb = if f.group.is_empty() {
            format!("{head} › {}", f.label)
        } else {
            format!("{head} › {} › {}", f.group, f.label)
        };
        let value = value_text(&f.kind, loc);
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
pub(super) fn tab_strip_line(
    tabs: &[&str],
    active: usize,
    focused: bool,
    palette: &Palette,
) -> Line<'static> {
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
pub(super) fn header_line(
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
        used += label_width(&tag);
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
pub(super) fn render_field_line(
    f: &FieldRow,
    label_col: usize,
    value_w: usize,
    modified: bool,
    selected: bool,
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
    // Левая колонка (2 клетки), фиксом — поля выглядят отступленными под
    // заголовком группы. У выбранного поля — зелёный рейл `▌` (как активная секция
    // в меню слева); иначе маркер «изменено против дефолта» `•`, иначе пусто. Рейл
    // на выбранной строке важнее маркера (поле и так в фокусе; отойдёшь — `•`
    // вернётся), поэтому имеет приоритет.
    let marker = if selected {
        Span::styled("▌ ", Style::new().fg(palette.success))
    } else if modified {
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
pub(super) fn truncate_to_width(s: &str, max: usize) -> (String, usize) {
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

pub(super) fn mode_label(m: ServerMode) -> String {
    match m {
        ServerMode::Managed => "managed".into(),
        ServerMode::External => "external".into(),
        ServerMode::OpenAi => "openai".into(),
        ServerMode::Gemini => "gemini".into(),
        ServerMode::Claude => "claude".into(),
    }
}

/// Циклически меняет режим движка (5 значений, с учётом направления ←/→).
pub(super) fn cycle_mode(m: ServerMode, dir: i32) -> ServerMode {
    use ServerMode::*;
    let order = [Managed, External, OpenAi, Gemini, Claude];
    let idx = order.iter().position(|x| *x == m).unwrap_or(0) as i32;
    let n = order.len() as i32;
    order[(((idx + dir) % n + n) % n) as usize]
}

pub(super) fn imp_mode_label(m: ImpersonationMode) -> String {
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
pub(super) fn cycle_imp_mode(m: ImpersonationMode, dir: i32) -> ImpersonationMode {
    use ImpersonationMode::*;
    let order = [Shared, Managed, External, OpenAi, Gemini, Claude];
    let idx = order.iter().position(|x| *x == m).unwrap_or(0) as i32;
    let n = order.len() as i32;
    order[(((idx + dir) % n + n) % n) as usize]
}

pub(super) fn theme_label(t: Theme, loc: &'static Locale) -> String {
    loc.t(match t {
        Theme::Auto => "ui.settings.choice.theme_auto",
        Theme::Dark => "ui.settings.choice.theme_dark",
        Theme::Light => "ui.settings.choice.theme_light",
    })
    .to_string()
}

pub(super) fn cycle_theme(t: Theme) -> Theme {
    match t {
        Theme::Auto => Theme::Dark,
        Theme::Dark => Theme::Light,
        Theme::Light => Theme::Auto,
    }
}

/// Циклический сдвиг языка интерфейса по всем известным языкам (вшитые + внешние,
/// `Lang::all()`) с учётом направления `dir`.
pub(super) fn cycle_lang(l: crate::shared::i18n::Lang, dir: i32) -> crate::shared::i18n::Lang {
    let all = crate::shared::i18n::Lang::all();
    let idx = all.iter().position(|&x| x == l).unwrap_or(0) as i32;
    let n = all.len() as i32;
    all[(((idx + dir) % n + n) % n) as usize]
}

pub(super) fn opt_bool_label(b: Option<bool>, loc: &'static Locale) -> String {
    match b {
        None => "—".into(),
        Some(true) => loc.t("ui.settings.bool.on").to_string(),
        Some(false) => loc.t("ui.settings.bool.off").to_string(),
    }
}

pub(super) fn cycle_opt_bool(b: Option<bool>) -> Option<bool> {
    match b {
        None => Some(true),
        Some(true) => Some(false),
        Some(false) => None,
    }
}

pub(super) fn reasoning_label(r: Option<ReasoningEffort>) -> String {
    match r {
        None => "—".into(),
        Some(e) => e.as_wire().to_string(),
    }
}

/// Порядок вариантов `reasoning_effort` в цикле/меню (совпадает с [`REASONING_ORDER`]).
pub(super) const REASONING_ORDER: [Option<ReasoningEffort>; 7] = [
    None,
    Some(ReasoningEffort::None),
    Some(ReasoningEffort::Minimal),
    Some(ReasoningEffort::Low),
    Some(ReasoningEffort::Medium),
    Some(ReasoningEffort::High),
    Some(ReasoningEffort::XHigh),
];

pub(super) fn cycle_reasoning(r: Option<ReasoningEffort>) -> Option<ReasoningEffort> {
    let i = REASONING_ORDER.iter().position(|&x| x == r).unwrap_or(0);
    REASONING_ORDER[(i + 1) % REASONING_ORDER.len()]
}

pub(super) fn verbosity_label(v: Option<Verbosity>) -> String {
    match v {
        None => "—".into(),
        Some(x) => x.as_wire().to_string(),
    }
}

/// Порядок вариантов `verbosity` в цикле/меню.
pub(super) const VERBOSITY_ORDER: [Option<Verbosity>; 4] = [
    None,
    Some(Verbosity::Low),
    Some(Verbosity::Medium),
    Some(Verbosity::High),
];

pub(super) fn cycle_verbosity(v: Option<Verbosity>) -> Option<Verbosity> {
    let i = VERBOSITY_ORDER.iter().position(|&x| x == v).unwrap_or(0);
    VERBOSITY_ORDER[(i + 1) % VERBOSITY_ORDER.len()]
}

/// Числовой вид поля для валидации (`None` — не числовое: текст/URL/списки/выбор).
/// Config-поля — из таблицы доступа (`field_spec.num`); семплинг — по своему виду.
pub(super) fn field_num_kind(id: FieldId) -> Option<NumKind> {
    match id {
        FieldId::S(p) | FieldId::IS(p) => p.num_kind(),
        _ => super::spec::field_spec(id).and_then(|s| s.num),
    }
}

/// i18n-ключ ошибки валидации поля (`None` — валидно). Пустой ввод допустим
/// (очистка/сохранение прежнего); непустой в числовом поле обязан парситься. Проверка
/// «мягкая» (i64/f64), точный тип и диапазон досматривает `apply_text`. Возвращает
/// **ключ** — текст резолвит вызывающий (`loc.t`) в локали интерфейса.
pub(super) fn field_validation_error(id: FieldId, text: &str) -> Option<&'static str> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    match field_num_kind(id) {
        Some(NumKind::Int) if t.parse::<i64>().is_err() => Some("ui.settings.err.int"),
        Some(NumKind::Float) if t.parse::<f64>().is_err() => Some("ui.settings.err.float"),
        _ => None,
    }
}

/// Порядок вариантов режима движка (для Choice-попапа; совпадает с `cycle_mode`).
pub(super) const SERVER_MODES: [ServerMode; 5] = [
    ServerMode::Managed,
    ServerMode::External,
    ServerMode::OpenAi,
    ServerMode::Gemini,
    ServerMode::Claude,
];

/// Порядок вариантов режима имперсонации (совпадает с `cycle_imp_mode`).
pub(super) const IMP_MODES: [ImpersonationMode; 6] = [
    ImpersonationMode::Shared,
    ImpersonationMode::Managed,
    ImpersonationMode::External,
    ImpersonationMode::OpenAi,
    ImpersonationMode::Gemini,
    ImpersonationMode::Claude,
];

/// Порядок тем (совпадает с `cycle_theme`).
pub(super) const THEMES: [Theme; 3] = [Theme::Auto, Theme::Dark, Theme::Light];

/// Строит (подписи, индекс текущего) из массива вариантов и функции-подписи.
pub(super) fn index_menu<T: Copy + PartialEq>(
    all: &[T],
    cur: T,
    label: impl Fn(T) -> String,
) -> (Vec<String>, usize) {
    let opts = all.iter().map(|&x| label(x)).collect();
    let idx = all.iter().position(|&x| x == cur).unwrap_or(0);
    (opts, idx)
}

pub(super) fn flash_menu(cur: FlashAttn) -> (Vec<String>, usize) {
    index_menu(&FlashAttn::ALL, cur, |x| x.label().to_string())
}

pub(super) fn spec_menu(cur: SpecType) -> (Vec<String>, usize) {
    index_menu(&SpecType::ALL, cur, |x| x.label().to_string())
}

/// Меню выбора для Choice-параметров семплинга (`Thinking`/`Reasoning`); порядок
/// подписей совпадает с циклом `cycle_opt_bool`/`cycle_reasoning`.
pub(super) fn sampling_choice_menu(
    s: &SamplingConfig,
    p: SamplingParam,
    loc: &'static Locale,
) -> (Vec<String>, usize) {
    match p {
        SamplingParam::Thinking => {
            let opts = [None, Some(true), Some(false)]
                .iter()
                .map(|&b| opt_bool_label(b, loc))
                .collect();
            let idx = match s.thinking {
                None => 0,
                Some(true) => 1,
                Some(false) => 2,
            };
            (opts, idx)
        }
        SamplingParam::Reasoning => {
            let opts = REASONING_ORDER
                .iter()
                .map(|&r| reasoning_label(r))
                .collect();
            let idx = REASONING_ORDER
                .iter()
                .position(|&r| r == s.reasoning_effort)
                .unwrap_or(0);
            (opts, idx)
        }
        SamplingParam::Verbosity => {
            let opts = VERBOSITY_ORDER
                .iter()
                .map(|&v| verbosity_label(v))
                .collect();
            let idx = VERBOSITY_ORDER
                .iter()
                .position(|&v| v == s.verbosity)
                .unwrap_or(0);
            (opts, idx)
        }
        _ => (Vec::new(), 0),
    }
}

/// Поле принадлежит профилю (у него нет config-дефолта → не участвует в `•`/сбросе).
pub(super) fn is_profile_field(id: FieldId) -> bool {
    matches!(
        id,
        FieldId::PSelect
            | FieldId::PName
            | FieldId::PLanguage
            | FieldId::PSystem
            | FieldId::PGreeting
            | FieldId::PImpSystem
            | FieldId::PTool(_)
            | FieldId::ProfileSub
    )
}

pub(super) fn parse_opt<T: std::str::FromStr>(s: &str) -> Option<T> {
    if s.is_empty() { None } else { s.parse().ok() }
}

pub(super) fn parse_opt_f32(s: &str) -> Option<f32> {
    parse_opt(s)
}

/// Разбор текстового поля-списка строк (UI): разбивает по `sep`, обрезает
/// пробелы у элементов, отбрасывает пустые; пустой ввод → `None`.
pub(super) fn parse_list(s: &str, sep: char) -> Option<Vec<String>> {
    let v: Vec<String> = s
        .split(sep)
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
        .collect();
    (!v.is_empty()).then_some(v)
}

/// Склейка списка строк для отображения через `sep`; `None`/пусто → «—».
pub(super) fn join_list(v: Option<&[String]>, sep: char) -> String {
    match v {
        Some(items) if !items.is_empty() => items.join(&sep.to_string()),
        _ => "—".to_string(),
    }
}

/// DRY-брейкеры: как [`parse_list`] по запятой, но с декодированием эскейпов
/// `\n`/`\t`/`\r` (однострочный редактор не даёт ввести их буквально).
pub(super) fn parse_breakers(s: &str) -> Option<Vec<String>> {
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
pub(super) fn join_breakers(v: Option<&[String]>) -> String {
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
pub(super) fn decode_escapes(s: &str) -> String {
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
pub(super) fn encode_escapes(s: &str) -> String {
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
pub(super) fn multiline_popup_height(area: Rect) -> u16 {
    (area.height.saturating_mul(60) / 100)
        .max(8)
        .min(area.height)
}

/// Прямоугольник по центру `area`: `pct_x`% ширины (≥`min_w`), фикс. высота.
pub(super) fn centered_rect(pct_x: u16, min_w: u16, height: u16, area: Rect) -> Rect {
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
pub(super) fn centered_rect_wh(width: u16, height: u16, area: Rect) -> Rect {
    let [h] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(area);
    let [v] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(h);
    v
}

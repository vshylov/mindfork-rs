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

// ---------- описания полей (прикрепляются к строкам при построении, см.
// `FieldRow::describe`) ----------
//
// Описания, общие для нескольких мест построения строк, вынесены в `const` (единый
// источник текста). Специфичные для одного поля — инлайн-литералом у места постройки
// (catalog.rs / managed_rows). Ngl/Jinja различаются у ассистента и имперсонации —
// их тексты несёт `ManagedFieldIds`. Параметры семплинга — `SamplingParam::description`.

/// Режим движка (ассистента/эмбеддингов).
pub(super) const DESC_MODE: &str = "managed — локальный llama-server (приложение запускает процесс); \
     external — свой OpenAI-совместимый сервер по URL; openai/gemini — облако \
     (нужны имя модели и API-ключ из env-переменной).";
/// Режим движка имперсонации (добавляет `shared`).
pub(super) const DESC_IMP_MODE: &str = "shared — тот же движок, что у ассистента (с семплингом имперсонации); \
     managed — отдельный llama-server; external — отдельный удалённый сервер; \
     openai/gemini — облако (имя модели + API-ключ из env).";
/// Имя env-переменной с API-ключом (X/Ix/E).
pub(super) const DESC_API_KEY_ENV: &str = "Имя переменной окружения с API-ключом (например OPENAI_API_KEY). Хранится \
     только имя — сам ключ читается из окружения и на диск не пишется.";
/// Имя облачной модели (X/Ix/E).
pub(super) const DESC_MODEL_NAME: &str = "Имя модели у провайдера (например gpt-4o, gemini-2.5-pro, \
     text-embedding-3-small). Для облака обязательно.";
/// Селектор подсекции (таб-стрип Модель/Семплинг/Профили).
pub(super) const DESC_SUBSECTION: &str = "Переключение между настройками ассистента и имперсонации (написание \
     сообщения от лица пользователя, Ctrl+U). ←/→ или Enter.";

/// -ngl у движка ассистента (текст отличается от имперсонации).
pub(super) const DESC_NGL_ASSISTANT: &str = "Сколько слоёв модели выгрузить на видеокарту (GPU). Больше слоёв — \
     быстрее, но нужна видеопамять; 0 — считать только на процессоре, \
     99 — вся модель на GPU.";
/// -ngl у движка имперсонации.
pub(super) const DESC_NGL_IMP: &str = "Сколько слоёв модели имперсонации выгрузить на видеокарту (GPU). \
     0 — только процессор, 99 — вся модель на GPU.";
/// --jinja у движка ассистента.
pub(super) const DESC_JINJA_ASSISTANT: &str = "Использовать встроенный chat-шаблон модели (Jinja). Нужен для \
     правильного формата сообщений и вызова инструментов — обычно держат включённым.";
/// --jinja у движка имперсонации.
pub(super) const DESC_JINJA_IMP: &str = "Использовать встроенный chat-шаблон модели (Jinja) для сервера \
     имперсонации.";

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
pub(super) fn managed_rows(m: &ManagedSettings, ids: ManagedFieldIds) -> Vec<FieldRow> {
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
            row(ids.jinja, "Шаблон (--jinja)", FieldKind::Toggle(m.jinja)).describe(ids.jinja_desc),
        ],
    ));
    rows.extend(grouped(
        "Производительность",
        vec![
            num_field(ids.ngl, "GPU-слои (-ngl)", m.gpu_layers).describe(ids.ngl_desc),
            row(
                ids.flash_attn,
                "FlashAttn (--flash-attn)",
                FieldKind::Choice(m.flash_attn.label().to_string()),
            )
            .describe(
                "FlashAttention — оптимизация механизма внимания: ускоряет генерацию и \
                 экономит видеопамять на поддерживаемых GPU. auto — пусть llama.cpp решит \
                 сам; on/off — включить/выключить принудительно.",
            ),
            row(
                ids.no_mmap,
                "No-mmap (--no-mmap)",
                FieldKind::Toggle(m.no_mmap),
            )
            .describe(
                "Грузить веса модели целиком в оперативную память вместо отображения \
                 файла с диска (mmap). Помогает на сетевых и медленных дисках, но требует \
                 больше свободной RAM.",
            ),
        ],
    ));
    let mut spec = vec![
        row(
            ids.spec_type,
            "Спек. декод. (--spec-type)",
            FieldKind::Choice(m.spec_type.label().to_string()),
        )
        .describe(
            "Спекулятивное декодирование ускоряет генерацию: «черновик» предлагает \
             несколько токенов вперёд, основная модель их разом проверяет. draft-* — \
             нужна отдельная черновая модель (-md); для MTP-моделей (mtp-gemma-…) — \
             draft-mtp; ngram-* — без модели (черновик из контекста). none — выключено.",
        ),
    ];
    // Поля черновой модели показываем только для типов draft-* (им нужна модель);
    // ngram-* и none их не используют — не загромождаем секцию.
    if m.spec_type.needs_draft_model() {
        spec.push(
            text_row(ids.draft_model, "Черновая модель (-md)", &m.draft_model).describe(
                "Путь к «черновой» GGUF-модели для спекулятивного декодирования (-md). \
                 Должна быть совместима с основной по словарю. Для MTP — путь к \
                 соответствующему MTP-GGUF.",
            ),
        );
        spec.push(
            num_row(
                ids.draft_ngl,
                "Черновик GPU-слои (-ngld)",
                m.draft_gpu_layers,
            )
            .describe("Сколько слоёв черновой модели выгрузить на GPU (-ngld). Пусто — авто."),
        );
        spec.push(
            num_row(ids.draft_n_max, "Черновик n-max", m.draft_n_max).describe(
                "Сколько токенов черновая модель предлагает за один шаг \
             (--spec-draft-n-max). Пусто — значение llama.cpp по умолчанию (3).",
            ),
        );
        spec.push(
            num_row(ids.draft_n_min, "Черновик n-min", m.draft_n_min).describe(
                "Минимум черновых токенов за шаг (--spec-draft-n-min). Пусто — по \
             умолчанию (0).",
            ),
        );
    }
    rows.extend(grouped("Спекулятивное декодирование", spec));
    rows
}

/// Поля облачного провайдера (модель/API-ключ-env/base URL). `cloud` — настройки
/// активного провайдера (`None` маловероятен в облачном режиме — тогда пустые поля).
pub(super) fn cloud_rows(
    cloud: Option<&CloudSettings>,
    model_name: FieldId,
    api_key_env: FieldId,
    url: FieldId,
) -> Vec<FieldRow> {
    let none = CloudSettings::default();
    let c = cloud.unwrap_or(&none);
    vec![
        text_row(model_name, "Модель", &c.model_name).describe(DESC_MODEL_NAME),
        text_row(api_key_env, "API-ключ (env)", &c.api_key_env).describe(DESC_API_KEY_ENV),
        text_row(url, "Base URL (опц.)", &c.url),
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
pub(super) fn sampling_row(id: FieldId, p: SamplingParam, s: &SamplingConfig) -> FieldRow {
    use SamplingParam::*;
    let label = p.label();
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
        Thinking => row(id, label, FieldKind::Choice(opt_bool_label(s.thinking))),
        Reasoning => row(
            id,
            label,
            FieldKind::Choice(reasoning_label(s.reasoning_effort)),
        ),
    };
    // Описание параметра — единый источник `SamplingParam::description` (одинаково для
    // обеих подсекций Ассистент/Имперсонация).
    r.description = p.description();
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
        Thinking | Reasoning => {}
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
pub(super) fn gate_hint(gate: ToolGate) -> &'static str {
    match gate {
        ToolGate::Web => "выкл. глобально: Web-поиск",
        ToolGate::Python => "выкл. глобально: Python",
        ToolGate::Fs => "выкл. глобально: файлы",
    }
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
    palette: &Palette,
) -> Vec<Span<'static>> {
    let glyphs = palette.glyphs();
    let (glyph, color, text) = match status {
        ServerStatus::Ready => ("●", palette.success, format!("{label}: готов")),
        ServerStatus::Connecting => (
            glyphs.status_connecting,
            palette.warning,
            format!("{label}: подключение…"),
        ),
        ServerStatus::NotConfigured => (
            glyphs.status_off,
            palette.muted,
            format!("{label}: не настроен"),
        ),
        ServerStatus::Disconnected(why) => (
            glyphs.status_off,
            palette.error,
            format!("{label}: нет связи: {why}"),
        ),
    };
    vec![
        Span::styled(glyph, Style::new().fg(color).bold()),
        Span::styled(format!(" {text} "), Style::new().fg(color)),
    ]
}

/// Является ли поле селектором подсекции (рисуется таб-стрипом, а не строкой списка).
pub(super) fn is_subsection(id: FieldId) -> bool {
    matches!(
        id,
        FieldId::ModelSub | FieldId::SamplingSub | FieldId::ProfileSub
    )
}

/// Отображаемое значение поля (для крошки поиска).
pub(super) fn value_text(kind: &FieldKind) -> String {
    match kind {
        FieldKind::Toggle(on) => (if *on { "вкл" } else { "выкл" }).to_string(),
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
) {
    let head = match sub_label {
        Some(sub) => format!("{} · {}", section.title(), sub),
        None => section.title().to_string(),
    };
    for (fi, f) in fields.iter().enumerate() {
        if is_subsection(f.id) {
            continue;
        }
        let desc = f.description.unwrap_or("");
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

pub(super) fn theme_label(t: Theme) -> String {
    match t {
        Theme::Auto => "авто".into(),
        Theme::Dark => "тёмная".into(),
        Theme::Light => "светлая".into(),
    }
}

pub(super) fn cycle_theme(t: Theme) -> Theme {
    match t {
        Theme::Auto => Theme::Dark,
        Theme::Dark => Theme::Light,
        Theme::Light => Theme::Auto,
    }
}

pub(super) fn opt_bool_label(b: Option<bool>) -> String {
    match b {
        None => "—".into(),
        Some(true) => "вкл".into(),
        Some(false) => "выкл".into(),
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

pub(super) fn cycle_reasoning(r: Option<ReasoningEffort>) -> Option<ReasoningEffort> {
    match r {
        None => Some(ReasoningEffort::None),
        Some(ReasoningEffort::None) => Some(ReasoningEffort::Low),
        Some(ReasoningEffort::Low) => Some(ReasoningEffort::Medium),
        Some(ReasoningEffort::Medium) => Some(ReasoningEffort::High),
        Some(ReasoningEffort::High) => None,
    }
}

/// Числовой вид поля для валидации (`None` — не числовое: текст/URL/списки/выбор).
/// Config-поля — из таблицы доступа (`field_spec.num`); семплинг — по своему виду.
pub(super) fn field_num_kind(id: FieldId) -> Option<NumKind> {
    match id {
        FieldId::S(p) | FieldId::IS(p) => p.num_kind(),
        _ => super::spec::field_spec(id).and_then(|s| s.num),
    }
}

/// Ошибка валидации поля (`None` — валидно). Пустой ввод допустим (очистка/сохранение
/// прежнего); непустой в числовом поле обязан парситься. Проверка «мягкая» (i64/f64),
/// точный тип и диапазон досматривает `apply_text`.
pub(super) fn field_validation_error(id: FieldId, text: &str) -> Option<&'static str> {
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
pub(super) fn sampling_choice_menu(s: &SamplingConfig, p: SamplingParam) -> (Vec<String>, usize) {
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
pub(super) fn is_profile_field(id: FieldId) -> bool {
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

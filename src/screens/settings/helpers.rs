//! Settings screen — free functions: field-row builders, descriptions,
//! layout/truncation, enum-value cycles, value parsers. Part of the
//! [super] module; split out of the settings.rs monolith (see docs/history/refactoring-god-objects.md).

use super::*;
// ---------- free functions ----------

/// Whether a sampling parameter is in the subset accepted by a cloud provider.
/// Gemini (strict OpenAI dialect, `restrict_to_strict`): temperature/top_p/
/// penalties/seed/max_tokens. OpenAI — the same **without `temperature`/`top_p`**: only
/// the GPT 5.4 family accepted them, and GPT 5.5/5.6 reject them. Anthropic (Claude):
/// **only `max_tokens`** — the newest
/// 4.x models "locked in" sampling and reject `temperature`/`top_p`/`top_k` as
/// deprecated, so we don't send them (see `anthropic::wire`). The rest — llama.cpp
/// extensions and reasoning fields — the cloud doesn't accept. See ADR 0004.
pub(super) fn cloud_supported_param(provider: CloudProvider, p: SamplingParam) -> bool {
    // A single source of truth shared with the get_sampling/set_sampling tools — the set
    // of fields the provider's engine accepts (mirrors the wire dialect).
    crate::entities::sampling::supported_sampling_fields(Some(provider)).contains(&p.field_name())
}

// ---------- i18n keys for field descriptions (attached to rows at build time,
// see `FieldRow::describe`) ----------
//
// Descriptions live in the `locales/*.json` bundles (axis B, docs/i18n-ui.md). Here —
// only keys shared by several row-building sites (a single source for the key).
// Ones specific to a single field — an inline key at the build site (catalog.rs /
// managed_rows). Ngl/Jinja differ between the assistant and impersonation — their keys
// are carried by `ManagedFieldIds`. Texts are resolved by `loc.t(...)` at the render site.

/// Key: engine mode (assistant/embeddings).
pub(super) const DESC_MODE: &str = "ui.settings.desc.mode";
/// Key: impersonation engine mode (adds `shared`).
pub(super) const DESC_IMP_MODE: &str = "ui.settings.desc.imp_mode";
/// Key: the env-variable name holding the API key (X/Ix/E).
pub(super) const DESC_API_KEY_ENV: &str = "ui.settings.desc.api_key_env";
/// Description of the "API key" field (the secret itself, stored encrypted for this machine).
pub(super) const DESC_API_KEY: &str = "ui.settings.desc.api_key";
/// Key: cloud model name (X/Ix/E).
pub(super) const DESC_MODEL_NAME: &str = "ui.settings.desc.model_name";
/// Key: the env-variable name holding the external-server key (optional).
pub(super) const DESC_EXT_API_KEY_ENV: &str = "ui.settings.desc.ext_api_key_env";
/// Key: the subsection selector (Model/Sampling/Profiles tab strip).
pub(super) const DESC_SUBSECTION: &str = "ui.settings.desc.subsection";
/// Key: the profile's scaffold language (axis A).
pub(super) const DESC_PROFILE_LANGUAGE: &str = "ui.settings.desc.profile_language";
/// Key: `-ngl` for the assistant engine (text differs from impersonation).
pub(super) const DESC_NGL_ASSISTANT: &str = "ui.settings.desc.ngl_assistant";
/// Key: `-ngl` for the impersonation engine.
pub(super) const DESC_NGL_IMP: &str = "ui.settings.desc.ngl_imp";
/// Key: `--jinja` for the assistant engine.
pub(super) const DESC_JINJA_ASSISTANT: &str = "ui.settings.desc.jinja_assistant";
/// Key: `--jinja` for the impersonation engine.
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

/// Stamps a group onto every row of the batch — sections are built as a series of
/// `grouped("Group", vec![...])`, and the UI puts the group header at the transition.
pub(super) fn grouped(group: &'static str, mut rows: Vec<FieldRow>) -> Vec<FieldRow> {
    for r in &mut rows {
        r.group = group;
    }
    rows
}

/// A text row from `Option<String>` (empty → "—").
pub(super) fn text_row(id: FieldId, label: &str, value: &Option<String>) -> FieldRow {
    row(
        id,
        label,
        FieldKind::Text(value.clone().unwrap_or_else(|| "—".to_string())),
    )
}

/// A row from a required numeric value (rendered as text).
pub(super) fn num_field<T: ToString>(id: FieldId, label: &str, value: T) -> FieldRow {
    row(id, label, FieldKind::Text(value.to_string()))
}

/// Field identifiers of the managed server for one engine (assistant/impersonation).
/// Grouped into a struct so [`managed_rows`] doesn't grow into a list of arguments.
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
    /// Descriptions of `-ngl`/`--jinja` — differ between the assistant and
    /// impersonation, so they're carried here rather than inline in `managed_rows`
    /// (shared by both engines).
    ngl_desc: &'static str,
    jinja_desc: &'static str,
}

/// The set of FieldIds for the assistant's model/server.
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

/// The set of FieldIds for impersonation's model/server.
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

/// Fields of the managed `llama-server` (shared by the assistant/impersonation
/// engines), laid out into semantic groups: Server / Model / Performance /
/// Speculative decoding.
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
    // We show draft-model fields only for draft-* types (they need a model);
    // ngram-* and none don't use them — don't clutter the section.
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

/// Cloud-provider fields (model/API key/API-key env/base URL). `cloud` —
/// the active provider's settings (`None` is unlikely in cloud mode — then empty
/// fields). `key_present` — whether the provider's key is stored on this machine
/// (the "API key" field's value is a status, not a secret).
pub(super) fn cloud_rows(
    cloud: Option<&CloudSettings>,
    model_name: FieldId,
    api_key: FieldId,
    api_key_env: FieldId,
    url: FieldId,
    key_present: bool,
    loc: &'static Locale,
) -> Vec<FieldRow> {
    let none = CloudSettings::default();
    let c = cloud.unwrap_or(&none);
    vec![
        text_row(model_name, loc.t("ui.settings.field.model"), &c.model_name)
            .describe(loc.t(DESC_MODEL_NAME)),
        api_key_row(api_key, key_present, loc),
        text_row(
            api_key_env,
            loc.t("ui.settings.field.api_key_env"),
            &c.api_key_env,
        )
        .describe(loc.t(DESC_API_KEY_ENV)),
        text_row(url, loc.t("ui.settings.field.base_url"), &c.url),
    ]
}

/// An input field for the API key itself (not an env-variable name)? Such fields
/// have special behavior: an empty seed, a masked editor, a commit as a separate
/// intent, and `Del` as deleting the key. See [`SettingsScreen::api_key_field_provider`].
pub(super) fn is_api_key_field(id: FieldId) -> bool {
    matches!(
        id,
        FieldId::XApiKey | FieldId::IxApiKey | FieldId::EApiKey | FieldId::TtsApiKey
    )
}

/// The "API key" field row: the value is a **status**, not a secret ("configured (this
/// computer)" / "not set"; if the machine doesn't support encryption — "unavailable",
/// the env path remains). Editing opens an empty masked editor: a stored
/// key can't be shown, entering it = replacing it. See docs/research/api-key-storage.md.
pub(super) fn api_key_row(id: FieldId, present: bool, loc: &'static Locale) -> FieldRow {
    let (value, desc) = if !crate::shared::secrets::scheme_available() {
        (
            loc.t("ui.settings.value.key_unsupported"),
            loc.t("ui.settings.desc.api_key_unsupported"),
        )
    } else if present {
        (loc.t("ui.settings.value.key_set"), loc.t(DESC_API_KEY))
    } else {
        (loc.t("ui.settings.value.key_unset"), loc.t(DESC_API_KEY))
    };
    row(
        id,
        loc.t("ui.settings.field.api_key"),
        FieldKind::Text(value.to_string()),
    )
    .describe(desc)
}

/// Parses an editable optional-number value: empty → `None` (clear),
/// valid → `Some`, non-numeric → keep the previous value (as with required fields).
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

/// A numeric row from `Option<T>` (None → "—").
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

/// A sampling-field row by parameter: numeric ones — text (`num_row`),
/// `Thinking`/`Reasoning` — a cyclic choice.
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
    // The parameter's description — a single source `SamplingParam::description` (the
    // same for both the Assistant/Impersonation subsections).
    r.description = p.description(loc).map(String::from);
    r
}

/// Applies editor text to a numeric sampling parameter. `Thinking`/`Reasoning`
/// — Choice fields (edited via ←/→), not editable as text.
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

/// Label width in terminal columns (Cyrillic/Latin = 1, CJK/emoji = 2).
pub(super) fn label_width(label: &str) -> usize {
    crate::shared::wrap::display_width(&label.chars().collect::<Vec<_>>())
}

/// The value column's floor: a section made of only short labels doesn't push
/// values all the way to the left edge (a stable minimum across sections).
pub(super) const MIN_LABEL_COL: usize = 20;
/// A cap on a label's involvement in alignment: a longer label doesn't push away
/// the whole section's value column — its value sits locally right after it
/// (local overflow). All current labels fit within the cap
/// (test `all_labels_fit_alignment_cap`) — a safety net for the future.
pub(super) const LABEL_CAP: usize = 28;

/// The section's shared value column: the longest label among visible fields,
/// with a floor [`MIN_LABEL_COL`] and a cap [`LABEL_CAP`]. One column per section
/// (not per group): the values of all groups line up on a shared vertical — a per-group
/// column "sawtoothed" with different stops from group to group. The subsection
/// selector is drawn as a tab strip, not a list row — excluded from alignment.
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

/// An inline hint for a tool disabled by a global gate ("disabled globally:
/// <switch>"). Shown in warning color.
pub(super) fn gate_hint(gate: ToolGate, loc: &'static Locale) -> &'static str {
    loc.t(match gate {
        ToolGate::Web => "ui.settings.gate.web",
        ToolGate::Python => "ui.settings.gate.python",
        ToolGate::Fs => "ui.settings.gate.fs",
        ToolGate::Mcp => "ui.settings.gate.mcp",
    })
}

/// A span's width in terminal columns (for right-aligning the status chip).
pub(super) fn span_width(s: &Span) -> usize {
    crate::shared::wrap::display_width(&s.content.chars().collect::<Vec<_>>())
}

/// The server-status chip for the "Model/server" section: a glyph + a label (+ a
/// reason, if the server is unavailable/not configured — seeing it matters on the
/// settings screen). Glyphs/colors mirror the status bar's row (`widgets::status_bar`):
/// `●` ready, `◐` connecting, `✕` no connection/not configured. The glyph is 1 column
/// wide (WGL4/GlyphSet, compat-safe).
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

/// Localized tab labels of the "Model" section (order = [`ModelTab`]).
pub(super) fn model_tab_labels(loc: &'static Locale) -> Vec<&'static str> {
    MODEL_TAB_KEYS.iter().map(|&k| loc.t(k)).collect()
}

/// Localized tab labels of the "Assistant"/"Impersonation" subsection.
pub(super) fn sub_tab_labels(loc: &'static Locale) -> Vec<&'static str> {
    SUB_TAB_KEYS.iter().map(|&k| loc.t(k)).collect()
}

/// Whether the field is a subsection selector (drawn as a tab strip, not a list row).
pub(super) fn is_subsection(id: FieldId) -> bool {
    matches!(
        id,
        FieldId::ModelSub | FieldId::SamplingSub | FieldId::ProfileSub
    )
}

/// The field's displayed value (for the search breadcrumb).
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

/// Adds a section/subsection's fields to the search index (skipping the subsection
/// selector). `sub_label` — the subsection's label (into the breadcrumb and the trap),
/// so identical fields of different tabs are distinguishable.
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

/// The subsection tab strip: `Assistant │ Impersonation │ Embeddings`. The active tab
/// is highlighted (a backdrop when the strip is focused, otherwise an accent color); on
/// the right, when focused — the `←→` hint. The `│` separator and all content are WGL4-safe.
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

/// A field-group header: `Group ──── N/M ──` spanning the full width. The name — muted
/// bold, the continuation — a line in the border color; `count = (on, total)` shows the
/// group's toggle counter. `─` is in WGL4 → no compat replacement needed.
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
        // A thin separator + a muted counter before the line.
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

/// A field row: label + value, colored by type (a toggle — green/
/// muted, a choice — blue, a dash/empty — the border color, text — the base color).
/// The value is truncated to `value_w` with "…" (fully visible in the bottom panel).
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
            // Gate: a tool enabled in the profile but disabled globally — in
            // warning color (it has no effect), not green.
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
    // Pad the label with spaces up to the column width by real width in columns
    // (Rust `{:<N}` counts characters, not columns — for CJK/emoji that drifts).
    let pad = label_col.saturating_sub(label_width(&f.label));
    // The left column (2 cells), fixed — fields look indented under the
    // group header. The selected field gets a green rail `▌` (like the active section
    // in the left menu); otherwise a "modified vs. default" marker `•`, otherwise
    // empty. The rail on the selected row takes priority over the marker (the field
    // is in focus anyway; step away and `•` returns).
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
    // An inline hint (the tool's description) right of the value — in the remaining width.
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

/// Truncates a string to `max` columns, adding "…" (WGL4-safe). Returns the
/// truncated string and its actual width in columns.
pub(super) fn truncate_to_width(s: &str, max: usize) -> (String, usize) {
    let chars: Vec<char> = s.chars().collect();
    let full = crate::shared::wrap::display_width(&chars);
    if full <= max {
        return (s.to_string(), full);
    }
    if max == 0 {
        return (String::new(), 0);
    }
    let budget = max.saturating_sub(1); // room for "…"
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

/// Cyclically changes the engine mode (5 values, direction-aware ←/→).
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

/// Cyclically changes the impersonation mode (6 values, direction-aware).
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

pub(super) fn python_mode_label(m: PythonMode, loc: &'static Locale) -> String {
    loc.t(match m {
        PythonMode::Wasmer => "ui.settings.choice.python_wasmer",
        PythonMode::Local => "ui.settings.choice.python_local",
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

/// Cyclically shifts the interface language across all known languages (built-in +
/// external, `Lang::all()`), direction-aware (`dir`).
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

/// The order of `reasoning_effort` options in the cycle/menu (matches [`REASONING_ORDER`]).
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

/// The order of `verbosity` options in the cycle/menu.
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

/// A field's numeric kind for validation (`None` — not numeric: text/URL/lists/choice).
/// Config fields — from the access table (`field_spec.num`); sampling — by its own kind.
pub(super) fn field_num_kind(id: FieldId) -> Option<NumKind> {
    match id {
        FieldId::S(p) | FieldId::IS(p) => p.num_kind(),
        _ => super::spec::field_spec(id).and_then(|s| s.num),
    }
}

/// The field-validation-error i18n key (`None` — valid). Empty input is allowed
/// (clear/keep the previous value); non-empty in a numeric field must parse. The check is
/// "soft" (i64/f64); the exact type and range are checked further by `apply_text`. Returns
/// the **key** — the caller resolves the text (`loc.t`) in the interface locale.
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

/// The order of engine-mode options (for the Choice popup; matches `cycle_mode`).
pub(super) const SERVER_MODES: [ServerMode; 5] = [
    ServerMode::Managed,
    ServerMode::External,
    ServerMode::OpenAi,
    ServerMode::Gemini,
    ServerMode::Claude,
];

/// The order of impersonation-mode options (matches `cycle_imp_mode`).
pub(super) const IMP_MODES: [ImpersonationMode; 6] = [
    ImpersonationMode::Shared,
    ImpersonationMode::Managed,
    ImpersonationMode::External,
    ImpersonationMode::OpenAi,
    ImpersonationMode::Gemini,
    ImpersonationMode::Claude,
];

/// The order of themes (matches `cycle_theme`).
pub(super) const THEMES: [Theme; 3] = [Theme::Auto, Theme::Dark, Theme::Light];

/// Builds (labels, the current index) from an option array and a labeling function.
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

/// The option menu for Choice sampling parameters (`Thinking`/`Reasoning`); the label
/// order matches the `cycle_opt_bool`/`cycle_reasoning` cycle.
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

/// The field belongs to a profile (it has no config default → doesn't participate in
/// the `•` marker/reset).
pub(super) fn is_profile_field(id: FieldId) -> bool {
    matches!(
        id,
        FieldId::PSelect
            | FieldId::PName
            | FieldId::PLanguage
            | FieldId::PSystem
            | FieldId::PGreeting
            | FieldId::PImpProfile
            | FieldId::PTool(_)
            | FieldId::ProfileSub
            // The impersonation-profile list lives in the config, but it's user data
            // (no meaningful "default value") — same treatment as profile fields.
            | FieldId::IpSelect
            | FieldId::IpName
            | FieldId::IpSystem
    )
}

pub(super) fn parse_opt<T: std::str::FromStr>(s: &str) -> Option<T> {
    if s.is_empty() { None } else { s.parse().ok() }
}

pub(super) fn parse_opt_f32(s: &str) -> Option<f32> {
    parse_opt(s)
}

/// Parses a text field into a list of strings (UI): splits on `sep`, trims
/// whitespace from elements, drops empty ones; empty input → `None`.
pub(super) fn parse_list(s: &str, sep: char) -> Option<Vec<String>> {
    let v: Vec<String> = s
        .split(sep)
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
        .collect();
    (!v.is_empty()).then_some(v)
}

/// Joins a list of strings for display via `sep`; `None`/empty → "—".
pub(super) fn join_list(v: Option<&[String]>, sep: char) -> String {
    match v {
        Some(items) if !items.is_empty() => items.join(&sep.to_string()),
        _ => "—".to_string(),
    }
}

/// DRY breakers: like [`parse_list`] on commas, but with decoding the `\n`/`\t`/`\r`
/// escapes (a single-line editor can't take them literally).
pub(super) fn parse_breakers(s: &str) -> Option<Vec<String>> {
    let v: Vec<String> = s
        .split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(decode_escapes)
        .collect();
    (!v.is_empty()).then_some(v)
}

/// DRY breakers for display: encodes control characters back into `\n`
/// etc., joins with commas; `None`/empty → "—".
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

/// Decodes `\n`/`\t`/`\r`/`\\` literals into real characters.
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

/// Encodes control characters into `\n`/`\t`/`\r`/`\\` literals (the inverse of [`decode_escapes`]).
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

/// The height of the large system-message editor popup: ~60% of the screen height,
/// but no less than 8 rows and no taller than the screen itself.
pub(super) fn multiline_popup_height(area: Rect) -> u16 {
    (area.height.saturating_mul(60) / 100)
        .max(8)
        .min(area.height)
}

/// A rectangle centered in `area`: `pct_x`% of the width (≥`min_w`), a fixed height.
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

/// A rectangle centered in `area` with explicit width/height (clamped to `area`).
pub(super) fn centered_rect_wh(width: u16, height: u16, area: Rect) -> Rect {
    let [h] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(area);
    let [v] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(h);
    v
}

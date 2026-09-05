//! Settings screen — free functions: field-row builders, descriptions,
//! layout/truncation, enum-value cycles, value parsers. Part of the
//! [super] module; split out of the settings.rs monolith (see docs/history/refactoring-god-objects.md).

use super::*;
use crate::shared::config::WebProvider;
// ---------- free functions ----------

/// Whether a sampling parameter is in the subset accepted by a cloud provider.
/// Gemini (strict OpenAI dialect, `restrict_to_strict`): temperature/top_p/
/// penalties/seed/max_tokens. OpenAI — the same **without `temperature`/`top_p`**: only
/// the GPT 5.4 family accepted them, and GPT 5.5/5.6 reject them. Anthropic (Claude):
/// **only `max_tokens`** — the newest
/// 4.x models "locked in" sampling and reject `temperature`/`top_p`/`top_k` as
/// deprecated, so we don't send them (see `anthropic::wire`). Grok (xAI):
/// `temperature`/`top_p`/`max_tokens`/`seed` — the penalties are a hard 400 and
/// `top_k`/`min_p` aren't in xAI's schema at all. The rest — llama.cpp
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
/// The video slot's key row: same storage, but it names the provider, because
/// this row is often the only place a Gemini key gets entered.
pub(super) const DESC_VIDEO_API_KEY: &str = "ui.settings.desc.video_api_key";
/// Key: cloud model name (X/Ix/E).
pub(super) const DESC_MODEL_NAME: &str = "ui.settings.desc.model_name";
/// The same field on an **external** server, where it means something different:
/// optional, sent to the server when set (a multi-model endpoint routes on it),
/// and with it blank the app asks the server for the name instead. See
/// docs/research/external-model-name.md.
pub(super) const DESC_MODEL_NAME_EXTERNAL: &str = "ui.settings.desc.model_name_external";
/// Key: the env-variable name holding the external-server key (optional).
pub(super) const DESC_EXT_API_KEY_ENV: &str = "ui.settings.desc.ext_api_key_env";
/// Key: the external server's own stored key (X/Ix/E/Tts) — optional, and it wins
/// over the variable named in the row below it. See docs/history/external-api-key.md.
pub(super) const DESC_EXT_API_KEY: &str = "ui.settings.desc.ext_api_key";
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
        warn_note: None,
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
    mmproj: FieldId,
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
    mmproj: FieldId::XMmproj,
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
    mmproj: FieldId::IxMmproj,
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
            // Directly under the GGUF path: the projector is the second half of the
            // same download, and pairing them is what makes the connection obvious.
            text_row(ids.mmproj, loc.t("ui.settings.field.mmproj"), &m.mmproj)
                .describe(loc.t("ui.settings.desc.mmproj")),
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

/// An input field holding a **secret** itself — a cloud API key or the backup
/// password (not an env-variable name)? Such fields have special behavior: an
/// empty seed, a masked editor, a commit as a separate intent, and `Del` as
/// deleting the secret. See [`SettingsScreen::api_key_field_provider`].
pub(super) fn is_secret_field(id: FieldId) -> bool {
    matches!(
        id,
        FieldId::McpEnvSecret(_)
            | FieldId::XApiKey
            | FieldId::IxApiKey
            | FieldId::EApiKey
            | FieldId::TtsApiKey
            | FieldId::VideoApiKey
            | FieldId::BackupPassword
    )
}

/// The "API key" field row: the value is a **status**, not a secret ("configured (this
/// computer)" / "not set"; if the machine doesn't support encryption — "unavailable",
/// the env path remains). Editing opens an empty masked editor: a stored
/// key can't be shown, entering it = replacing it. See docs/research/api-key-storage.md.
pub(super) fn api_key_row(id: FieldId, present: bool, loc: &'static Locale) -> FieldRow {
    secret_row(
        id,
        present,
        loc.t("ui.settings.field.api_key"),
        DESC_API_KEY,
        loc,
    )
}

/// The same row for an **external** server's key — the storage and every behaviour
/// are identical (see [`api_key_row`]), only the wording differs: here the key is
/// optional (a local `llama-server` needs none) and it sits directly above the
/// env-name row, so the description has to say which of the two decides.
/// See docs/history/external-api-key.md §5.5.
pub(super) fn ext_api_key_row(id: FieldId, present: bool, loc: &'static Locale) -> FieldRow {
    secret_row(
        id,
        present,
        loc.t("ui.settings.field.api_key_opt"),
        DESC_EXT_API_KEY,
        loc,
    )
}

/// A stored-secret row: the value shown is a **status**, and the description
/// switches to "not supported on this machine" when no encryption scheme is
/// available (Linux without machine-id), because then the field cannot work at
/// all. Shared by the API keys and the backup password.
pub(super) fn secret_row(
    id: FieldId,
    present: bool,
    // Not `&'static str`: an MCP environment row is labelled with the variable
    // name, which is user data.
    label: &str,
    desc_key: &str,
    loc: &'static Locale,
) -> FieldRow {
    let (value, desc) = if !crate::shared::secrets::scheme_available() {
        (
            loc.t("ui.settings.value.key_unsupported"),
            loc.t("ui.settings.desc.api_key_unsupported"),
        )
    } else if present {
        (loc.t("ui.settings.value.key_set"), loc.t(desc_key))
    } else {
        (loc.t("ui.settings.value.key_unset"), loc.t(desc_key))
    };
    row(id, label, FieldKind::Text(value.to_string())).describe(desc)
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

/// Bytes in one megabyte — the unit the image size limit is *shown* in while the
/// config stores bytes ([`crate::shared::config::ImageSettings::max_bytes`]).
/// Nobody types a byte count, and every provider quotes its own ceiling in MB.
pub(super) const BYTES_PER_MB: u64 = 1024 * 1024;

/// The stored byte ceiling as whole megabytes, for display. Truncating is
/// deliberate: the row is an editable number, so it must round-trip through
/// [`mb_to_bytes`] unchanged, which rounding up would break.
pub(super) fn bytes_to_mb(bytes: u64) -> u64 {
    bytes / BYTES_PER_MB
}

/// Megabytes typed in the row back into stored bytes.
///
/// Floored at 1 MB: zero is not a limit anyone means — it would refuse every image
/// ever attached, which reads as "the feature is broken" rather than as a setting.
/// Same taste as the compaction threshold's clamp.
pub(super) fn mb_to_bytes(mb: u64) -> u64 {
    mb.max(1) * BYTES_PER_MB
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
/// The ceiling of the value column: a long label may push the column up to here
/// and no further — beyond it the label itself is clipped ([`render_field_line`])
/// rather than shifting its own value right. All *static* labels fit within the cap
/// (test `all_labels_fit_alignment_cap`) — the cap is set by the widest of them, the
/// ru "confirm regeneration/deletion" toggle; the labels that do not fit are
/// user/server data — an MCP tool id `mcp__<server>__<tool>` (up to 64 characters)
/// or an env-var name.
pub(super) const LABEL_CAP: usize = 35;

/// The section's shared value column: the longest label among visible fields,
/// clamped to \[[`MIN_LABEL_COL`], [`LABEL_CAP`]\]. One column per section
/// (not per group): the values of all groups line up on a shared vertical — a per-group
/// column "sawtoothed" with different stops from group to group. An over-cap label
/// raises the column *to the cap* (so a clipped label keeps as much of itself as the
/// cap allows) and no further — the section's remaining width belongs to the values
/// and their inline hints. The subsection selector is drawn as a tab strip, not a
/// list row — excluded from alignment.
pub(super) fn section_label_col(fields: &[FieldRow]) -> usize {
    fields
        .iter()
        .filter(|f| !is_subsection(f.id))
        .map(|f| label_width(&f.label))
        .max()
        .unwrap_or(0)
        .clamp(MIN_LABEL_COL, LABEL_CAP)
}

/// The bottom hint panel's floor in content rows (the border is extra): a section
/// of short hints keeps the panel it has always had, so the height only ever grows.
pub(super) const HINT_MIN_ROWS: usize = 3;
/// ...and its ceiling: an MCP server's tool description is arbitrary text, so
/// without a cap one field could push the list off the screen. Further bounded by
/// the pane height at the call site.
pub(super) const HINT_MAX_ROWS: usize = 12;
/// The tallest fields-pane header: the section title plus a tab strip. The hint
/// panel's cap subtracts this constant rather than the current section's real
/// header, so tabbed and untabbed sections cannot end up with different caps.
pub(super) const HEAD_MAX_ROWS: usize = 2;

/// Rows a plain text occupies when wrapped to `width` (an embedded newline is a
/// hard break). Counting only — the wrapped rows themselves aren't built.
pub(super) fn wrapped_rows(text: &str, width: usize) -> usize {
    if width == 0 {
        return 0;
    }
    text.split('\n')
        .map(|l| crate::shared::wrap::wrap_ranges(&l.chars().collect::<Vec<_>>(), width).len())
        .sum()
}

/// Wraps plain text into styled visual rows of `width` (an embedded newline is a
/// hard break). Pre-wrapping — rather than leaving it to `Paragraph`'s `Wrap` — is
/// what lets the panel be sized to its content before it is laid out.
pub(super) fn wrap_text(text: &str, style: Style, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    text.split('\n')
        .flat_map(|l| crate::shared::wrap::wrap_line(&Line::styled(l.to_string(), style), width))
        .collect()
}

/// The full-value preview for the bottom panel: at most `rows` wrapped lines of
/// the value's head. A value the panel cannot show whole ends with a visible
/// `…` — the previous fixed character cap used to stop a wide panel mid-word
/// with rows to spare, which read as the text simply ending there.
pub(super) fn value_preview(
    value: &str,
    rows: usize,
    width: usize,
    style: Style,
) -> Vec<Line<'static>> {
    if rows == 0 || width == 0 {
        return Vec::new();
    }
    // Enough characters to fill the rows plus one row's slack to detect
    // overflow — never the whole value: a system message can be huge, and
    // wrapping all of it on every frame would be paid for nothing.
    let budget = rows * width + width;
    let mut it = value.chars();
    let head: String = it.by_ref().take(budget).collect();
    let clipped_chars = it.next().is_some();
    let mut lines = wrap_text(&head, style, width);
    let clipped = clipped_chars || lines.len() > rows;
    lines.truncate(rows);
    if clipped {
        ellipsize_last(&mut lines, width);
    }
    lines
}

/// Rewrites the last line to visibly end in `…` within `width` — the marker that
/// more text exists than the panel shows. Keeps the line's own style.
pub(super) fn ellipsize_last(lines: &mut [Line<'static>], width: usize) {
    let Some(last) = lines.last_mut() else {
        return;
    };
    let style = last.spans.first().map(|s| s.style).unwrap_or_default();
    let text: String = last.spans.iter().map(|s| s.content.as_ref()).collect();
    // `truncate_to_width` appends its own "…" when it has to cut, so the marker
    // survives whether the padded text fits or not.
    let (marked, _) = truncate_to_width(&format!("{} …", text.trim_end()), width);
    *last = Line::styled(marked, style);
}

/// Rows one field's hint needs: its description plus, for a globally-gated tool,
/// the expanded gate warning below it.
pub(super) fn hint_rows(f: &FieldRow, width: usize, loc: &'static Locale) -> usize {
    let mut n = f
        .description
        .as_deref()
        .map_or(0, |t| wrapped_rows(t, width));
    if f.warn {
        n += wrapped_rows(loc.t("ui.settings.ui.gate_warn"), width);
    }
    n
}

/// The bottom panel's content height for a field set: the **longest** hint in it,
/// within [`HINT_MIN_ROWS`]`..=cap`. Sized per field set rather than per field, so
/// no hint is ever clipped mid-sentence *and* stepping between fields never shifts
/// the list under the cursor (a per-field height would do the latter constantly).
/// The full value preview isn't counted — it fills whatever the hint leaves, so a
/// long path or system message can't inflate the panel for the whole section.
pub(super) fn hint_panel_rows(
    fields: &[FieldRow],
    width: usize,
    cap: usize,
    loc: &'static Locale,
) -> usize {
    fields
        .iter()
        .map(|f| hint_rows(f, width, loc))
        .max()
        .unwrap_or(0)
        .clamp(HINT_MIN_ROWS, cap.max(HINT_MIN_ROWS))
}

/// An inline hint for a tool disabled by a global gate ("disabled globally:
/// <switch>"). Shown in warning color.
pub(super) fn gate_hint(gate: ToolGate, loc: &'static Locale) -> &'static str {
    loc.t(match gate {
        ToolGate::Web => "ui.settings.gate.web",
        ToolGate::Python => "ui.settings.gate.python",
        ToolGate::Fs => "ui.settings.gate.fs",
        ToolGate::Mcp => "ui.settings.gate.mcp",
        ToolGate::Background => "ui.settings.gate.background",
    })
}

/// The expanded explanation for a tool that is on in the profile but off
/// globally — naming the section that actually holds the switch. The MCP master
/// switch moved to "Plugins" when the server editor got its own section, so a
/// single hardcoded "Tools" was wrong for it.
pub(super) fn gate_warn_note(gate: ToolGate, loc: &'static Locale) -> String {
    let section = loc.t(match gate {
        ToolGate::Mcp => "ui.settings.section.plugins",
        _ => "ui.settings.section.tools",
    });
    loc.tf("ui.settings.ui.gate_warn", &[("section", section)])
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
            id: f.id,
            section_idx,
            subsection,
            field_idx: fi,
            crumb,
            value,
            haystack,
        });
    }
}

/// The style of a pane's focus marker: the `▸` on the section menu's title and the `◆`
/// on the field pane's — green when that pane holds the focus, muted otherwise.
///
/// The two markers are a **pair**, hence one helper: they encode the same fact from
/// opposite sides, and having them drift apart would make the screen lie about where
/// the focus is. The glyph itself always stays in place and only its colour changes —
/// showing/hiding it (as the menu title used to) both flickers and shifts the title
/// text sideways on every focus change.
pub(super) fn focus_marker_style(focused: bool, palette: &Palette) -> Style {
    Style::new().fg(if focused {
        palette.success
    } else {
        palette.muted
    })
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
/// The value is truncated to `value_w` with "…" (fully visible in the bottom panel),
/// and the label to `label_col` — the value column holds unconditionally.
pub(super) fn render_field_line(
    f: &FieldRow,
    label_col: usize,
    value_w: usize,
    modified: bool,
    selected: bool,
    palette: &Palette,
) -> Line<'static> {
    let (value, value_style) = field_value_and_style(f, palette);
    let (value, vw) = truncate_to_width(&value, value_w.max(1));
    // A label wider than the section's column is clipped with "…" instead of
    // pushing its own value right: such labels are user/server data (an MCP tool id
    // `mcp__<server>__<tool>`, an env-var name), and one of them was enough to break
    // the single vertical the shared column exists for (spec §11.6). The full text
    // stays reachable — the inline hint carries the bare tool name, the bottom panel
    // the description.
    let (label, lw) = truncate_to_width(&f.label, label_col);
    // Pad the label with spaces up to the column width by real width in columns
    // (Rust `{:<N}` counts characters, not columns — for CJK/emoji that drifts).
    let pad = label_col.saturating_sub(lw);
    let marker = row_marker(selected, modified, palette);
    let mut spans = vec![
        marker,
        Span::styled(label, palette.muted_style()),
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

/// A field's displayed value and its style, by kind (the color coding described
/// on [`render_field_line`]).
fn field_value_and_style(f: &FieldRow, palette: &Palette) -> (String, Style) {
    match &f.kind {
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
    }
}

/// A field row's 2-cell left marker.
fn row_marker(selected: bool, modified: bool, palette: &Palette) -> Span<'static> {
    // The left column (2 cells), fixed — fields look indented under the
    // group header. The selected field gets a green rail `▌` (like the active section
    // in the left menu); otherwise a "modified vs. default" marker `•`, otherwise
    // empty. The rail on the selected row takes priority over the marker (the field
    // is in focus anyway; step away and `•` returns).
    if selected {
        Span::styled("▌ ", Style::new().fg(palette.success))
    } else if modified {
        Span::styled("• ", Style::new().fg(palette.accent))
    } else {
        Span::raw("  ")
    }
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
        ServerMode::Grok => "grok".into(),
    }
}

/// Cyclically changes the engine mode (6 values, direction-aware ←/→).
pub(super) fn cycle_mode(m: ServerMode, dir: i32) -> ServerMode {
    use ServerMode::*;
    let order = [Managed, External, OpenAi, Gemini, Claude, Grok];
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
        ImpersonationMode::Grok => "grok".into(),
    }
}

/// Cyclically changes the impersonation mode (7 values, direction-aware).
pub(super) fn cycle_imp_mode(m: ImpersonationMode, dir: i32) -> ImpersonationMode {
    use ImpersonationMode::*;
    let order = [Shared, Managed, External, OpenAi, Gemini, Claude, Grok];
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

/// The preferred `web_search` backend's label (spec §9.3.1).
pub(super) fn web_provider_label(m: WebProvider, loc: &'static Locale) -> String {
    loc.t(match m {
        WebProvider::Auto => "ui.settings.choice.web_provider_auto",
        WebProvider::FreeOnly => "ui.settings.choice.web_provider_free",
    })
    .to_string()
}

/// The description key of the keyed-search API-key rows.
pub(super) const DESC_SEARCH_API_KEY: &str = "ui.settings.desc.search_api_key";

/// The video-understanding frame-sampling detail's label
/// (`youtube_watch`/`config.video.media_resolution`).
pub(super) fn video_resolution_label(m: MediaResolution, loc: &'static Locale) -> String {
    loc.t(match m {
        MediaResolution::Low => "ui.settings.choice.video_res_low",
        MediaResolution::Medium => "ui.settings.choice.video_res_medium",
    })
    .to_string()
}

/// The OSC-52 mode's label (`interface.clipboard_osc52`).
pub(super) fn osc52_label(m: Osc52Mode, loc: &'static Locale) -> String {
    loc.t(match m {
        Osc52Mode::Auto => "ui.settings.choice.osc52_auto",
        Osc52Mode::Always => "ui.settings.choice.osc52_always",
        Osc52Mode::Off => "ui.settings.choice.osc52_off",
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
        // The selected MCP server's numbers — indexed, so outside the access table.
        FieldId::McpTimeout | FieldId::McpMaxResult => Some(NumKind::Int),
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
pub(super) const SERVER_MODES: [ServerMode; 6] = [
    ServerMode::Managed,
    ServerMode::External,
    ServerMode::OpenAi,
    ServerMode::Gemini,
    ServerMode::Claude,
    ServerMode::Grok,
];

/// The order of impersonation-mode options (matches `cycle_imp_mode`).
pub(super) const IMP_MODES: [ImpersonationMode; 7] = [
    ImpersonationMode::Shared,
    ImpersonationMode::Managed,
    ImpersonationMode::External,
    ImpersonationMode::OpenAi,
    ImpersonationMode::Gemini,
    ImpersonationMode::Claude,
    ImpersonationMode::Grok,
];

/// The order of OSC-52 modes (matches `cycle_osc52`).
pub(super) const OSC52_MODES: [Osc52Mode; 3] = [Osc52Mode::Auto, Osc52Mode::Always, Osc52Mode::Off];

/// The order of automatic-titling modes: "after the user's message" first
/// (user's decision, docs/history/auto-chat-title.md §2), Off last like every
/// tri-state here. The default (`AfterAssistantReply`) sits in the middle.
pub(super) const AUTO_TITLE_MODES: [AutoTitleMode; 3] = [
    AutoTitleMode::AfterUserMessage,
    AutoTitleMode::AfterAssistantReply,
    AutoTitleMode::Off,
];

/// Cyclically shifts the automatic-titling mode, direction-aware.
pub(super) fn cycle_auto_title(m: AutoTitleMode, dir: i32) -> AutoTitleMode {
    let idx = AUTO_TITLE_MODES.iter().position(|&x| x == m).unwrap_or(0) as i32;
    let n = AUTO_TITLE_MODES.len() as i32;
    AUTO_TITLE_MODES[(((idx + dir) % n + n) % n) as usize]
}

/// The automatic-titling mode's label (`interface.auto_title`).
pub(super) fn auto_title_label(m: AutoTitleMode, loc: &'static Locale) -> String {
    loc.t(match m {
        AutoTitleMode::AfterUserMessage => "ui.settings.choice.auto_title_user",
        AutoTitleMode::AfterAssistantReply => "ui.settings.choice.auto_title_assistant",
        AutoTitleMode::Off => "ui.settings.choice.auto_title_off",
    })
    .to_string()
}

/// The order of self-model observation orders (matches `cycle_note_order`): the
/// default — newest first — is the entry the row opens on.
pub(super) const NOTE_ORDERS: [NoteOrder; 2] = [NoteOrder::NewestFirst, NoteOrder::OldestFirst];

/// Cyclically shifts the self-model observation order, direction-aware.
pub(super) fn cycle_note_order(m: NoteOrder, dir: i32) -> NoteOrder {
    let idx = NOTE_ORDERS.iter().position(|&x| x == m).unwrap_or(0) as i32;
    let n = NOTE_ORDERS.len() as i32;
    NOTE_ORDERS[(((idx + dir) % n + n) % n) as usize]
}

/// The self-model observation order's label (`interface.self_model_note_order`).
pub(super) fn note_order_label(m: NoteOrder, loc: &'static Locale) -> String {
    loc.t(match m {
        NoteOrder::NewestFirst => "ui.settings.choice.note_order_newest",
        NoteOrder::OldestFirst => "ui.settings.choice.note_order_oldest",
    })
    .to_string()
}

/// Cyclically shifts the OSC-52 mode, direction-aware.
pub(super) fn cycle_osc52(m: Osc52Mode, dir: i32) -> Osc52Mode {
    let idx = OSC52_MODES.iter().position(|&x| x == m).unwrap_or(0) as i32;
    let n = OSC52_MODES.len() as i32;
    OSC52_MODES[(((idx + dir) % n + n) % n) as usize]
}

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
            | FieldId::PUserName
            | FieldId::PAssistantName
            | FieldId::PImpProfile
            | FieldId::PTool(_)
            | FieldId::ProfileSub
            // The impersonation-profile list lives in the config, but it's user data
            // (no meaningful "default value") — same treatment as profile fields.
            | FieldId::IpSelect
            | FieldId::IpName
            | FieldId::IpSystem
            // MCP servers live in the config too, but they are an inventory the
            // user authors — there is no meaningful "default server" to reset to.
            | FieldId::McpSelect
            | FieldId::McpId
            | FieldId::McpCommand
            | FieldId::McpArgs
            | FieldId::McpEnv
            | FieldId::McpEnvSecret(_)
            | FieldId::McpEnvSource(_)
            | FieldId::McpImport
            | FieldId::McpEnabled
            | FieldId::McpTimeout
            | FieldId::McpMaxResult
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

/// An MCP server's `args` as a command line, and back. Both are
/// [`crate::shared::cmdline`]'s job — the code workspace's command slots need
/// the same shell-style splitting, and `features` cannot import `screens`, so
/// the pair moved down a layer rather than being copied (docs/lessons.md §2).
pub(super) fn join_args(args: &[String]) -> String {
    crate::shared::cmdline::join(args)
}

/// A command line back into an MCP server's `args`. See [`join_args`].
pub(super) fn parse_args(s: &str) -> Vec<String> {
    crate::shared::cmdline::split(s)
}

/// An MCP server's `env` map as text: `CHILD=SOURCE, CHILD2=SOURCE2`. Both
/// sides are environment **variable names** (the value is the name of the
/// source variable, not a secret — ADR 0007 R8), and a variable name contains
/// neither `,` nor `=`, which is what makes this flat form unambiguous.
pub(super) fn join_env_map(env: &std::collections::BTreeMap<String, String>) -> String {
    env.iter()
        .map(|(k, v)| {
            if v.is_empty() {
                k.clone()
            } else {
                format!("{k}={v}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Text back into an MCP server's `env` map. A bare `VARIABLE` declares it with
/// no source — the usual form, since the value is then entered in the row below
/// (or simply inherited: the child gets the app's whole environment). The
/// `VARIABLE=SOURCE` form remains for the rare case of taking the value from a
/// *differently named* variable. A name we cannot carry (`valid_env_name`) is
/// dropped — the field is edited character by character, so a half-typed entry
/// must not break the ones already there.
pub(super) fn parse_env_map(s: &str) -> std::collections::BTreeMap<String, String> {
    s.split(',')
        .filter_map(|part| {
            let (k, v) = part.split_once('=').unwrap_or((part, ""));
            let (k, v) = (k.trim(), v.trim());
            crate::shared::mcp::valid_env_name(k).then(|| (k.to_string(), v.to_string()))
        })
        .collect()
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

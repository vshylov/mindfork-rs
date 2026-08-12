//! Settings screen — the access table for config-field values (`field_spec`). One
//! match on [`FieldId`] instead of the previous four (`toggle_field`/`cycle_field`/
//! `apply_text`/`field_num_kind`) — a field's whole story (how to read/write/cycle/validate
//! the value) is described on one line. See docs/history/refactoring-solid.md §5 (step 3.2).
//!
//! **Scope boundary.** Only config fields (over `AppConfig`). Outside the table (the
//! previous path in apply.rs): profile fields (`PName`/`PSystem`/…, over `profiles[idx]`),
//! sampling parameters `S(p)`/`IS(p)` (their own `SamplingParam` descriptor), subsection
//! selectors and `PSelect` (navigation). Label/group/description/value-row rendering stays
//! in the catalog builders (catalog.rs) — this table only handles value access.

use super::helpers::*;
use super::*;

/// How to read/change a config field's value. `fn` pointers (not closures) — `'static`,
/// no capturing; routing by mode (external vs cloud) lives **inside** the setter
/// (it has access to the whole `AppConfig`).
pub(super) enum Access {
    /// Toggle: flips the value in place.
    Toggle(fn(&mut AppConfig)),
    /// Text/number: parses `trimmed` and assigns (parsing semantics live in the setter itself).
    Text(fn(&mut AppConfig, &str)),
    /// Cyclic choice: a step by direction + the option list with the current index.
    /// `options` receives the locale — some labels (theme) are localized.
    Choice {
        cycle: fn(&mut AppConfig, i32),
        options: fn(&AppConfig, &'static Locale) -> (Vec<String>, usize),
    },
}

/// The access spec for a config field's value.
pub(super) struct FieldSpec {
    pub(super) access: Access,
    /// Numeric kind for editor validation (`None` — text/list/choice).
    pub(super) num: Option<NumKind>,
}

/// Empty → `None` (clears `Option<String>`), else `Some(text)`. Mirrors the local
/// `opt` from the previous `apply_text`.
fn opt(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

/// **The single** match on `FieldId` for config-field value access. `None` —
/// the field is out of scope (profile/sampling/navigation — the previous path in apply.rs).
pub(super) fn field_spec(id: FieldId) -> Option<FieldSpec> {
    use FieldId::*;
    let spec = |access, num| FieldSpec { access, num };
    let toggle = |f: fn(&mut AppConfig)| spec(Access::Toggle(f), None);
    // A plain text string field (no numeric validation).
    let text = |f: fn(&mut AppConfig, &str)| spec(Access::Text(f), None);
    // An integer field (validated as i64).
    let int = |f: fn(&mut AppConfig, &str)| spec(Access::Text(f), Some(NumKind::Int));
    let choice = |cycle, options| spec(Access::Choice { cycle, options }, None);

    Some(match id {
        // ---------- toggles ----------
        XJinja => toggle(|c| c.engine.managed.jinja = !c.engine.managed.jinja),
        XNoMmap => toggle(|c| c.engine.managed.no_mmap = !c.engine.managed.no_mmap),
        IxJinja => {
            toggle(|c| c.impersonation_engine.managed.jinja = !c.impersonation_engine.managed.jinja)
        }
        IxNoMmap => toggle(|c| {
            c.impersonation_engine.managed.no_mmap = !c.impersonation_engine.managed.no_mmap
        }),
        TWeb => toggle(|c| c.tools.web_enabled = !c.tools.web_enabled),
        TWebFetch => toggle(|c| c.tools.web_fetch_content = !c.tools.web_fetch_content),
        TPython => toggle(|c| c.tools.python_enabled = !c.tools.python_enabled),
        TPythonNet => toggle(|c| c.tools.python_net_enabled = !c.tools.python_net_enabled),
        TFs => toggle(|c| c.tools.fs_enabled = !c.tools.fs_enabled),
        TConfirmDangerous => toggle(|c| c.tools.confirm_dangerous = !c.tools.confirm_dangerous),
        TMcpEnabled => toggle(|c| c.mcp.enabled = !c.mcp.enabled),
        TMcpImages => toggle(|c| c.tools.mcp_images = !c.tools.mcp_images),
        ICompat => toggle(|c| c.interface.terminal_compat = !c.interface.terminal_compat),
        ITableSeparators => {
            toggle(|c| c.interface.table_row_separators = !c.interface.table_row_separators)
        }
        IMermaid => toggle(|c| c.interface.render_mermaid = !c.interface.render_mermaid),
        ISpell => toggle(|c| c.interface.spellcheck_enabled = !c.interface.spellcheck_enabled),
        IConfirmKeys => {
            toggle(|c| c.interface.confirm_destructive_keys = !c.interface.confirm_destructive_keys)
        }
        SmProtocol => {
            toggle(|c| c.self_model.maintenance_protocol = !c.self_model.maintenance_protocol)
        }
        NotesRecallIncludesSelf => {
            toggle(|c| c.notes.recall_includes_self = !c.notes.recall_includes_self)
        }
        CompactEnabled => toggle(|c| c.compaction.enabled = !c.compaction.enabled),
        ICopyThoughts => toggle(|c| c.copy.copy_thoughts = !c.copy.copy_thoughts),
        ICopyToolCalls => toggle(|c| c.copy.copy_tool_calls = !c.copy.copy_tool_calls),
        ICopyToolResults => toggle(|c| c.copy.copy_tool_results = !c.copy.copy_tool_results),

        // ---------- cyclic choices ----------
        XMode => choice(
            |c, dir| c.engine.mode = cycle_mode(c.engine.mode, dir),
            |c, _loc| index_menu(&SERVER_MODES, c.engine.mode, mode_label),
        ),
        EMode => choice(
            |c, dir| c.embed.mode = cycle_mode(c.embed.mode, dir),
            |c, _loc| index_menu(&SERVER_MODES, c.embed.mode, mode_label),
        ),
        EConvention => choice(
            |c, dir| c.embed.convention = c.embed.convention.cycle(dir),
            |c, _loc| {
                index_menu(&EmbedConvention::ALL, c.embed.convention, |x| {
                    x.id().to_string()
                })
            },
        ),
        IxMode => choice(
            |c, dir| c.impersonation_engine.mode = cycle_imp_mode(c.impersonation_engine.mode, dir),
            |c, _loc| index_menu(&IMP_MODES, c.impersonation_engine.mode, imp_mode_label),
        ),
        TPythonMode => choice(
            |c, dir| c.tools.python_mode = c.tools.python_mode.cycle(dir),
            |c, loc| {
                index_menu(&PythonMode::ALL, c.tools.python_mode, |x| {
                    python_mode_label(x, loc)
                })
            },
        ),
        VideoResolution => choice(
            |c, dir| c.video.media_resolution = c.video.media_resolution.cycle(dir),
            |c, loc| {
                index_menu(&MediaResolution::ALL, c.video.media_resolution, |x| {
                    video_resolution_label(x, loc)
                })
            },
        ),
        XFlashAttn => choice(
            |c, dir| c.engine.managed.flash_attn = c.engine.managed.flash_attn.cycle(dir),
            |c, _loc| flash_menu(c.engine.managed.flash_attn),
        ),
        IxFlashAttn => choice(
            |c, dir| {
                let m = &mut c.impersonation_engine.managed;
                m.flash_attn = m.flash_attn.cycle(dir)
            },
            |c, _loc| flash_menu(c.impersonation_engine.managed.flash_attn),
        ),
        XSpecType => choice(
            |c, dir| c.engine.managed.spec_type = c.engine.managed.spec_type.cycle(dir),
            |c, _loc| spec_menu(c.engine.managed.spec_type),
        ),
        IxSpecType => choice(
            |c, dir| {
                let m = &mut c.impersonation_engine.managed;
                m.spec_type = m.spec_type.cycle(dir)
            },
            |c, _loc| spec_menu(c.impersonation_engine.managed.spec_type),
        ),
        // The theme cycles in one direction — `dir` is ignored (as in the previous cycle_field).
        ITheme => choice(
            |c, _dir| c.interface.theme = cycle_theme(c.interface.theme),
            |c, loc| index_menu(&THEMES, c.interface.theme, |t| theme_label(t, loc)),
        ),
        // Interface language (axis B): each language shown in its own name (`Lang::label`),
        // not translated by the UI language. The list — all known ones (built-in + external).
        ILanguage => choice(
            |c, dir| c.interface.language = cycle_lang(c.interface.language, dir),
            |c, _loc| {
                index_menu(
                    &crate::shared::i18n::Lang::all(),
                    c.interface.language,
                    |l| l.label().to_string(),
                )
            },
        ),

        // ---------- text: the assistant engine ----------
        // URL/model routed by mode (external → external.*, cloud → cloud_mut()).
        XUrl => text(|c, t| {
            if c.engine.mode == ServerMode::External {
                c.engine.external.url = opt(t);
            } else if let Some(cl) = c.engine.cloud_mut() {
                cl.url = opt(t);
            }
        }),
        XModelName => text(|c, t| {
            if c.engine.mode == ServerMode::External {
                c.engine.external.model_name = opt(t);
            } else if let Some(cl) = c.engine.cloud_mut() {
                cl.model_name = opt(t);
            }
        }),
        XApiKeyEnv => text(|c, t| {
            if c.engine.mode == ServerMode::External {
                c.engine.external.api_key_env = opt(t);
            } else if let Some(cl) = c.engine.cloud_mut() {
                cl.api_key_env = opt(t);
            }
        }),
        XBinary => text(|c, t| c.engine.managed.binary = opt(t)),
        XModel => text(|c, t| c.engine.managed.model_path = opt(t)),
        XMmproj => text(|c, t| c.engine.managed.mmproj = opt(t)),
        XDraftModel => text(|c, t| c.engine.managed.draft_model = opt(t)),
        XDraftNgl => int(|c, t| {
            c.engine.managed.draft_gpu_layers = parse_opt_num(t, c.engine.managed.draft_gpu_layers)
        }),
        XDraftNMax => int(|c, t| {
            c.engine.managed.draft_n_max = parse_opt_num(t, c.engine.managed.draft_n_max)
        }),
        XDraftNMin => int(|c, t| {
            c.engine.managed.draft_n_min = parse_opt_num(t, c.engine.managed.draft_n_min)
        }),
        XHost => text(|c, t| {
            if !t.is_empty() {
                c.engine.managed.host = t.to_string();
            }
        }),
        XNgl => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.engine.managed.gpu_layers = v;
            }
        }),
        XCtx => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.engine.managed.context_size = v;
            }
        }),
        XPort => int(|c, t| {
            if let Ok(p) = t.parse() {
                c.engine.managed.port = p;
            }
        }),

        // ---------- text: the impersonation engine ----------
        IxUrl => text(|c, t| {
            if c.impersonation_engine.mode == ImpersonationMode::External {
                c.impersonation_engine.external.url = opt(t);
            } else if let Some(cl) = c.impersonation_engine.cloud_mut() {
                cl.url = opt(t);
            }
        }),
        IxModelName => text(|c, t| {
            if c.impersonation_engine.mode == ImpersonationMode::External {
                c.impersonation_engine.external.model_name = opt(t);
            } else if let Some(cl) = c.impersonation_engine.cloud_mut() {
                cl.model_name = opt(t);
            }
        }),
        IxApiKeyEnv => text(|c, t| {
            if c.impersonation_engine.mode == ImpersonationMode::External {
                c.impersonation_engine.external.api_key_env = opt(t);
            } else if let Some(cl) = c.impersonation_engine.cloud_mut() {
                cl.api_key_env = opt(t);
            }
        }),
        IxBinary => text(|c, t| c.impersonation_engine.managed.binary = opt(t)),
        IxModel => text(|c, t| c.impersonation_engine.managed.model_path = opt(t)),
        IxMmproj => text(|c, t| c.impersonation_engine.managed.mmproj = opt(t)),
        IxDraftModel => text(|c, t| c.impersonation_engine.managed.draft_model = opt(t)),
        IxDraftNgl => int(|c, t| {
            let m = &mut c.impersonation_engine.managed;
            m.draft_gpu_layers = parse_opt_num(t, m.draft_gpu_layers)
        }),
        IxDraftNMax => int(|c, t| {
            let m = &mut c.impersonation_engine.managed;
            m.draft_n_max = parse_opt_num(t, m.draft_n_max)
        }),
        IxDraftNMin => int(|c, t| {
            let m = &mut c.impersonation_engine.managed;
            m.draft_n_min = parse_opt_num(t, m.draft_n_min)
        }),
        IxHost => text(|c, t| {
            if !t.is_empty() {
                c.impersonation_engine.managed.host = t.to_string();
            }
        }),
        IxNgl => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.impersonation_engine.managed.gpu_layers = v;
            }
        }),
        IxCtx => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.impersonation_engine.managed.context_size = v;
            }
        }),
        IxPort => int(|c, t| {
            if let Ok(p) = t.parse() {
                c.impersonation_engine.managed.port = p;
            }
        }),

        // ---------- text: the embedding server ----------
        EUrl => text(|c, t| {
            if c.embed.mode == ServerMode::External {
                c.embed.external.url = opt(t);
            } else if let Some(cl) = c.embed.cloud_mut() {
                cl.url = opt(t);
            }
        }),
        EModelName => text(|c, t| {
            if c.embed.mode == ServerMode::External {
                c.embed.external.model_name = opt(t);
            } else if let Some(cl) = c.embed.cloud_mut() {
                cl.model_name = opt(t);
            }
        }),
        EApiKeyEnv => text(|c, t| {
            if c.embed.mode == ServerMode::External {
                c.embed.external.api_key_env = opt(t);
            } else if let Some(cl) = c.embed.cloud_mut() {
                cl.api_key_env = opt(t);
            }
        }),
        EBinary => text(|c, t| c.embed.managed.binary = opt(t)),
        EModel => text(|c, t| c.embed.managed.model_path = opt(t)),
        // ── Speech (TTS) ──────────────────────────────────────────────
        TtsMode => choice(
            |c, dir| c.tts.mode = c.tts.mode.cycle(dir),
            |c, _loc| {
                index_menu(&crate::shared::config::TtsMode::ALL, c.tts.mode, |m| {
                    m.label().to_string()
                })
            },
        ),
        // Text fields routed by mode: external — its own substructure,
        // cloud — the active provider's substructure (as for the engine).
        TtsModelName => text(|c, t| {
            if c.tts.mode == crate::shared::config::TtsMode::External {
                c.tts.external.model_name = opt(t);
            } else if let Some(cl) = c.tts.cloud_mut() {
                cl.model_name = opt(t);
            }
        }),
        TtsVoice => text(|c, t| {
            if c.tts.mode == crate::shared::config::TtsMode::External {
                c.tts.external.voice = opt(t);
            } else if let Some(cl) = c.tts.cloud_mut() {
                cl.voice = opt(t);
            }
        }),
        TtsUserVoice => text(|c, t| {
            if c.tts.mode == crate::shared::config::TtsMode::External {
                c.tts.external.user_voice = opt(t);
            } else if let Some(cl) = c.tts.cloud_mut() {
                cl.user_voice = opt(t);
            }
        }),
        TtsInstructions => text(|c, t| {
            if let Some(cl) = c.tts.cloud_mut() {
                cl.instructions = opt(t);
            }
        }),
        TtsApiKeyEnv => text(|c, t| {
            if c.tts.mode == crate::shared::config::TtsMode::External {
                c.tts.external.api_key_env = opt(t);
            } else if let Some(cl) = c.tts.cloud_mut() {
                cl.api_key_env = opt(t);
            }
        }),
        TtsUrl => text(|c, t| {
            if c.tts.mode == crate::shared::config::TtsMode::External {
                c.tts.external.url = opt(t);
            } else if let Some(cl) = c.tts.cloud_mut() {
                cl.url = opt(t);
            }
        }),
        TtsSpeed => spec(
            Access::Text(|c, t| {
                if let Ok(v) = t.parse::<f32>()
                    && v > 0.0
                {
                    c.tts.speed = v;
                }
            }),
            Some(NumKind::Float),
        ),
        TtsSpeakRoles => toggle(|c| c.tts.speak_roles = !c.tts.speak_roles),
        TtsStopOnSwitch => toggle(|c| c.tts.stop_on_chat_switch = !c.tts.stop_on_chat_switch),
        TtsStopOnGeneration => {
            toggle(|c| c.tts.stop_on_generation_start = !c.tts.stop_on_generation_start)
        }
        EPort => int(|c, t| {
            if let Ok(p) = t.parse() {
                c.embed.managed.port = p;
            }
        }),

        // ---------- text: tools / memory / interface ----------
        MaxToolRounds => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.max_tool_rounds = v;
            }
        }),
        TPythonPath => text(|c, t| c.tools.python_path = opt(t)),
        TPythonWasmTimeout => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.tools.python_wasm_timeout_secs = v;
            }
        }),
        // Memory limit: empty/0/invalid → no limit (None), else Some(MB).
        TPythonWasmMemory => int(|c, t| {
            c.tools.python_wasm_memory_mb = t.trim().parse::<u64>().ok().filter(|&m| m > 0);
        }),
        VideoModel => text(|c, t| c.video.model_name = opt(t)),
        VideoMaxMinutes => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.video.max_minutes = v;
            }
        }),
        VideoApiKeyEnv => text(|c, t| c.video.api_key_env = opt(t)),
        TFsRoot => text(|c, t| c.tools.fs_root = opt(t)),
        TSubMaxTokens => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.tools.subagent_max_tokens = v;
            }
        }),
        TSubTimeout => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.tools.subagent_timeout_secs = v;
            }
        }),
        RagTarget => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.rag.chunk_target_chars = v;
            }
        }),
        RagOverlap => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.rag.chunk_overlap_chars = v;
            }
        }),
        RagMax => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.rag.chunk_max_chars = v;
            }
        }),
        AttachMaxFile => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.attachments.max_file_tokens = v;
            }
        }),
        AttachMaxTotal => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.attachments.max_total_tokens = v;
            }
        }),
        AttachExcerpt => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.attachments.excerpt_tokens = v;
            }
        }),
        AttachPage => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.attachments.page_tokens = v;
            }
        }),
        ImageMaxCount => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.images.max_count = v;
            }
        }),
        // Typed in MB, stored in bytes — see `mb_to_bytes` for why the row is not
        // simply the raw field.
        ImageMaxBytes => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.images.max_bytes = mb_to_bytes(v);
            }
        }),
        ImageDownscale => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.images.downscale_px = v;
            }
        }),
        SmMaxNarrative => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.self_model.max_narrative = v;
            }
        }),
        SmNarrativeInPrompt => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.self_model.narrative_in_prompt = v;
            }
        }),
        SmPromptCap => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.self_model.prompt_cap = v;
            }
        }),
        SmSummaryTarget => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.self_model.summary_target_chars = v;
            }
        }),
        SmAutoReflect => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.self_model.auto_reflect_every = v;
            }
        }),
        SmAutoConsolidate => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.self_model.auto_consolidate_every = v;
            }
        }),
        NotesAutoConsolidate => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.notes.auto_consolidate_every = v;
            }
        }),
        // History compression (spec §6.7). Same sanitization as the neighbours:
        // an empty or non-numeric entry keeps the previous value.
        CompactWords => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.compaction.summary_words = v;
            }
        }),
        CompactTail => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.compaction.tail_tokens = v;
            }
        }),
        CompactThreshold => int(|c, t| {
            // Clamped rather than rejected: above 100 the trigger could never
            // fire, which reads as "the feature is broken" rather than as a
            // refused entry. `0` legitimately means "manual only".
            if let Ok(v) = t.parse::<u32>() {
                c.compaction.threshold_pct = v.min(100) as u8;
            }
        }),
        CompactContext => int(|c, t| {
            // `0` is how the UI spells "resolve it yourself" — the field is a
            // number, and an empty entry already means "keep the previous value".
            if let Ok(v) = t.parse::<usize>() {
                c.compaction.context_tokens = (v > 0).then_some(v);
            }
        }),
        CompactPage => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.compaction.page_tokens = v;
            }
        }),
        IDicts => text(|c, t| {
            c.interface.selected_dictionaries = t
                .split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect();
        }),

        _ => return None,
    })
}

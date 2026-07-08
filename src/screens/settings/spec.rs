//! Экран настроек — таблица доступа к значениям config-полей (`field_spec`). Один
//! match по [`FieldId`] вместо прежних четырёх (`toggle_field`/`cycle_field`/
//! `apply_text`/`field_num_kind`) — поле целиком (как читать/писать/циклить/валидировать
//! значение) описано в одной строке. См. docs/refactoring-solid.md §5 (шаг 3.2).
//!
//! **Границы охвата.** Только config-поля (над `AppConfig`). Вне таблицы (прежний путь
//! в apply.rs): профильные (`PName`/`PSystem`/…, над `profiles[idx]`), параметры
//! семплинга `S(p)`/`IS(p)` (свой дескриптор `SamplingParam`), селекторы подсекций и
//! `PSelect` (навигация). Подпись/группа/описание/значение строки остаются в построителях
//! каталога (catalog.rs) — таблица заведует только доступом к значению.

use super::helpers::*;
use super::*;

/// Как читать/менять значение config-поля. `fn`-указатели (не замыкания) — `'static`,
/// без капчуринга; маршрутизация по режиму (external vs cloud) живёт **внутри** сеттера
/// (ему доступен весь `AppConfig`).
pub(super) enum Access {
    /// Тумблер: инвертирует значение на месте.
    Toggle(fn(&mut AppConfig)),
    /// Текст/число: парсит `trimmed` и присваивает (семантика парсинга — в самом сеттере).
    Text(fn(&mut AppConfig, &str)),
    /// Циклический выбор: шаг по направлению + список вариантов с индексом текущего.
    Choice {
        cycle: fn(&mut AppConfig, i32),
        options: fn(&AppConfig) -> (Vec<String>, usize),
    },
}

/// Спецификация доступа к значению config-поля.
pub(super) struct FieldSpec {
    pub(super) access: Access,
    /// Числовой вид для валидации редактора (`None` — текст/список/выбор).
    pub(super) num: Option<NumKind>,
}

/// Пусто → `None` (очистка `Option<String>`), иначе `Some(текст)`. Зеркало локального
/// `opt` из прежнего `apply_text`.
fn opt(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

/// **Единственный** match по `FieldId` для доступа к значению config-поля. `None` —
/// поле вне охвата (профильное/семплинг/навигация — прежний путь в apply.rs).
pub(super) fn field_spec(id: FieldId) -> Option<FieldSpec> {
    use FieldId::*;
    let spec = |access, num| FieldSpec { access, num };
    let toggle = |f: fn(&mut AppConfig)| spec(Access::Toggle(f), None);
    // Текстовое строковое поле (без числовой валидации).
    let text = |f: fn(&mut AppConfig, &str)| spec(Access::Text(f), None);
    // Целочисленное поле (валидируется как i64).
    let int = |f: fn(&mut AppConfig, &str)| spec(Access::Text(f), Some(NumKind::Int));
    let choice = |cycle, options| spec(Access::Choice { cycle, options }, None);

    Some(match id {
        // ---------- тумблеры ----------
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
        TFs => toggle(|c| c.tools.fs_enabled = !c.tools.fs_enabled),
        ICompat => toggle(|c| c.interface.terminal_compat = !c.interface.terminal_compat),
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
        ICopyThoughts => toggle(|c| c.copy.copy_thoughts = !c.copy.copy_thoughts),
        ICopyToolCalls => toggle(|c| c.copy.copy_tool_calls = !c.copy.copy_tool_calls),
        ICopyToolResults => toggle(|c| c.copy.copy_tool_results = !c.copy.copy_tool_results),

        // ---------- циклические выборы ----------
        XMode => choice(
            |c, dir| c.engine.mode = cycle_mode(c.engine.mode, dir),
            |c| index_menu(&SERVER_MODES, c.engine.mode, mode_label),
        ),
        EMode => choice(
            |c, dir| c.embed.mode = cycle_mode(c.embed.mode, dir),
            |c| index_menu(&SERVER_MODES, c.embed.mode, mode_label),
        ),
        IxMode => choice(
            |c, dir| c.impersonation_engine.mode = cycle_imp_mode(c.impersonation_engine.mode, dir),
            |c| index_menu(&IMP_MODES, c.impersonation_engine.mode, imp_mode_label),
        ),
        XFlashAttn => choice(
            |c, dir| c.engine.managed.flash_attn = c.engine.managed.flash_attn.cycle(dir),
            |c| flash_menu(c.engine.managed.flash_attn),
        ),
        IxFlashAttn => choice(
            |c, dir| {
                let m = &mut c.impersonation_engine.managed;
                m.flash_attn = m.flash_attn.cycle(dir)
            },
            |c| flash_menu(c.impersonation_engine.managed.flash_attn),
        ),
        XSpecType => choice(
            |c, dir| c.engine.managed.spec_type = c.engine.managed.spec_type.cycle(dir),
            |c| spec_menu(c.engine.managed.spec_type),
        ),
        IxSpecType => choice(
            |c, dir| {
                let m = &mut c.impersonation_engine.managed;
                m.spec_type = m.spec_type.cycle(dir)
            },
            |c| spec_menu(c.impersonation_engine.managed.spec_type),
        ),
        // Тема циклится в одну сторону — `dir` игнорируется (как в прежнем cycle_field).
        ITheme => choice(
            |c, _dir| c.interface.theme = cycle_theme(c.interface.theme),
            |c| index_menu(&THEMES, c.interface.theme, theme_label),
        ),

        // ---------- текст: движок ассистента ----------
        // URL/модель маршрутизируются по режиму (external → external.*, облако → cloud_mut()).
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
            if let Some(cl) = c.engine.cloud_mut() {
                cl.api_key_env = opt(t);
            }
        }),
        XBinary => text(|c, t| c.engine.managed.binary = opt(t)),
        XModel => text(|c, t| c.engine.managed.model_path = opt(t)),
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

        // ---------- текст: движок имперсонации ----------
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
            if let Some(cl) = c.impersonation_engine.cloud_mut() {
                cl.api_key_env = opt(t);
            }
        }),
        IxBinary => text(|c, t| c.impersonation_engine.managed.binary = opt(t)),
        IxModel => text(|c, t| c.impersonation_engine.managed.model_path = opt(t)),
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

        // ---------- текст: эмбеддинг-сервер ----------
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
            if let Some(cl) = c.embed.cloud_mut() {
                cl.api_key_env = opt(t);
            }
        }),
        EBinary => text(|c, t| c.embed.managed.binary = opt(t)),
        EModel => text(|c, t| c.embed.managed.model_path = opt(t)),
        EPort => int(|c, t| {
            if let Ok(p) = t.parse() {
                c.embed.managed.port = p;
            }
        }),

        // ---------- текст: инструменты / память / интерфейс ----------
        MaxToolRounds => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.max_tool_rounds = v;
            }
        }),
        TPythonPath => text(|c, t| c.tools.python_path = opt(t)),
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
        NotesAutoConsolidate => int(|c, t| {
            if let Ok(v) = t.parse() {
                c.notes.auto_consolidate_every = v;
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

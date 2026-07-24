//! Settings-screen tests (via handle_key/render). See mod.rs.

use super::helpers::*;
use super::*;
use crate::features::tools::default_tool_ids;
use crate::shared::config::{FlashAttn, PythonMode, SpecType};

fn screen() -> SettingsScreen {
    let mut p = Profile::new("Базовый", "Ты — ассистент.");
    p.enabled_tools = default_tool_ids();
    SettingsScreen::new(AppConfig::default(), vec![p], vec![])
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// Moves to the target section via Tab (robust to section order).
/// After the call, focus is in the menu (Tab resets it), no field is focused.
fn goto_section(s: &mut SettingsScreen, sec: Section) {
    for _ in 0..SECTIONS.len() {
        if s.section() == sec {
            return;
        }
        s.handle_key(key(KeyCode::Tab));
    }
    assert_eq!(s.section(), sec, "section {sec:?} not found");
}

/// A field's description by id: builds the fields of all sections/subsections of the
/// screen and looks up the row. The description lives on `FieldRow` (attached at build
/// time — see stage 3.1), not in a separate match; so it exists only on **visible**
/// rows (draft fields need to be made visible by setting spec_type=draft-*).
fn field_desc(s: &SettingsScreen, id: FieldId) -> Option<String> {
    let mut rows = Vec::new();
    for mt in [
        ModelTab::Assistant,
        ModelTab::Impersonation,
        ModelTab::Embeddings,
        ModelTab::Tts,
    ] {
        rows.extend(s.model_fields_for(mt));
    }
    for sub in [Subsection::Assistant, Subsection::Impersonation] {
        rows.extend(s.sampling_fields_for(sub));
        rows.extend(s.profile_fields_for(sub));
    }
    rows.extend(s.tool_fields());
    rows.extend(s.memory_fields());
    rows.extend(s.interface_fields());
    rows.into_iter()
        .find(|r| r.id == id)
        .and_then(|r| r.description)
}

/// Focuses the fields and steps down to field `id` (robust to groups/order).
/// Assumes focus is in the menu (as right after [`goto_section`]).
fn goto_field(s: &mut SettingsScreen, id: FieldId) {
    s.handle_key(key(KeyCode::Enter)); // focus on the fields
    for _ in 0..300 {
        if s.fields().get(s.field_idx).map(|f| f.id) == Some(id) {
            return;
        }
        s.handle_key(key(KeyCode::Down));
    }
    panic!("field {id:?} not found in section {:?}", s.section());
}

#[test]
fn esc_closes() {
    let mut s = screen();
    assert_eq!(s.handle_key(key(KeyCode::Esc)), Some(SettingsIntent::Close));
}

#[test]
fn interface_language_field_cycles_and_relocalizes() {
    use crate::shared::i18n::Lang;
    let mut s = screen();
    assert_eq!(s.config.interface.language, Lang::Ru);
    // The "Interface language" field is present and has a description.
    assert!(field_desc(&s, FieldId::ILanguage).is_some());
    // A cyclic edit (←→) changes the interface language.
    goto_section(&mut s, Section::Interface);
    goto_field(&mut s, FieldId::ILanguage);
    s.handle_key(key(KeyCode::Right));
    assert_eq!(s.config.interface.language, Lang::En);
    // The settings screen re-renders in the new language: the section title switches to English.
    assert_eq!(s.loc().t("ui.settings.section.interface"), "Interface");
}

#[test]
fn ctrl_q_and_f10_quit() {
    let mut s = screen();
    assert_eq!(s.handle_key(ctrl('q')), Some(SettingsIntent::Quit));
    assert_eq!(
        s.handle_key(key(KeyCode::F(10))),
        Some(SettingsIntent::Quit)
    );
    // Even with a field editor open — still quit (Ctrl+Q on top of the editor).
    s.editor = Some(Editor {
        field: FieldId::XBinary,
        input: InputBox::new(),
        multiline: false,
        error: None,
    });
    assert_eq!(s.handle_key(ctrl('q')), Some(SettingsIntent::Quit));
}

#[test]
fn tab_cycles_sections() {
    let mut s = screen();
    assert_eq!(s.section(), Section::Model);
    s.handle_key(key(KeyCode::Tab));
    assert_eq!(s.section(), Section::Sampling);
    s.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
    assert_eq!(s.section(), Section::Model);
}

#[test]
fn toggle_web_emits_save_with_flipped_value() {
    let mut s = screen();
    // Go to Tools, to the web-search toggle.
    goto_section(&mut s, Section::Tools);
    goto_field(&mut s, FieldId::TWeb);
    let intent = s.handle_key(key(KeyCode::Char(' ')));
    match intent {
        Some(SettingsIntent::SaveConfig(c)) => assert!(!c.tools.web_enabled),
        other => panic!("expected SaveConfig, got {other:?}"),
    }
}

#[test]
fn python_group_visibility_follows_mode() {
    // By default — Wasmer mode: "network" and "sandbox timeout" are visible, the path is hidden.
    let mut s = screen();
    goto_section(&mut s, Section::Tools);
    let ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
    assert!(ids.contains(&FieldId::TPythonMode));
    assert!(ids.contains(&FieldId::TPython));
    assert!(ids.contains(&FieldId::TPythonNet));
    assert!(ids.contains(&FieldId::TPythonWasmTimeout));
    assert!(ids.contains(&FieldId::TPythonWasmMemory));
    assert!(!ids.contains(&FieldId::TPythonPath));

    // Local mode: the interpreter path is visible, sandbox fields are hidden.
    let mut cfg = AppConfig::default();
    cfg.tools.python_mode = PythonMode::Local;
    let mut p = Profile::new("Базовый", "Ты — ассистент.");
    p.enabled_tools = default_tool_ids();
    let mut s = SettingsScreen::new(cfg, vec![p], vec![]);
    goto_section(&mut s, Section::Tools);
    let ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
    assert!(ids.contains(&FieldId::TPythonMode));
    assert!(ids.contains(&FieldId::TPythonPath));
    assert!(!ids.contains(&FieldId::TPythonNet));
    assert!(!ids.contains(&FieldId::TPythonWasmTimeout));
    assert!(!ids.contains(&FieldId::TPythonWasmMemory));
}

#[test]
fn python_mode_cycles_and_emits_save() {
    let mut s = screen();
    goto_section(&mut s, Section::Tools);
    goto_field(&mut s, FieldId::TPythonMode);
    // A right cycle on the 2-option mode switches Wasmer → Local.
    let intent = s.handle_key(key(KeyCode::Right));
    match intent {
        Some(SettingsIntent::SaveConfig(c)) => {
            assert_eq!(c.tools.python_mode, PythonMode::Local)
        }
        other => panic!("expected SaveConfig, got {other:?}"),
    }
}

#[test]
fn model_section_shows_active_subsection_server_chip() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let render_text = |s: &mut SettingsScreen| -> String {
        let mut term = Terminal::new(TestBackend::new(94, 12)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut s = screen();
    s.set_server_statuses(ServerStatuses {
        chat: ServerStatus::Ready,
        embed: ServerStatus::Connecting,
        impersonation: ServerStatus::NotConfigured,
    });
    // Assistant → the chat-server chip ("ready").
    let t = render_text(&mut s);
    assert!(t.contains("чат: готов"), "chat-server chip: {t}");
    // Switching the subsection changes the chip to the embeddings server ("connecting").
    s.model_sub = ModelTab::Embeddings;
    let t = render_text(&mut s);
    assert!(
        t.contains("эмбеддинги: подключение"),
        "embeddings chip: {t}"
    );
    assert!(!t.contains("чат: готов"), "the other chip isn't shown");
    // Other sections don't draw a chip.
    goto_section(&mut s, Section::Interface);
    let t = render_text(&mut s);
    assert!(!t.contains("готов") && !t.contains("подключение"));
}

#[test]
fn interface_has_terminal_compat_toggle() {
    let mut s = screen();
    // The field is in the "Interface" section, right after the theme.
    let rows = s.interface_fields();
    assert!(rows.iter().any(|r| r.id == FieldId::ICompat));
    // Toggling it saves the config with the flag raised…
    match s.toggle_field(FieldId::ICompat) {
        Some(SettingsIntent::SaveConfig(c)) => assert!(c.interface.terminal_compat),
        other => panic!("expected SaveConfig, got {other:?}"),
    }
    // …and the working-copy palette immediately switches to the compat glyph set.
    assert!(s.palette().compat);
    assert!(field_desc(&s, FieldId::ICompat).is_some());
}

#[test]
fn interface_has_table_separators_toggle() {
    let mut s = screen();
    // The field is in the "Interface" section (the "Appearance" group).
    let rows = s.interface_fields();
    assert!(rows.iter().any(|r| r.id == FieldId::ITableSeparators));
    // Off by default; toggling it saves the config with the flag raised.
    match s.toggle_field(FieldId::ITableSeparators) {
        Some(SettingsIntent::SaveConfig(c)) => assert!(c.interface.table_row_separators),
        other => panic!("expected SaveConfig, got {other:?}"),
    }
    assert!(field_desc(&s, FieldId::ITableSeparators).is_some());
}

#[test]
fn interface_has_mermaid_toggle() {
    let mut s = screen();
    // The field is in the "Interface" section (the "Appearance" group).
    let rows = s.interface_fields();
    assert!(rows.iter().any(|r| r.id == FieldId::IMermaid));
    // On by default; toggling it saves the config with the flag cleared.
    match s.toggle_field(FieldId::IMermaid) {
        Some(SettingsIntent::SaveConfig(c)) => assert!(!c.interface.render_mermaid),
        other => panic!("expected SaveConfig, got {other:?}"),
    }
    assert!(field_desc(&s, FieldId::IMermaid).is_some());
}

#[test]
fn cycle_mode_changes_server_mode() {
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter)); // focus on the fields (ModelSub)
    s.handle_key(key(KeyCode::Down)); // XMode (mode, Choice)
    let intent = s.handle_key(key(KeyCode::Right));
    match intent {
        Some(SettingsIntent::SaveConfig(c)) => assert_eq!(c.engine.mode, ServerMode::External),
        other => panic!("expected SaveConfig, got {other:?}"),
    }
}

#[test]
fn model_subsection_switches_to_impersonation_fields() {
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter)); // focus on the fields (ModelSub — tab strip)
    // → switches the subsection to "Impersonation" (no save).
    assert_eq!(s.handle_key(key(KeyCode::Right)), None);
    assert_eq!(s.model_sub, ModelTab::Impersonation);
    // The subsection's first field — the impersonation mode (3 values).
    s.handle_key(key(KeyCode::Down)); // IxMode
    // A shared → managed cycle.
    let intent = s.handle_key(key(KeyCode::Right));
    match intent {
        Some(SettingsIntent::SaveConfig(c)) => {
            assert_eq!(c.impersonation_engine.mode, ImpersonationMode::Managed)
        }
        other => panic!("expected SaveConfig, got {other:?}"),
    }
}

#[test]
fn model_subsection_third_tab_is_embeddings() {
    // Model has a third tab "Embeddings" (the server moved from "Tools");
    // cycling tabs right: Assistant → Impersonation → Embeddings.
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter)); // ModelSub (tab strip)
    s.handle_key(key(KeyCode::Right)); // → Impersonation
    s.handle_key(key(KeyCode::Right)); // → Embeddings
    assert_eq!(s.model_sub, ModelTab::Embeddings);
    let ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
    assert!(ids.contains(&FieldId::EMode));
    assert!(ids.contains(&FieldId::EBinary));
    // Embeddings are no longer in "Tools".
    goto_section(&mut s, Section::Tools);
    assert!(!s.fields().iter().any(|f| f.id == FieldId::EMode));
}

#[test]
fn model_subsection_fourth_tab_is_tts_with_mode_driven_fields() {
    // The fourth tab of the "Model" section — "Speech" (spec §11.9): cloud
    // mode shows model/voice/instructions/key, external — a URL instead of a key.
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter)); // ModelSub (tab strip)
    for _ in 0..3 {
        s.handle_key(key(KeyCode::Right));
    }
    assert_eq!(s.model_sub, ModelTab::Tts);
    let ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
    // Cloud (default — OpenAI): model/voice/instructions/key + behavior.
    assert!(ids.contains(&FieldId::TtsMode));
    assert!(ids.contains(&FieldId::TtsModelName));
    assert!(ids.contains(&FieldId::TtsVoice));
    assert!(ids.contains(&FieldId::TtsInstructions));
    assert!(ids.contains(&FieldId::TtsApiKey));
    assert!(ids.contains(&FieldId::TtsSpeakRoles));
    assert!(ids.contains(&FieldId::TtsStopOnSwitch));
    assert!(ids.contains(&FieldId::TtsStopOnGeneration));

    // Switch the mode to external: a URL appears, the key and instructions disappear
    // (only the OpenAI cloud supports them).
    s.config.tts.mode = crate::shared::config::TtsMode::External;
    let ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
    assert!(ids.contains(&FieldId::TtsUrl));
    assert!(
        !ids.contains(&FieldId::TtsApiKey),
        "external has no status key"
    );
    assert!(
        !ids.contains(&FieldId::TtsInstructions),
        "instructions — only the OpenAI cloud"
    );
}

#[test]
fn tts_toggles_and_mode_are_saved() {
    // Behavior toggles and the mode cycle edit the config and yield a save intent.
    // We set the tab as a field (`goto_field` expects focus in the menu, and running the
    // tab strip via keys would leave it on the fields).
    let mut s = screen();
    goto_section(&mut s, Section::Model);
    s.model_sub = ModelTab::Tts;
    goto_field(&mut s, FieldId::TtsMode);
    s.handle_key(key(KeyCode::Right));
    assert_eq!(
        s.config.tts.mode,
        crate::shared::config::TtsMode::Gemini,
        "the mode cycle follows TtsMode::ALL"
    );

    let mut s = screen();
    goto_section(&mut s, Section::Model);
    s.model_sub = ModelTab::Tts;
    goto_field(&mut s, FieldId::TtsSpeakRoles);
    let intent = s.handle_key(key(KeyCode::Char(' ')));
    assert!(matches!(intent, Some(SettingsIntent::SaveConfig(_))));
    assert!(
        s.config.tts.speak_roles,
        "the \"Speak roles\" toggle turned on"
    );
    // Speech fields have descriptions (the bottom panel + the search trap).
    assert!(field_desc(&s, FieldId::TtsMode).is_some());
    assert!(field_desc(&s, FieldId::TtsSpeakRoles).is_some());
}

#[test]
fn section_counts_sum_matches_search_index() {
    // Section parameter counters are tied to the selected modes and derived from
    // the same index as search: the sum of section counters is identically equal to
    // the number of fields in search (an invariant the user asked for). We check
    // this across different combinations of engine/provider modes.
    let mut s = screen();
    for &m in SERVER_MODES.iter() {
        s.config.engine.mode = m;
        s.config.embed.mode = m;
        let total: usize = s.section_counts().iter().sum();
        let search_total = s.build_search_index().len();
        assert_eq!(total, search_total, "mode {m:?}");
    }
}

#[test]
fn section_count_tracks_selected_mode() {
    // Unlike the previous union approach, the counter reflects the current mode: a
    // managed llama-server has noticeably more fields than a cloud provider (which
    // shows only model/key/base URL — ADR 0004).
    let mut s = screen();
    s.config.engine.mode = ServerMode::Managed;
    s.config.impersonation_engine.mode = ImpersonationMode::Shared;
    s.config.embed.mode = ServerMode::Managed;
    let managed = s.section_field_count(Section::Model);
    s.config.engine.mode = ServerMode::OpenAi;
    let cloud = s.section_field_count(Section::Model);
    assert!(
        cloud < managed,
        "cloud must show fewer fields: managed={managed} cloud={cloud}"
    );

    // The "Model" section's counter matches the number of its fields in the search
    // index (the sum of three tabs for their current modes, without subsection selectors).
    let model_from_search = s
        .build_search_index()
        .iter()
        .filter(|h| SECTIONS[h.section_idx] == Section::Model)
        .count();
    assert_eq!(cloud, model_from_search);
}

#[test]
fn section_field_count_excludes_subsection_selector() {
    // The subsection selector (tab strip) — a navigation element, not a parameter —
    // doesn't count. Sampling in local mode (default) shows all
    // parameters of each subsection with no selector row.
    let s = screen();
    assert_eq!(
        s.section_field_count(Section::Sampling),
        Subsection::ALL.len() * SAMPLING_PARAMS.len(),
        "the subsection selector leaked into the sampling count"
    );
}

#[test]
fn impersonation_profile_subsection_has_no_tools() {
    let mut s = screen();
    goto_section(&mut s, Section::Profiles);
    goto_field(&mut s, FieldId::ProfileSub); // the subsection tab strip (field 0)
    s.handle_key(key(KeyCode::Right)); // → Impersonation
    assert_eq!(s.profile_sub, Subsection::Impersonation);
    let fields = s.fields();
    assert!(
        !fields.iter().any(|f| matches!(f.id, FieldId::PTool(_))),
        "the impersonation subsection has no tool toggles"
    );
    assert!(fields.iter().any(|f| f.id == FieldId::PImpSystem));
}

#[test]
fn editing_model_commits_text() {
    let mut s = screen();
    goto_field(&mut s, FieldId::XModel); // the GGUF model (the "Model" group)
    s.handle_key(key(KeyCode::Enter)); // open the XModel editor
    assert!(s.editor.is_some());
    for c in "gemma.gguf".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    let intent = s.handle_key(key(KeyCode::Enter));
    match intent {
        Some(SettingsIntent::SaveConfig(c)) => {
            assert_eq!(c.engine.managed.model_path.as_deref(), Some("gemma.gguf"))
        }
        other => panic!("expected SaveConfig, got {other:?}"),
    }
    assert!(s.editor.is_none());
}

#[test]
fn editor_esc_discards() {
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter)); // ModelSub
    s.handle_key(key(KeyCode::Down)); // XMode
    s.handle_key(key(KeyCode::Down)); // XBinary (managed mode)
    s.handle_key(key(KeyCode::Enter)); // the XBinary editor
    s.handle_key(key(KeyCode::Char('x')));
    let intent = s.handle_key(key(KeyCode::Esc));
    assert_eq!(intent, None);
    assert!(s.editor.is_none());
    // the value didn't change
    assert!(s.config.engine.managed.binary.is_none());
}

#[test]
fn create_and_delete_profile_in_profiles_section() {
    let mut s = screen();
    goto_section(&mut s, Section::Profiles);
    assert_eq!(s.section(), Section::Profiles);
    let create = s.handle_key(ctrl('n'));
    assert!(matches!(create, Some(SettingsIntent::CreateProfile { .. })));
    let id = s.profiles[0].id;
    let del = s.handle_key(ctrl('d'));
    assert_eq!(del, Some(SettingsIntent::DeleteProfile(id)));
}

#[test]
fn toggling_profile_tool_emits_save_profile() {
    let mut s = screen();
    goto_section(&mut s, Section::Profiles);
    // The catalog is ordered by id (not the default list), so take the index of the
    // first tool ACTUALLY enabled in the profile — its toggle must turn off.
    let catalog = crate::features::tools::tool_catalog();
    let enabled = s.profiles[0].enabled_tools.clone();
    let idx = catalog
        .iter()
        .position(|i| enabled.contains(&i.id))
        .expect("the profile has an enabled tool from the catalog");
    goto_field(&mut s, FieldId::PTool(idx));
    let before = s.profiles[0].enabled_tools.len();
    let intent = s.handle_key(key(KeyCode::Char(' ')));
    match intent {
        Some(SettingsIntent::SaveProfile { edit, .. }) => {
            let tools = edit.enabled_tools.unwrap();
            assert_eq!(tools.len(), before - 1, "the enabled tool turned off");
        }
        other => panic!("expected SaveProfile, got {other:?}"),
    }
}

#[test]
fn profile_select_cycles() {
    let mut p2 = Profile::new("Второй", "sys2");
    p2.enabled_tools = default_tool_ids();
    let mut s = SettingsScreen::new(
        AppConfig::default(),
        {
            let mut p1 = Profile::new("Первый", "sys1");
            p1.enabled_tools = default_tool_ids();
            vec![p1, p2]
        },
        vec![],
    );
    goto_section(&mut s, Section::Profiles);
    goto_field(&mut s, FieldId::PSelect); // the profile selector (after the tab strip)
    assert_eq!(s.profile_idx, 0);
    s.handle_key(key(KeyCode::Right));
    assert_eq!(s.profile_idx, 1);
}

#[test]
fn samplers_list_round_trip() {
    // Sampler order: split on ";", joined back, empty → None.
    let v = parse_list("penalties;dry; temperature ", ';').unwrap();
    assert_eq!(v, vec!["penalties", "dry", "temperature"]);
    assert_eq!(join_list(Some(&v), ';'), "penalties;dry;temperature");
    assert_eq!(parse_list("", ';'), None);
    assert_eq!(parse_list("   ;  ", ';'), None);
    assert_eq!(join_list(None, ';'), "—");
}

#[test]
fn dry_breakers_decode_and_encode_escapes() {
    // Escapes \n \t are decoded on input and encoded back on display.
    let v = parse_breakers(r#"\n, :, ", *"#).unwrap();
    assert_eq!(v, vec!["\n", ":", "\"", "*"]);
    assert_eq!(join_breakers(Some(&v)), r#"\n,:,",*"#);
    assert_eq!(parse_breakers(""), None);
    assert_eq!(join_breakers(None), "—");
}

#[test]
fn system_message_editor_is_multiline_and_keeps_newlines() {
    let mut s = screen();
    goto_section(&mut s, Section::Profiles);
    goto_field(&mut s, FieldId::PSystem);
    s.handle_key(key(KeyCode::Enter)); // open the editor
    let editor = s.editor.as_ref().expect("the editor is open");
    assert!(
        editor.multiline,
        "the system message is edited as multiline"
    );
    // Shift+Enter inserts a line break, not a commit.
    s.handle_key(key(KeyCode::Char('A')));
    s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    s.handle_key(key(KeyCode::Char('B')));
    assert!(s.editor.is_some(), "Shift+Enter doesn't close the editor");
    let intent = s.handle_key(key(KeyCode::Enter)); // commit
    match intent {
        Some(SettingsIntent::SaveProfile { edit, .. }) => {
            assert_eq!(edit.system_message.unwrap(), "Ты — ассистент.A\nB");
        }
        other => panic!("expected SaveProfile, got {other:?}"),
    }
}

#[test]
fn alt_enter_also_inserts_newline_in_multiline_editor() {
    // A fallback line break for terminals without the kitty protocol (Shift+Enter
    // is indistinguishable from Enter there). See item 11 of the InputBox audit.
    let mut s = screen();
    goto_section(&mut s, Section::Profiles);
    goto_field(&mut s, FieldId::PSystem);
    s.handle_key(key(KeyCode::Enter)); // open the editor (multiline)
    s.handle_key(key(KeyCode::Char('A')));
    s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    s.handle_key(key(KeyCode::Char('B')));
    assert!(s.editor.is_some(), "Alt+Enter doesn't close the editor");
    let intent = s.handle_key(key(KeyCode::Enter)); // commit
    match intent {
        Some(SettingsIntent::SaveProfile { edit, .. }) => {
            assert_eq!(edit.system_message.unwrap(), "Ты — ассистент.A\nB");
        }
        other => panic!("expected SaveProfile, got {other:?}"),
    }
}

#[test]
fn ctrl_k_clears_and_ctrl_z_restores_multiline_editor() {
    let mut s = screen();
    goto_section(&mut s, Section::Profiles);
    goto_field(&mut s, FieldId::PSystem);
    s.handle_key(key(KeyCode::Enter)); // open the editor (multiline)
    s.handle_key(key(KeyCode::Char('A')));
    s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    s.handle_key(key(KeyCode::Char('B')));
    let ctrl_k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
    let ctrl_z = KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL);
    s.handle_key(ctrl_k); // clear
    assert_eq!(s.editor.as_ref().unwrap().input.text(), "");
    s.handle_key(ctrl_z); // restore what was deleted (the shared undo model, §C)
    assert_eq!(
        s.editor.as_ref().unwrap().input.text(),
        "Ты — ассистент.A\nB"
    );
    assert!(s.editor.is_some(), "Ctrl+K/Ctrl+Z don't close the editor");
}

#[test]
fn greeting_editor_is_multiline_and_keeps_newlines() {
    let mut s = screen();
    goto_section(&mut s, Section::Profiles);
    goto_field(&mut s, FieldId::PGreeting);
    s.handle_key(key(KeyCode::Enter)); // open the editor
    let editor = s.editor.as_ref().expect("the editor is open");
    assert!(editor.multiline, "the greeting is edited as multiline");
    // Shift+Enter inserts a line break, not a commit.
    s.handle_key(key(KeyCode::Char('A')));
    s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    s.handle_key(key(KeyCode::Char('B')));
    assert!(s.editor.is_some(), "Shift+Enter doesn't close the editor");
    let intent = s.handle_key(key(KeyCode::Enter)); // commit
    match intent {
        Some(SettingsIntent::SaveProfile { edit, .. }) => {
            assert_eq!(edit.greeting.unwrap().as_deref(), Some("A\nB"));
        }
        other => panic!("expected SaveProfile, got {other:?}"),
    }
}

#[test]
fn other_fields_edit_single_line() {
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter)); // fields (ModelSub)
    s.handle_key(key(KeyCode::Down)); // XMode
    s.handle_key(key(KeyCode::Down)); // XBinary (text, managed mode)
    s.handle_key(key(KeyCode::Enter)); // the editor
    let editor = s.editor.as_ref().expect("the editor is open");
    assert!(
        !editor.multiline,
        "a regular field is edited as single-line"
    );
}

#[test]
fn cloud_mode_reveals_model_and_key_fields() {
    // Switching the assistant's mode to cloud (openai) shows the
    // "Model" and "API key (env)" fields and hides llama-server parameters.
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter)); // fields (ModelSub)
    s.handle_key(key(KeyCode::Down)); // XMode
    // managed → external → openai (cycle right twice).
    s.handle_key(key(KeyCode::Right));
    s.handle_key(key(KeyCode::Right));
    assert_eq!(s.config.engine.mode, ServerMode::OpenAi);
    let ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
    assert!(ids.contains(&FieldId::XModelName));
    assert!(ids.contains(&FieldId::XApiKeyEnv));
    // The local server's parameters are hidden in cloud mode.
    assert!(!ids.contains(&FieldId::XNgl));
    assert!(!ids.contains(&FieldId::XBinary));
}

#[test]
fn render_does_not_panic() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut s = screen();
    for (w, h) in [(80u16, 24u16), (40, 12)] {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
    }
}

#[test]
fn hotkeys_render_below_panel_and_wrap_when_narrow() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let rows = |w: u16, h: u16| -> Vec<String> {
        let mut s = screen();
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect()
    };
    // Hotkey rows (outside the border, right-aligned) start with a space at column 0,
    // whereas panel rows carry the border character. Count the trailing hotkey rows.
    let status_rows = |lines: &[String]| -> usize {
        lines
            .iter()
            .rev()
            .take_while(|l| l.starts_with(' '))
            .count()
    };
    // Wide — hotkeys fit on one row below the panel, right-aligned
    // (ending in "quit"); the row above them is the panel's bottom border (not a space).
    let wide = rows(120, 24);
    assert_eq!(status_rows(&wide), 1, "wide — one hotkey row");
    let last = wide.last().unwrap();
    assert!(last.contains("Tab") && last.contains("выход"));
    assert!(
        last.trim_end().ends_with("выход"),
        "right-aligned: {last:?}"
    );
    assert!(
        !wide[wide.len() - 2].starts_with(' '),
        "above the hotkeys — the panel's bottom border: {:?}",
        wide[wide.len() - 2]
    );
    // Narrow — hotkeys wrap onto several rows (the status area's height > 1).
    let narrow = rows(46, 24);
    assert!(
        status_rows(&narrow) > 1,
        "expected hotkeys to wrap: {}",
        status_rows(&narrow)
    );
}

#[test]
fn render_with_editor_does_not_panic() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter));
    s.handle_key(key(KeyCode::Down));
    s.handle_key(key(KeyCode::Down));
    s.handle_key(key(KeyCode::Enter)); // the editor
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
}

#[test]
fn flag_fields_have_descriptions() {
    // Draft fields are visible only for spec_type=draft-* — turn it on to check their description.
    let mut s = screen();
    s.config.engine.managed.spec_type = SpecType::DraftMtp;
    // -ngl, --jinja, and --no-mmap carry a human-readable hint; a regular field doesn't.
    assert!(field_desc(&s, FieldId::XNgl).is_some());
    assert!(field_desc(&s, FieldId::XJinja).is_some());
    assert!(field_desc(&s, FieldId::XNoMmap).is_some());
    assert!(field_desc(&s, FieldId::XPort).is_none());
    // The new FlashAttention/speculative-decoding fields are also described.
    assert!(field_desc(&s, FieldId::XFlashAttn).is_some());
    assert!(field_desc(&s, FieldId::XSpecType).is_some());
    assert!(field_desc(&s, FieldId::XDraftModel).is_some());
}

#[test]
fn managed_mode_shows_flash_attn_and_spec_type() {
    // In managed mode (default), FlashAttention and --spec-type are visible; draft
    // fields are hidden until the type is draft-*.
    let s = screen();
    let ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
    assert!(ids.contains(&FieldId::XFlashAttn));
    assert!(ids.contains(&FieldId::XSpecType));
    assert!(!ids.contains(&FieldId::XDraftModel));
}

#[test]
fn cycling_spec_type_to_draft_reveals_draft_fields() {
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter)); // focus on the fields (XMode)
    while s.fields().get(s.field_idx).map(|f| f.id) != Some(FieldId::XSpecType) {
        s.handle_key(key(KeyCode::Down));
    }
    // none → draft-simple → draft-eagle3 → draft-mtp (three steps right).
    s.handle_key(key(KeyCode::Right));
    s.handle_key(key(KeyCode::Right));
    s.handle_key(key(KeyCode::Right));
    assert_eq!(s.config.engine.managed.spec_type, SpecType::DraftMtp);
    let ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
    assert!(ids.contains(&FieldId::XDraftModel));
    assert!(ids.contains(&FieldId::XDraftNMax));
}

#[test]
fn cycling_flash_attn_changes_value_and_saves() {
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter)); // focus on the fields (XMode)
    while s.fields().get(s.field_idx).map(|f| f.id) != Some(FieldId::XFlashAttn) {
        s.handle_key(key(KeyCode::Down));
    }
    assert_eq!(s.config.engine.managed.flash_attn, FlashAttn::Auto);
    let intent = s.handle_key(key(KeyCode::Right));
    assert_eq!(s.config.engine.managed.flash_attn, FlashAttn::On);
    assert!(matches!(intent, Some(SettingsIntent::SaveConfig(_))));
}

#[test]
fn render_with_focused_description_does_not_panic() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter)); // focus on the fields (XMode)
    // Step down to the --no-mmap toggle (has a description hint below).
    while s.fields().get(s.field_idx).map(|f| f.id) != Some(FieldId::XNoMmap) {
        s.handle_key(key(KeyCode::Down));
    }
    for (w, h) in [(80u16, 24u16), (40, 12)] {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
    }
}

#[test]
fn fields_scrollbar_appears_only_on_overflow() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    // The "█" thumb on the screen's right border — only when there are more fields than the height.
    let has_thumb = |term: &Terminal<TestBackend>| {
        let buf = term.backend().buffer();
        let x = buf.area.right() - 1; // the settings-panel border column
        (buf.area.top()..buf.area.bottom()).any(|y| buf[(x, y)].symbol() == "█")
    };
    let mut s = screen();
    goto_section(&mut s, Section::Sampling); // fields+headers definitely exceed the height
    let mut term = Terminal::new(TestBackend::new(80, 14)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
    assert!(has_thumb(&term), "an overflowing section — has the thumb");
    // In a tall window all fields are visible — no thumb.
    let mut term = Terminal::new(TestBackend::new(80, 50)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
    assert!(!has_thumb(&term), "all fields visible — no thumb");
}

#[test]
fn editing_new_sampling_field_commits() {
    let mut s = screen();
    goto_section(&mut s, Section::Sampling);
    goto_field(&mut s, FieldId::S(SamplingParam::MinP));
    s.handle_key(key(KeyCode::Enter)); // open the min_p editor
    for c in "0.03".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    let intent = s.handle_key(key(KeyCode::Enter)); // commit
    match intent {
        Some(SettingsIntent::SaveConfig(c)) => {
            assert_eq!(c.default_sampling.min_p, Some(0.03))
        }
        other => panic!("expected SaveConfig, got {other:?}"),
    }
}

#[test]
fn cloud_hides_unsupported_sampling_params() {
    let mut s = screen();
    goto_section(&mut s, Section::Sampling);
    let has =
        |s: &SettingsScreen, p: SamplingParam| s.fields().iter().any(|f| f.id == FieldId::S(p));
    // Locally (managed by default) — all parameters are visible.
    assert!(has(&s, SamplingParam::TopK));
    assert!(has(&s, SamplingParam::Thinking));
    // Cloud (Gemini, native): llama.cpp extensions (min_p) are hidden, while base +
    // top_k + reasoning are visible.
    s.config.engine.mode = ServerMode::Gemini;
    assert!(!has(&s, SamplingParam::MinP));
    assert!(has(&s, SamplingParam::TopK));
    assert!(has(&s, SamplingParam::Thinking));
    assert!(has(&s, SamplingParam::Reasoning));
    assert!(has(&s, SamplingParam::Temp));
    assert!(has(&s, SamplingParam::TopP));
    assert!(has(&s, SamplingParam::MaxTokens));
    // OpenAI (Responses): no temperature/top_p/penalties/seed; has reasoning and
    // verbosity (Responses-specific).
    s.config.engine.mode = ServerMode::OpenAi;
    assert!(!has(&s, SamplingParam::Temp));
    assert!(!has(&s, SamplingParam::TopP));
    assert!(!has(&s, SamplingParam::FreqPen));
    assert!(has(&s, SamplingParam::MaxTokens));
    assert!(has(&s, SamplingParam::Thinking));
    assert!(has(&s, SamplingParam::Reasoning));
    assert!(has(&s, SamplingParam::Verbosity));
    // Verbosity — only for OpenAI: hidden for Gemini/Claude.
    s.config.engine.mode = ServerMode::Gemini;
    assert!(!has(&s, SamplingParam::Verbosity));
    // Claude 4.x "locked in" sampling: only max_tokens is visible.
    s.config.engine.mode = ServerMode::Claude;
    assert!(!has(&s, SamplingParam::Temp));
    assert!(!has(&s, SamplingParam::TopP));
    assert!(!has(&s, SamplingParam::FreqPen));
    assert!(has(&s, SamplingParam::MaxTokens));
}

#[test]
fn impersonation_shared_inherits_assistant_cloud_filter() {
    let mut s = screen();
    // The assistant is in cloud, impersonation is shared → its sampling is filtered as cloud.
    s.config.engine.mode = ServerMode::OpenAi;
    assert_eq!(
        s.config.impersonation_engine.mode,
        ImpersonationMode::Shared
    );
    goto_section(&mut s, Section::Sampling);
    s.handle_key(key(KeyCode::Enter)); // focus (SamplingSub)
    s.handle_key(key(KeyCode::Right)); // → the Impersonation subsection
    assert_eq!(s.sampling_sub, Subsection::Impersonation);
    let has_topk = s
        .fields()
        .iter()
        .any(|f| f.id == FieldId::IS(SamplingParam::TopK));
    assert!(
        !has_topk,
        "the assistant's cloud filters shared impersonation too"
    );
    // A local impersonation engine (managed) shows all parameters, even if the assistant is in the cloud.
    s.config.impersonation_engine.mode = ImpersonationMode::Managed;
    let has_topk = s
        .fields()
        .iter()
        .any(|f| f.id == FieldId::IS(SamplingParam::TopK));
    assert!(has_topk);
}

#[test]
fn subsection_renders_as_tab_strip_not_list_row() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter)); // focus on the fields (Model)
    let mut term = Terminal::new(TestBackend::new(90, 24)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
    let buf = term.backend().buffer();
    let text: String = (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    // All three tabs of the model subsection are visible as a tab strip.
    assert!(text.contains("Ассистент"));
    assert!(text.contains("Эмбеддинги"));
    // The pseudo-field "Subsection" is no longer drawn as a list row.
    assert!(
        !text.contains("Подсекция"),
        "the subsection selector should be a tab strip, not a list row"
    );
}

#[test]
fn profile_tools_are_grouped_with_descriptions() {
    // Every tool toggle is marked with a semantic group (from meta) and carries a
    // short inline description. Groups — from the known set (`group_titles`).
    let mut s = screen();
    goto_section(&mut s, Section::Profiles);
    let fields = s.profile_fields();
    let tool_rows: Vec<&FieldRow> = fields
        .iter()
        .filter(|r| matches!(r.id, FieldId::PTool(_)))
        .collect();
    assert!(!tool_rows.is_empty());
    for r in &tool_rows {
        assert!(
            crate::features::tools::meta::group_titles().contains(&r.group),
            "a tool outside the known group set: {}",
            r.label
        );
        assert!(r.hint.is_some(), "no inline description for {}", r.label);
    }
    // Tools of one group run consecutively (the header isn't repeated).
    let groups: Vec<&str> = tool_rows.iter().map(|r| r.group).collect();
    let mut seen = std::collections::HashSet::new();
    let mut prev = "";
    for g in groups {
        if g != prev {
            assert!(seen.insert(g), "group {g} isn't contiguous");
            prev = g;
        }
    }
}

#[test]
fn globally_disabled_tool_is_marked_gated() {
    // python is disabled globally but enabled in the profile → the row is marked gated
    // (warn + a "disabled globally" hint); web is enabled → a regular description.
    let mut s = screen();
    s.config.tools.python_enabled = false;
    s.config.tools.web_enabled = true;
    goto_section(&mut s, Section::Profiles);
    let fields = s.profile_fields();
    let idx_of = |name: &str| -> usize {
        crate::features::tools::all_tool_ids()
            .iter()
            .position(|t| t == name)
            .unwrap()
    };
    let find = |id: FieldId| fields.iter().find(|r| r.id == id).unwrap();
    let py = find(FieldId::PTool(idx_of("python_exec")));
    assert!(py.warn, "python_exec disabled globally — gated");
    assert!(py.hint.unwrap().contains("глобально"));
    let web = find(FieldId::PTool(idx_of("web_search")));
    assert!(!web.warn, "web enabled globally — not gated");
    assert_eq!(web.hint, Some("поиск в интернете"));
}

#[test]
fn mcp_tools_extend_profile_toggles_with_honest_gate() {
    use crate::features::tools::meta::{ToolGate, ToolGroup, ToolInfo};
    // The dynamic MCP catalog (a snapshot from Settings) is appended to the static one:
    // a toggle in the "Plugins (MCP)" group, enabled while the master gate is off —
    // is marked with an honest gate; toggling writes the id into the profile.
    let mut s = screen();
    s.config.mcp.enabled = false;
    s.profiles[0]
        .enabled_tools
        .push("mcp__fs__read_text_file".into());
    s.set_mcp(crate::features::tools::mcp::McpSnapshot {
        tools: vec![ToolInfo {
            id: "mcp__fs__read_text_file".into(),
            group: ToolGroup::Plugins,
            label: "read_text_file",
            gate: Some(ToolGate::Mcp),
            enabled_by_default: false,
            description: Some("Read the complete contents of a file".into()),
        }],
        servers: Vec::new(),
    });
    goto_section(&mut s, Section::Profiles);
    let fields = s.profile_fields();
    let idx = s
        .tool_catalog()
        .iter()
        .position(|i| i.id == "mcp__fs__read_text_file")
        .unwrap();
    let row = fields.iter().find(|r| r.id == FieldId::PTool(idx)).unwrap();
    assert!(matches!(row.kind, FieldKind::Toggle(true)));
    assert!(row.warn, "MCP disabled globally — an honest gate");
    assert!(row.hint.unwrap().contains("MCP"));
    // The master gate is on → a regular hint (the tool's name) + the FULL
    // server description in the bottom panel (a tool-poisoning antidote, spec §9.6).
    s.config.mcp.enabled = true;
    let fields = s.profile_fields();
    let row = fields.iter().find(|r| r.id == FieldId::PTool(idx)).unwrap();
    assert!(!row.warn);
    assert_eq!(row.hint, Some("read_text_file"));
    assert_eq!(
        row.description.as_deref(),
        Some("Read the complete contents of a file")
    );
    // Toggling it removes the id from the profile (and back).
    s.toggle_profile_tool(idx).unwrap();
    assert!(
        !s.profiles[0]
            .enabled_tools
            .iter()
            .any(|t| t == "mcp__fs__read_text_file")
    );
    s.toggle_profile_tool(idx).unwrap();
    assert!(
        s.profiles[0]
            .enabled_tools
            .iter()
            .any(|t| t == "mcp__fs__read_text_file")
    );
}

#[test]
fn mcp_server_rows_show_status_and_confirm_changed_catalog() {
    use crate::features::tools::mcp::{McpServerSnapshot, McpSnapshot};
    use crate::shared::server::ServerStatus;
    // Server rows in the "Plugins (MCP)" group: a ready one shows the tool
    // count; a server with a changed catalog — warn + Enter confirms.
    let mut s = screen();
    s.config.mcp.enabled = true;
    s.set_mcp(McpSnapshot {
        tools: Vec::new(),
        servers: vec![
            McpServerSnapshot {
                id: "fs".into(),
                status: ServerStatus::Ready,
                tool_count: 14,
                pending_catalog: false,
            },
            McpServerSnapshot {
                id: "github".into(),
                status: ServerStatus::Disconnected("каталог изменился".into()),
                tool_count: 0,
                pending_catalog: true,
            },
        ],
    });
    goto_section(&mut s, Section::Tools);
    let fields = s.tool_fields();
    let fs = fields
        .iter()
        .find(|r| r.id == FieldId::TMcpServer(0))
        .unwrap();
    assert!(matches!(&fs.kind, FieldKind::Text(v) if v.contains("14")));
    assert!(!fs.warn);
    let gh = fields
        .iter()
        .find(|r| r.id == FieldId::TMcpServer(1))
        .unwrap();
    assert!(gh.warn, "a changed catalog — a warning");
    assert!(gh.hint.unwrap().contains("Enter"));
    // Enter on a ready server — a no-op; on a changed one — the confirm intent.
    assert!(s.confirm_mcp_catalog(0).is_none());
    assert_eq!(
        s.confirm_mcp_catalog(1),
        Some(SettingsIntent::ConfirmMcpCatalog("github".into()))
    );
    // Enter via handle_key reaches confirmation.
    goto_field(&mut s, FieldId::TMcpServer(1));
    assert_eq!(
        s.handle_key(key(KeyCode::Enter)),
        Some(SettingsIntent::ConfirmMcpCatalog("github".into()))
    );
}

#[test]
fn mcp_master_toggle_lives_in_tools_section() {
    // The "MCP servers" toggle in the "Tools" section: toggling it saves the config.
    let mut s = screen();
    assert!(!s.config.mcp.enabled);
    goto_section(&mut s, Section::Tools);
    goto_field(&mut s, FieldId::TMcpEnabled);
    s.handle_key(key(KeyCode::Char(' ')));
    assert!(s.config.mcp.enabled, "Space enables the MCP master gate");
    assert!(field_desc(&s, FieldId::TMcpEnabled).is_some());
}

#[test]
fn group_header_shows_toggle_count() {
    // A group header with ≥2 toggles carries an "on/total" counter; a single one doesn't.
    let palette = Palette::default();
    let line = header_line("Веб-поиск", Some((1, 2)), 60, &palette);
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("1/2"), "no counter: {text:?}");
    let plain = header_line("Сервер", None, 60, &palette);
    let ptext: String = plain.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        !ptext.contains('/'),
        "a group with no counter shouldn't have one"
    );
}

#[test]
fn group_headers_same_length_with_and_without_count() {
    // Group headers with and without a counter reach the same column —
    // previously the counter made the line 1 character shorter (spurious +1).
    let palette = Palette::default();
    let line_width = |line: Line<'static>| -> usize {
        line.spans
            .iter()
            .map(|s| label_width(s.content.as_ref()))
            .sum()
    };
    for w in [40usize, 60, 74, 100] {
        let counted = line_width(header_line("Интроспекция", Some((5, 5)), w, &palette));
        let plain = line_width(header_line("Персона", None, w, &palette));
        assert_eq!(
            counted, plain,
            "width {w}: with the counter {counted} ≠ without {plain}"
        );
    }
}

#[test]
fn choice_popup_opens_and_applies_selection() {
    // Enter on a Choice field opens the option-list popup; ↓ + Enter applies the choice.
    let mut s = screen();
    goto_field(&mut s, FieldId::XSpecType);
    s.handle_key(key(KeyCode::Enter));
    assert!(s.choice.is_some(), "Enter on Choice opens the popup");
    assert_eq!(s.config.engine.managed.spec_type, SpecType::None);
    s.handle_key(key(KeyCode::Down)); // none → draft-simple
    let intent = s.handle_key(key(KeyCode::Enter));
    assert!(s.choice.is_none(), "Enter applies and closes the popup");
    assert_eq!(s.config.engine.managed.spec_type, SpecType::DraftSimple);
    assert!(matches!(intent, Some(SettingsIntent::SaveConfig(_))));
}

#[test]
fn choice_popup_esc_cancels() {
    let mut s = screen();
    goto_field(&mut s, FieldId::XMode);
    s.handle_key(key(KeyCode::Enter));
    assert!(s.choice.is_some());
    s.handle_key(key(KeyCode::Down));
    s.handle_key(key(KeyCode::Esc));
    assert!(s.choice.is_none());
    assert_eq!(
        s.config.engine.mode,
        ServerMode::Managed,
        "Esc doesn't change the value"
    );
}

#[test]
fn invalid_number_keeps_editor_open() {
    // A non-number in a numeric field leaves the editor open with an error; an edit clears it.
    let mut s = screen();
    goto_field(&mut s, FieldId::XNgl);
    s.handle_key(key(KeyCode::Enter)); // the editor
    for c in "abc".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    let intent = s.handle_key(key(KeyCode::Enter)); // validation: don't close
    assert_eq!(intent, None);
    assert!(s.editor.is_some(), "invalid input doesn't close the editor");
    assert!(s.editor.as_ref().unwrap().error.is_some());
    // An edit clears the error and a valid value commits.
    s.handle_key(ctrl('k')); // clear
    for c in "42".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    let intent = s.handle_key(key(KeyCode::Enter));
    assert!(s.editor.is_none());
    match intent {
        Some(SettingsIntent::SaveConfig(c)) => assert_eq!(c.engine.managed.gpu_layers, 42),
        other => panic!("expected SaveConfig, got {other:?}"),
    }
}

#[test]
fn field_validation_error_classifies_numbers() {
    assert!(field_validation_error(FieldId::XNgl, "abc").is_some());
    assert!(field_validation_error(FieldId::XNgl, "12").is_none());
    assert!(field_validation_error(FieldId::XNgl, "").is_none()); // empty is valid
    assert!(field_validation_error(FieldId::S(SamplingParam::Temp), "x").is_some());
    assert!(field_validation_error(FieldId::S(SamplingParam::Temp), "0.7").is_none());
    // Text/list fields aren't validated as numbers.
    assert!(field_validation_error(FieldId::XBinary, "любой текст").is_none());
    assert!(field_validation_error(FieldId::S(SamplingParam::Samplers), "top_k;top_p").is_none());
}

#[test]
fn del_resets_field_to_default() {
    let mut s = screen();
    s.config.engine.managed.gpu_layers = 40; // not the default
    let default_ngl = AppConfig::default().engine.managed.gpu_layers;
    goto_field(&mut s, FieldId::XNgl);
    let intent = s.handle_key(key(KeyCode::Delete));
    match intent {
        Some(SettingsIntent::SaveConfig(c)) => {
            assert_eq!(c.engine.managed.gpu_layers, default_ngl)
        }
        other => panic!("expected SaveConfig, got {other:?}"),
    }
}

#[test]
fn del_on_default_field_is_noop() {
    // The field is already at the default → Del does nothing; Del doesn't touch profile fields.
    let mut s = screen();
    goto_field(&mut s, FieldId::XNgl);
    assert_eq!(s.handle_key(key(KeyCode::Delete)), None);
}

#[test]
fn modified_field_shows_marker() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let render_text = |s: &mut SettingsScreen| -> String {
        let mut term = Terminal::new(TestBackend::new(92, 24)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
            .map(|(x, y)| buf[(x, y)].symbol().to_string())
            .collect()
    };
    // The default config — no markers.
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter));
    assert!(
        !render_text(&mut s).contains('•'),
        "no markers at the default"
    );
    // A modified field — the marker appears.
    s.config.engine.managed.gpu_layers = 40;
    assert!(
        render_text(&mut s).contains('•'),
        "a modified field is marked with •"
    );
}

#[test]
fn search_filters_and_jumps_to_field() {
    let mut s = screen();
    // `/` opens search; typing filters by a unique word.
    s.handle_key(key(KeyCode::Char('/')));
    assert!(s.search.is_some(), "`/` opens the search overlay");
    for c in "приветствие".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    {
        let st = s.search.as_ref().unwrap();
        assert!(!st.results.is_empty());
        assert!(
            st.results
                .iter()
                .all(|&i| st.all[i].haystack.contains("приветствие")),
            "all results contain the query"
        );
    }
    // Enter — jump to the field (section/focus/index), the overlay closes.
    s.handle_key(key(KeyCode::Enter));
    assert!(s.search.is_none());
    assert_eq!(s.section(), Section::Profiles);
    assert!(s.focus == Focus::Fields);
    assert_eq!(
        s.fields().get(s.field_idx).map(|f| f.id),
        Some(FieldId::PGreeting)
    );
}

#[test]
fn search_opens_on_cyrillic_slash_key() {
    // Under a Russian layout the physical `/` key sends `.` — search still
    // must open.
    let mut s = screen();
    s.handle_key(key(KeyCode::Char('.')));
    assert!(s.search.is_some(), "`.` (Russian layout) opens search");
}

#[test]
fn search_jump_switches_subsection() {
    // Jumping into a field of an inactive subsection switches it (Model → Embeddings).
    let mut s = screen();
    assert_eq!(s.model_sub, ModelTab::Assistant);
    s.handle_key(key(KeyCode::Char('/')));
    for c in "эмбеддинги порт".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    assert!(!s.search.as_ref().unwrap().results.is_empty());
    s.handle_key(key(KeyCode::Enter));
    assert_eq!(s.section(), Section::Model);
    assert_eq!(s.model_sub, ModelTab::Embeddings);
    assert_eq!(
        s.fields().get(s.field_idx).map(|f| f.id),
        Some(FieldId::EPort)
    );
}

#[test]
fn search_esc_cancels_without_jump() {
    let mut s = screen();
    let before = (s.section_idx, s.field_idx);
    s.handle_key(key(KeyCode::Char('/')));
    for c in "порт".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    s.handle_key(key(KeyCode::Esc));
    assert!(s.search.is_none());
    assert_eq!(
        (s.section_idx, s.field_idx),
        before,
        "Esc doesn't move navigation"
    );
}

#[test]
fn search_index_covers_all_subsections() {
    // The search index contains fields of all subsections (e.g. both the managed
    // server and the assistant's cloud model are reachable via search under the current mode).
    let s = screen();
    let idx = s.build_search_index();
    assert!(
        idx.len() > 100,
        "the index covers all sections: {}",
        idx.len()
    );
    // The impersonation-model field is indexed even though the assistant subsection is active.
    assert!(
        idx.iter().any(|h| h.crumb.contains("Имперсонация")),
        "the index has impersonation-subsection fields"
    );
}

#[test]
fn memory_section_gathers_rag_notes_self_model() {
    // The "Memory" section gathered fields previously smeared across "Tools".
    let mut s = screen();
    goto_section(&mut s, Section::Memory);
    let ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
    for id in [
        FieldId::RagTarget,
        FieldId::NotesAutoConsolidate,
        FieldId::SmMaxNarrative,
        FieldId::SmProtocol,
    ] {
        assert!(ids.contains(&id), "\"Memory\" is missing {id:?}");
    }
    // And "Tools" no longer has them — only gates/parameters live there.
    goto_section(&mut s, Section::Tools);
    let tool_ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
    assert!(!tool_ids.contains(&FieldId::RagTarget));
    assert!(!tool_ids.contains(&FieldId::SmMaxNarrative));
    // max_tool_rounds moved from the former "Inference" into "Tools".
    assert!(tool_ids.contains(&FieldId::MaxToolRounds));
}

#[test]
fn fields_carry_group_headers() {
    // The section's fields are marked with semantic groups (group headers in the UI).
    let s = screen();
    let groups: Vec<&str> = s.model_fields().iter().map(|f| f.group).collect();
    // The subsection/mode — outside a group; server parameters — in the "Server" group.
    assert!(groups.iter().any(|g| g.is_empty()));
    assert!(groups.contains(&"Сервер"));
    // Sampling: parameters are grouped by meaning.
    let sg: Vec<&str> = s.sampling_fields().iter().map(|f| f.group).collect();
    assert!(sg.contains(&"Основные"));
    assert!(sg.contains(&"Рассуждения"));
}

#[test]
fn long_value_is_truncated_with_ellipsis() {
    // A very long value is truncated with "…" to the column width.
    let palette = Palette::default();
    let f = FieldRow {
        id: FieldId::XModel,
        label: "GGUF-модель (-m)".into(),
        kind: FieldKind::Text("D:\\LLM\\GGUF\\very-long-model-name-".repeat(4)),
        group: "Модель",
        hint: None,
        description: None,
        warn: false,
    };
    let line = render_field_line(&f, 20, 24, false, false, &palette);
    let rendered: String = line.spans.iter().map(|sp| sp.content.as_ref()).collect();
    assert!(
        rendered.contains('…'),
        "the long value is truncated: {rendered:?}"
    );
}

#[test]
fn selected_field_shows_green_rail() {
    let palette = Palette::default();
    let f = FieldRow {
        id: FieldId::XModel,
        label: "GGUF-модель (-m)".into(),
        kind: FieldKind::Text("model".into()),
        group: "Модель",
        hint: None,
        description: None,
        warn: false,
    };
    // The selected field — a green rail `▌` in the left column (as the active menu section).
    let sel = render_field_line(&f, 20, 24, false, true, &palette);
    assert_eq!(sel.spans[0].content.as_ref(), "▌ ");
    assert_eq!(sel.spans[0].style.fg, Some(palette.success));
    // Unselected, modified — the "•" marker.
    let modf = render_field_line(&f, 20, 24, true, false, &palette);
    assert_eq!(modf.spans[0].content.as_ref(), "• ");
    // Unselected, unmodified — empty.
    let plain = render_field_line(&f, 20, 24, false, false, &palette);
    assert_eq!(plain.spans[0].content.as_ref(), "  ");
    // A selected, modified field's rail takes priority over the "•" marker.
    let both = render_field_line(&f, 20, 24, true, true, &palette);
    assert_eq!(both.spans[0].content.as_ref(), "▌ ");
}

#[test]
fn section_label_col_has_floor_cap_and_skips_subsection() {
    let text = |s: &str| FieldKind::Text(s.into());
    // Only short labels → the floor MIN_LABEL_COL.
    let short = vec![row(FieldId::PName, "Имя", text("x"))];
    assert_eq!(section_label_col(&short), MIN_LABEL_COL);
    // The longest label within the cap sets the whole section's column.
    let medium = vec![
        row(FieldId::PName, "Имя", text("x")),
        row(FieldId::PGreeting, "Подпись средней длины!", text("y")),
    ];
    assert_eq!(section_label_col(&medium), 22);
    // An overlong label (> LABEL_CAP) doesn't push the column…
    let mut with_outlier = medium;
    with_outlier.push(row(
        FieldId::PSystem,
        &"а".repeat(LABEL_CAP + 12),
        text("z"),
    ));
    assert_eq!(section_label_col(&with_outlier), 22);
    // …and the subsection selector (a tab strip, not a list row) is excluded entirely.
    let with_sub = vec![
        row(FieldId::PName, "Имя", text("x")),
        row(FieldId::ModelSub, &"б".repeat(25), text("s")),
    ];
    assert_eq!(section_label_col(&with_sub), MIN_LABEL_COL);
}

#[test]
fn all_labels_fit_alignment_cap() {
    // All labels of all sections/subsections fit within the alignment cap —
    // each section's values line up on one vertical, with no local
    // overflows. For a new long name — shorten the label, moving the
    // context into the group header (like "Copy conversation (F5)").
    let mut cfg = AppConfig::default();
    // Speculative-decoding draft fields are visible only for draft-* types.
    cfg.engine.managed.spec_type = SpecType::DraftMtp;
    let mut p = Profile::new("Базовый", "Ты — ассистент.");
    p.enabled_tools = default_tool_ids();
    let s = SettingsScreen::new(cfg, vec![p], vec![]);
    let mut all: Vec<(&str, Vec<FieldRow>)> = vec![
        ("Инструменты", s.tool_fields()),
        ("Память", s.memory_fields()),
        ("Интерфейс", s.interface_fields()),
    ];
    for tab in ModelTab::ALL {
        all.push(("Модель", s.model_fields_for(tab)));
    }
    for sub in Subsection::ALL {
        all.push(("Семплинг", s.sampling_fields_for(sub)));
        all.push(("Профили", s.profile_fields_for(sub)));
    }
    for (section, fields) in all {
        for f in fields {
            assert!(
                label_width(&f.label) <= LABEL_CAP,
                "label \"{}\" ({section}) is wider than LABEL_CAP={LABEL_CAP} — shorten \
                     it or move the context into the group header",
                f.label
            );
        }
    }
}

#[test]
fn value_column_is_shared_across_groups() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    // Values of different section groups line up on one vertical (a single column
    // per section; a per-group column "sawtoothed" between groups).
    let mut s = screen();
    goto_section(&mut s, Section::Tools);
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
    let buf = term.backend().buffer();
    let lines: Vec<String> = (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect()
        })
        .collect();
    // The cell of the first non-space character after the label (= the value's start).
    let value_cell = |label: &str| -> usize {
        let line = lines
            .iter()
            .find(|l| l.contains(label))
            .unwrap_or_else(|| panic!("no row with the label {label:?}"));
        let chars: Vec<char> = line.chars().collect();
        let needle: Vec<char> = label.chars().collect();
        let start = (0..=chars.len() - needle.len())
            .find(|&i| chars[i..i + needle.len()] == needle[..])
            .unwrap();
        let after = start + needle.len();
        after + chars[after..].iter().take_while(|c| **c == ' ').count()
    };
    // Three fields from three different groups ("Agentic loop"/"Web search"/"Files").
    let a = value_cell("Лимит раундов инструментов");
    let b = value_cell("Web-поиск");
    let c = value_cell("Доступ к файлам");
    assert_eq!(
        a, b,
        "the \"Agentic loop\" and \"Web search\" group values are in one column"
    );
    assert_eq!(b, c, "the \"Files\" group values are in the same column");
}

#[test]
fn sampling_extensions_have_descriptions() {
    // Every sampling parameter has a hint in both subsections —
    // both llama.cpp extensions and base OpenAI fields.
    let s = screen();
    for &p in SAMPLING_PARAMS {
        assert!(
            field_desc(&s, FieldId::S(p)).is_some(),
            "no hint for {p:?} (Assistant)",
        );
        assert!(
            field_desc(&s, FieldId::IS(p)).is_some(),
            "no hint for {p:?} (Impersonation)",
        );
    }
}

/// The "API key" field exists only in cloud modes and shows a **status**, not a
/// secret; a stored key is reflected as the value "configured".
#[test]
fn api_key_field_shows_status_in_cloud_modes_only() {
    let mut s = screen();
    goto_section(&mut s, Section::Model);
    let has_key_field = |s: &SettingsScreen| s.fields().iter().any(|f| f.id == FieldId::XApiKey);
    // A local managed engine — no key field (no cloud provider exists).
    assert!(!has_key_field(&s));

    s.config.engine.mode = ServerMode::OpenAi;
    assert!(has_key_field(&s));
    let value = |s: &SettingsScreen| {
        s.fields()
            .into_iter()
            .find(|f| f.id == FieldId::XApiKey)
            .map(|f| value_text(&f.kind, s.loc()))
            .unwrap()
    };
    let unset = value(&s);
    // This provider's key is stored on the machine → the status changes.
    s.set_api_keys_present(vec![CloudProvider::OpenAi]);
    let set = value(&s);
    assert_ne!(
        unset, set,
        "the field's status doesn't reflect the key's presence"
    );
    // Another provider's key doesn't affect OpenAI's status.
    s.set_api_keys_present(vec![CloudProvider::Claude]);
    assert_eq!(value(&s), unset);
    // The secret itself never appears in the field's value under any status.
    assert!(!set.contains("sk-"));
}

/// Editing the key field opens an **empty** masked editor (a stored key
/// can't be shown), and the commit goes as a separate intent — not into the screen's config.
#[test]
fn api_key_editor_is_masked_empty_and_commits_intent() {
    let mut s = screen();
    s.config.engine.mode = ServerMode::Claude;
    s.set_api_keys_present(vec![CloudProvider::Claude]); // the key is already stored
    goto_section(&mut s, Section::Model);
    goto_field(&mut s, FieldId::XApiKey);

    s.handle_key(key(KeyCode::Enter));
    let editor = s.editor.as_ref().expect("the key field's editor is open");
    assert_eq!(editor.input.text(), "", "the seed must be empty");
    assert!(editor.input.is_masked(), "the secret field must be masked");

    for c in "sk-ant-123".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    let intent = s.handle_key(key(KeyCode::Enter));
    assert_eq!(
        intent,
        Some(SettingsIntent::SetApiKey {
            provider: CloudProvider::Claude,
            key: "sk-ant-123".into()
        })
    );
    // The secret didn't leak into the config working copy.
    let json = serde_json::to_string(&s.config).unwrap();
    assert!(
        !json.contains("sk-ant-123"),
        "the key leaked into the screen's config: {json}"
    );
}

/// `Del` on the key field deletes the stored key (an empty intent value);
/// if there's no key — a no-op (nothing to delete).
#[test]
fn del_on_api_key_field_removes_stored_key() {
    let mut s = screen();
    s.config.engine.mode = ServerMode::Gemini;
    goto_section(&mut s, Section::Model);
    goto_field(&mut s, FieldId::XApiKey);
    // No key — the reset does nothing.
    assert_eq!(s.handle_key(key(KeyCode::Delete)), None);

    s.set_api_keys_present(vec![CloudProvider::Gemini]);
    assert_eq!(
        s.handle_key(key(KeyCode::Delete)),
        Some(SettingsIntent::SetApiKey {
            provider: CloudProvider::Gemini,
            key: String::new()
        })
    );
}

/// The key field exists for all three slots and addresses **its own** engine's
/// provider: the key is shared across chat/impersonation/embeddings of one provider.
#[test]
fn api_key_field_targets_provider_of_its_slot() {
    let mut s = screen();
    s.config.engine.mode = ServerMode::OpenAi;
    s.config.impersonation_engine.mode = ImpersonationMode::Claude;
    s.config.embed.mode = ServerMode::Gemini;
    assert_eq!(
        s.api_key_field_provider(FieldId::XApiKey),
        Some(CloudProvider::OpenAi)
    );
    assert_eq!(
        s.api_key_field_provider(FieldId::IxApiKey),
        Some(CloudProvider::Claude)
    );
    assert_eq!(
        s.api_key_field_provider(FieldId::EApiKey),
        Some(CloudProvider::Gemini)
    );
    assert_eq!(s.api_key_field_provider(FieldId::XUrl), None);
}

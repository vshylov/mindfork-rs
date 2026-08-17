//! Settings-screen tests (via handle_key/render). See mod.rs.

use super::helpers::*;
use super::*;
use crate::features::tools::default_tool_ids;
use crate::shared::config::{FlashAttn, MediaResolution, PythonMode, SpecType};
use crate::shared::embed_prefix::EmbedConvention;
use crate::shared::secrets::ExternalSlot;

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
/// After the call, focus is in the menu, no field is focused.
///
/// Tab **preserves** the focus (docs/history/settings-navigation.md R4), so the helper returns
/// to the sections itself — it can no longer rely on Tab's former side effect. Without
/// this, [`goto_field`]'s `Enter` would open an editor instead of entering the pane.
fn goto_section(s: &mut SettingsScreen, sec: Section) {
    if s.focus == Focus::Fields {
        s.handle_key(key(KeyCode::Esc));
    }
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
    rows.extend(s.plugin_fields());
    rows.extend(s.memory_fields());
    rows.extend(s.data_fields());
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

/// Moves to another field when focus is **already** in the pane: steps back out
/// first, since [`goto_field`] starts from the menu and its `Enter` would otherwise
/// open an editor instead of entering the pane.
fn goto_field_again(s: &mut SettingsScreen, id: FieldId) {
    s.handle_key(key(KeyCode::Esc));
    goto_field(s, id);
}

/// Types `value` into the **currently selected** secret field and returns the commit
/// intent, asserting on the way that the editor opened *empty and masked* — a stored
/// secret can never be shown back, whichever field it belongs to. Every such test
/// needs the same three steps before it can assert anything about its own field, and
/// the third copy of an opening is a fixture rather than a test (lessons.md §2).
fn enter_secret(s: &mut SettingsScreen, value: &str) -> Option<SettingsIntent> {
    s.handle_key(key(KeyCode::Enter));
    let editor = s
        .editor
        .as_ref()
        .expect("the secret field's editor is open");
    assert_eq!(editor.input.text(), "", "a stored secret can't be shown");
    assert!(editor.input.is_masked(), "a secret field must be masked");
    for c in value.chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    s.handle_key(key(KeyCode::Enter))
}

#[test]
fn esc_closes_from_the_sections() {
    let mut s = screen();
    assert_eq!(s.handle_key(key(KeyCode::Esc)), Some(SettingsIntent::Close));
}

/// The focus ladder (docs/history/settings-navigation.md R3): Esc in the field pane steps back
/// to the sections **without** closing; only the second one closes.
#[test]
fn esc_steps_out_of_the_field_pane_then_closes() {
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter));
    assert!(s.focus == Focus::Fields);

    assert_eq!(s.handle_key(key(KeyCode::Esc)), None, "must not close yet");
    assert!(s.focus == Focus::Menu);
    assert_eq!(s.handle_key(key(KeyCode::Esc)), Some(SettingsIntent::Close));
}

/// `→` no longer enters the pane — Enter is the only way in (R1). This is the keystroke
/// that used to teach the "`←` leaves" model behind the accidental edits (§1.2).
#[test]
fn right_does_not_enter_the_field_pane() {
    let mut s = screen();
    assert_eq!(s.handle_key(key(KeyCode::Right)), None);
    assert!(s.focus == Focus::Menu, "`→` must not enter the field pane");

    s.handle_key(key(KeyCode::Enter));
    assert!(s.focus == Focus::Fields);
}

/// A regression guard for R2, not its detector: a Choice field returned early under the
/// old code too, so this pins that symmetrizing the `←`/`→` arms didn't break value
/// cycling. The behaviour change itself is caught by
/// [`left_on_a_non_choice_field_is_a_no_op`] (verified by mutation).
#[test]
fn left_on_a_choice_field_cycles_and_keeps_focus() {
    let mut s = screen();
    goto_section(&mut s, Section::Interface);
    goto_field(&mut s, FieldId::ITheme);
    let before = s.config.interface.theme;

    let intent = s.handle_key(key(KeyCode::Left));
    assert!(matches!(intent, Some(SettingsIntent::SaveConfig(_))));
    assert_ne!(s.config.interface.theme, before);
    assert!(s.focus == Focus::Fields, "`←` must not leave the pane");
}

/// `←` on a non-Choice field is a plain no-op — it no longer returns to the menu (R2).
#[test]
fn left_on_a_non_choice_field_is_a_no_op() {
    let mut s = screen();
    goto_section(&mut s, Section::Tools);
    goto_field(&mut s, FieldId::TWeb); // a Toggle
    let idx = s.field_idx;

    assert_eq!(s.handle_key(key(KeyCode::Left)), None);
    assert!(s.focus == Focus::Fields, "`←` must not leave the pane");
    assert_eq!(s.field_idx, idx, "and must not move the cursor");
}

/// Tab switches the section and nothing else: the focus stays where it was, in either
/// state (R4). The field index still resets — field sets differ per section.
#[test]
fn tab_preserves_focus_and_resets_the_field_index() {
    let mut s = screen();
    // On the sections — stays on the sections.
    s.handle_key(key(KeyCode::Tab));
    assert!(s.focus == Focus::Menu);

    // In the pane — stays in the pane, on the section's first field.
    s.handle_key(key(KeyCode::Enter));
    s.handle_key(key(KeyCode::Down));
    let section_before = s.section();
    s.handle_key(key(KeyCode::Tab));
    assert!(s.focus == Focus::Fields, "Tab must not drop the focus");
    assert_ne!(s.section(), section_before);
    assert_eq!(s.field_idx, 0);
}

/// `↑` on the first field stays put (R5) — no implicit arrow-driven focus move, which
/// is the category of behaviour this whole change removes.
#[test]
fn up_on_the_first_field_stays_in_the_pane() {
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter));
    assert_eq!(s.handle_key(key(KeyCode::Up)), None);
    assert!(s.focus == Focus::Fields);
    assert_eq!(s.field_idx, 0);
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
fn tools_have_confirm_dangerous_toggle() {
    let mut s = screen();
    // The field is in the "Tools" section (the "Agentic loop" group — it gates the
    // loop's tool calls, not one particular tool).
    goto_section(&mut s, Section::Tools);
    let ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
    assert!(ids.contains(&FieldId::TConfirmDangerous));
    // Off by default; toggling it saves the config with the flag raised.
    goto_field(&mut s, FieldId::TConfirmDangerous);
    match s.handle_key(key(KeyCode::Char(' '))) {
        Some(SettingsIntent::SaveConfig(c)) => assert!(c.tools.confirm_dangerous),
        other => panic!("expected SaveConfig, got {other:?}"),
    }
    assert!(field_desc(&s, FieldId::TConfirmDangerous).is_some());
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
fn video_group_rows_present_grouped_and_ordered() {
    // The four "Video (YouTube)" rows live in "Tools", in the documented order
    // (Model / Input resolution / Max video length / API key), sharing one group
    // header, each with its own description hint.
    let s = screen();
    // `tool_fields()` builds the "Tools" section's rows directly — no navigation needed.
    let fields = s.tool_fields();
    let video_ids: Vec<FieldId> = fields
        .iter()
        .filter(|f| {
            matches!(
                f.id,
                FieldId::VideoModel
                    | FieldId::VideoResolution
                    | FieldId::VideoMaxMinutes
                    | FieldId::VideoApiKeyEnv
            )
        })
        .map(|f| f.id)
        .collect();
    assert_eq!(
        video_ids,
        vec![
            FieldId::VideoModel,
            FieldId::VideoResolution,
            FieldId::VideoMaxMinutes,
            FieldId::VideoApiKeyEnv,
        ]
    );
    // All four rows carry the same group header (the default screen() locale is ru).
    for id in &video_ids {
        let group = fields.iter().find(|f| f.id == *id).unwrap().group;
        assert_eq!(
            group, "Видео (YouTube)",
            "field {id:?} not in the video group"
        );
    }
    for id in video_ids {
        assert!(
            field_desc(&s, id).is_some(),
            "field {id:?} has no description"
        );
    }
}

#[test]
fn video_resolution_field_cycles_and_saves() {
    let mut s = screen();
    assert_eq!(s.config.video.media_resolution, MediaResolution::Low);
    goto_section(&mut s, Section::Tools);
    goto_field(&mut s, FieldId::VideoResolution);
    let intent = s.handle_key(key(KeyCode::Right));
    match intent {
        Some(SettingsIntent::SaveConfig(c)) => {
            assert_eq!(c.video.media_resolution, MediaResolution::Medium)
        }
        other => panic!("expected SaveConfig, got {other:?}"),
    }
    // The working copy is updated too — the row reflects the new option.
    assert_eq!(s.config.video.media_resolution, MediaResolution::Medium);
}

#[test]
fn video_model_text_field_writes_config() {
    let mut s = screen();
    goto_section(&mut s, Section::Tools);
    goto_field(&mut s, FieldId::VideoModel);
    s.handle_key(key(KeyCode::Enter)); // open the editor (seeded with the default model)
    s.handle_key(ctrl('k')); // clear it
    for c in "gemini-2.5-pro".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    match s.handle_key(key(KeyCode::Enter)) {
        Some(SettingsIntent::SaveConfig(c)) => {
            assert_eq!(c.video.model_name.as_deref(), Some("gemini-2.5-pro"))
        }
        other => panic!("expected SaveConfig, got {other:?}"),
    }

    // An empty value clears the field to `None` (a required-looking field that's
    // actually optional — the tool reports itself unconfigured).
    let mut s2 = screen();
    goto_section(&mut s2, Section::Tools);
    goto_field(&mut s2, FieldId::VideoModel);
    s2.handle_key(key(KeyCode::Enter));
    s2.handle_key(ctrl('k'));
    match s2.handle_key(key(KeyCode::Enter)) {
        Some(SettingsIntent::SaveConfig(c)) => assert_eq!(c.video.model_name, None),
        other => panic!("expected SaveConfig, got {other:?}"),
    }
}

#[test]
fn video_max_minutes_is_a_validated_int_field() {
    let mut s = screen();
    goto_section(&mut s, Section::Tools);
    goto_field(&mut s, FieldId::VideoMaxMinutes);
    assert_eq!(field_num_kind(FieldId::VideoMaxMinutes), Some(NumKind::Int));
    s.handle_key(key(KeyCode::Enter)); // open the editor (seeded "30")
    s.handle_key(ctrl('k'));
    s.handle_key(key(KeyCode::Char('x')));
    // Non-numeric input keeps the editor open and flags a validation error.
    assert!(s.handle_key(key(KeyCode::Enter)).is_none());
    assert!(
        s.editor.is_some(),
        "the editor stays open on an invalid number"
    );
    assert!(s.editor.as_ref().unwrap().error.is_some());
    // Fixing it commits normally.
    s.handle_key(ctrl('k'));
    for c in "45".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    match s.handle_key(key(KeyCode::Enter)) {
        Some(SettingsIntent::SaveConfig(c)) => assert_eq!(c.video.max_minutes, 45),
        other => panic!("expected SaveConfig, got {other:?}"),
    }
    // `0` is a valid value (no ceiling), not an error.
    let mut s3 = screen();
    goto_section(&mut s3, Section::Tools);
    goto_field(&mut s3, FieldId::VideoMaxMinutes);
    s3.handle_key(key(KeyCode::Enter));
    s3.handle_key(ctrl('k'));
    s3.handle_key(key(KeyCode::Char('0')));
    match s3.handle_key(key(KeyCode::Enter)) {
        Some(SettingsIntent::SaveConfig(c)) => assert_eq!(c.video.max_minutes, 0),
        other => panic!("expected SaveConfig, got {other:?}"),
    }
}

#[test]
fn video_api_key_env_text_field_writes_config() {
    let mut s = screen();
    goto_section(&mut s, Section::Tools);
    goto_field(&mut s, FieldId::VideoApiKeyEnv);
    s.handle_key(key(KeyCode::Enter));
    for c in "GEMINI_API_KEY".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    match s.handle_key(key(KeyCode::Enter)) {
        Some(SettingsIntent::SaveConfig(c)) => {
            assert_eq!(c.video.api_key_env.as_deref(), Some("GEMINI_API_KEY"))
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
fn embeddings_tab_has_the_input_convention_field() {
    // The one control for per-model input prefixes. It lives in the Embeddings
    // tab regardless of the server mode — the convention is a property of the
    // model, not of where it runs (docs/research/embedding-input-prefixes.md).
    let mut s = screen();
    goto_section(&mut s, Section::Model);
    s.model_sub = ModelTab::Embeddings;
    assert!(
        s.fields().iter().any(|f| f.id == FieldId::EConvention),
        "the convention field is present in managed mode"
    );
    s.config.embed.mode = ServerMode::OpenAi;
    assert!(
        s.fields().iter().any(|f| f.id == FieldId::EConvention),
        "and in a cloud mode too"
    );
    // It is not in the assistant's tab — that engine has no embeddings.
    s.model_sub = ModelTab::Assistant;
    assert!(!s.fields().iter().any(|f| f.id == FieldId::EConvention));
}

#[test]
fn cycling_the_input_convention_saves_it() {
    let mut s = screen();
    assert_eq!(s.config.embed.convention, EmbedConvention::None, "default");
    match s.cycle_field(FieldId::EConvention, 1) {
        Some(SettingsIntent::SaveConfig(c)) => {
            assert_eq!(c.embed.convention, EmbedConvention::E5)
        }
        other => panic!("expected a config save, got {other:?}"),
    }
    // Described, since a wrong value measurably degrades retrieval.
    let mut s2 = screen();
    goto_section(&mut s2, Section::Model);
    s2.model_sub = ModelTab::Embeddings;
    let row = s2
        .fields()
        .into_iter()
        .find(|f| f.id == FieldId::EConvention)
        .unwrap();
    assert!(row.description.is_some());
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
fn interface_has_the_osc52_choice() {
    let mut s = screen();
    // In the "Interface" section, with a hint — the field needs one more than
    // most: whether it does anything depends on the terminal, not on the app.
    let rows = s.interface_fields();
    assert!(rows.iter().any(|r| r.id == FieldId::IClipboardOsc52));
    assert!(field_desc(&s, FieldId::IClipboardOsc52).is_some());

    // Cycling forward walks auto -> always -> off and back, saving each step.
    use crate::shared::osc52::Osc52Mode;
    let mut seen = vec![s.config.interface.clipboard_osc52];
    for _ in 0..3 {
        match s.cycle_field(FieldId::IClipboardOsc52, 1) {
            Some(SettingsIntent::SaveConfig(c)) => seen.push(c.interface.clipboard_osc52),
            other => panic!("expected SaveConfig, got {other:?}"),
        }
    }
    assert_eq!(
        seen,
        vec![
            Osc52Mode::Auto,
            Osc52Mode::Always,
            Osc52Mode::Off,
            Osc52Mode::Auto
        ],
        "the cycle must return to where it started"
    );
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

    // Switch the mode to external: a URL appears and the instructions disappear (only
    // the OpenAI cloud supports them). The key row stays — an external speech server
    // can require one too, and since docs/history/external-api-key.md it can be entered here
    // rather than only named as an env variable — but it now addresses that slot's own
    // key rather than a provider's.
    s.config.tts.mode = crate::shared::config::TtsMode::External;
    let ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
    assert!(ids.contains(&FieldId::TtsUrl));
    assert!(ids.contains(&FieldId::TtsApiKey));
    assert_eq!(
        s.secret_field_key(FieldId::TtsApiKey),
        Some(SecretKey::External(ExternalSlot::Tts))
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

/// Switches the "Profiles" section to the "Impersonation" subsection.
fn goto_impersonation_sub(s: &mut SettingsScreen) {
    goto_section(s, Section::Profiles);
    goto_field(s, FieldId::ProfileSub); // the subsection tab strip (field 0)
    s.handle_key(key(KeyCode::Right)); // → Impersonation
    assert_eq!(s.profile_sub, Subsection::Impersonation);
}

#[test]
fn impersonation_profile_subsection_edits_its_own_list_without_tools() {
    let mut s = screen();
    s.config.impersonation_profiles = vec![crate::shared::config::ImpersonationProfile::new(
        "Юзер",
        "Ты — Владимир.",
    )];
    goto_impersonation_sub(&mut s);
    let fields = s.fields();
    assert!(
        !fields.iter().any(|f| matches!(f.id, FieldId::PTool(_))),
        "the impersonation subsection has no tool toggles"
    );
    // Its own selector/name/system message — not the assistant profile's.
    assert!(fields.iter().any(|f| f.id == FieldId::IpSelect));
    assert!(fields.iter().any(|f| f.id == FieldId::IpName));
    assert!(fields.iter().any(|f| f.id == FieldId::IpSystem));
    assert!(
        !fields
            .iter()
            .any(|f| matches!(f.id, FieldId::PSelect | FieldId::PName)),
        "the assistant profile selector/name don't belong on the impersonation tab"
    );
}

#[test]
fn impersonation_profile_create_edit_delete() {
    let mut s = screen();
    goto_impersonation_sub(&mut s);
    // An empty list — only the selector placeholder; Ctrl+N creates the first entry.
    assert!(s.fields().iter().all(|f| f.id != FieldId::IpName));
    assert!(matches!(
        s.handle_key(ctrl('n')),
        Some(SettingsIntent::SaveConfig(_))
    ));
    assert_eq!(s.config.impersonation_profiles.len(), 1);
    assert_eq!(s.imp_profile_idx, 0);

    // Editing the name and system message goes into the config working copy.
    goto_field(&mut s, FieldId::IpName);
    s.handle_key(key(KeyCode::Enter));
    s.handle_key(ctrl('k')); // the editor is seeded with the current name
    for c in "Владимир".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    assert!(matches!(
        s.handle_key(key(KeyCode::Enter)),
        Some(SettingsIntent::SaveConfig(_))
    ));
    assert_eq!(s.config.impersonation_profiles[0].name, "Владимир");

    s.handle_key(key(KeyCode::Esc)); // back to the menu (goto_field starts from there)
    goto_field(&mut s, FieldId::IpSystem);
    s.handle_key(key(KeyCode::Enter));
    for c in "Ты — я.".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    s.handle_key(key(KeyCode::Enter));
    assert_eq!(s.config.impersonation_profiles[0].system_message, "Ты — я.");

    // Ctrl+D removes it; the list is empty again.
    assert!(matches!(
        s.handle_key(ctrl('d')),
        Some(SettingsIntent::SaveConfig(_))
    ));
    assert!(s.config.impersonation_profiles.is_empty());
}

#[test]
fn assistant_profile_references_impersonation_profile() {
    let mut s = screen();
    let ip = crate::shared::config::ImpersonationProfile::new("Юзер", "Ты — Владимир.");
    let ip_id = ip.id;
    s.config.impersonation_profiles = vec![ip];
    goto_section(&mut s, Section::Profiles);
    goto_field(&mut s, FieldId::PImpProfile);
    // "Not set" by default — the shared default text applies.
    assert_eq!(
        s.fields()
            .iter()
            .find(|f| f.id == FieldId::PImpProfile)
            .map(|f| value_text(&f.kind, s.loc())),
        Some("(не задан)".to_string())
    );
    // → picks the only impersonation profile and saves the profile edit.
    let intent = s.handle_key(key(KeyCode::Right));
    assert!(matches!(intent, Some(SettingsIntent::SaveProfile { .. })));
    assert_eq!(s.profiles[0].impersonation_profile_id, Some(ip_id));
    // → again wraps back to "not set" (the list has a single entry + the empty option).
    s.handle_key(key(KeyCode::Right));
    assert_eq!(s.profiles[0].impersonation_profile_id, None);
}

#[test]
fn new_profile_is_selected_after_the_settings_reemit() {
    let mut s = screen();
    goto_section(&mut s, Section::Profiles);
    assert_eq!(s.profile_idx, 0);
    // Ctrl+N only asks: the orchestrator owns the list.
    assert!(matches!(
        s.handle_key(ctrl('n')),
        Some(SettingsIntent::CreateProfile { .. })
    ));
    // The re-emitted snapshot carries the new profile — it must become the selected one.
    let mut profiles = s.profiles.clone();
    let created = Profile::new("Новый профиль", "");
    let created_id = created.id;
    profiles.push(created);
    s.refresh(s.config.clone(), profiles, Vec::new());
    assert_eq!(s.profiles[s.profile_idx].id, created_id);

    // One-shot: an unrelated later re-emit doesn't move the selection.
    let mut profiles = s.profiles.clone();
    profiles.push(Profile::new("Ещё один", ""));
    s.refresh(s.config.clone(), profiles, Vec::new());
    assert_eq!(s.profiles[s.profile_idx].id, created_id);
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

/// Custom role names (spec §5.1): the fields live in the profile's "Persona" group,
/// carry a description, and commit into `character_names` — an empty value is
/// legitimate ("not set" → the localized label).
#[test]
fn role_name_fields_edit_character_names() {
    let mut s = screen();
    goto_section(&mut s, Section::Profiles);
    goto_field(&mut s, FieldId::PUserName);
    s.handle_key(key(KeyCode::Enter)); // open the editor
    for c in "Гайя".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    match s.handle_key(key(KeyCode::Enter)) {
        Some(SettingsIntent::SaveProfile { edit, .. }) => {
            let names = edit
                .character_names
                .expect("names are part of the snapshot");
            assert_eq!(names.user, "Гайя");
            assert_eq!(names.assistant, "", "the assistant's name isn't touched");
        }
        other => panic!("expected SaveProfile, got {other:?}"),
    }
    // The working copy is updated too — the row now shows the new value.
    assert!(s.fields().iter().any(
        |f| f.id == FieldId::PUserName && matches!(&f.kind, FieldKind::Text(t) if t == "Гайя")
    ));

    // Clearing the name is a valid edit: the feed goes back to the localized header.
    let mut s2 = screen();
    s2.profiles[0].character_names.user = "Гайя".into();
    goto_section(&mut s2, Section::Profiles);
    goto_field(&mut s2, FieldId::PUserName);
    s2.handle_key(key(KeyCode::Enter));
    s2.handle_key(ctrl('k')); // clear the field
    match s2.handle_key(key(KeyCode::Enter)) {
        Some(SettingsIntent::SaveProfile { edit, .. }) => {
            assert_eq!(edit.character_names.unwrap().user, "");
        }
        other => panic!("expected SaveProfile, got {other:?}"),
    }
}

#[test]
fn role_name_fields_are_described_and_grouped_with_persona() {
    let s = screen();
    let rows = s.profile_fields_for(Subsection::Assistant);
    let persona = rows
        .iter()
        .find(|r| r.id == FieldId::PSystem)
        .expect("the system message is in the Persona group")
        .group;
    for id in [FieldId::PUserName, FieldId::PAssistantName] {
        let r = rows.iter().find(|r| r.id == id).expect("field present");
        assert_eq!(r.group, persona);
        assert!(field_desc(&s, id).is_some(), "{id:?} has no description");
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

// ---------- undo/redo of an edit (docs/history/settings-undo.md) ----------

/// The core: `Ctrl+Z` puts the previous value back and re-emits the same intent the
/// edit produced — no new command is needed, since `SaveConfig` already carries the
/// whole working config.
#[test]
fn undo_restores_a_config_value_and_emits_save() {
    let mut s = screen();
    goto_section(&mut s, Section::Interface);
    goto_field(&mut s, FieldId::ITheme);
    let before = s.config.interface.theme;

    s.handle_key(key(KeyCode::Right));
    assert_ne!(s.config.interface.theme, before);

    let intent = s.handle_key(ctrl('z'));
    assert!(matches!(intent, Some(SettingsIntent::SaveConfig(_))));
    assert_eq!(s.config.interface.theme, before, "the value must come back");
}

/// A profile edit travels as a different intent (`SaveProfile`), and undo mirrors it.
#[test]
fn undo_restores_a_profile_field() {
    let mut s = screen();
    goto_section(&mut s, Section::Profiles);
    goto_field(&mut s, FieldId::PUserName);
    s.handle_key(key(KeyCode::Enter));
    for c in "Гея".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    s.handle_key(key(KeyCode::Enter));
    assert_eq!(s.profiles[0].character_names.user, "Гея");

    let intent = s.handle_key(ctrl('z'));
    assert!(matches!(intent, Some(SettingsIntent::SaveProfile { .. })));
    assert_eq!(s.profiles[0].character_names.user, "");
}

/// U2: a run of edits to the **same** field is one step — cycling past the value you
/// wanted comes back in a single press.
#[test]
fn consecutive_edits_of_one_field_undo_together() {
    let mut s = screen();
    goto_section(&mut s, Section::Model);
    goto_field(&mut s, FieldId::XMode);
    let before = s.config.engine.mode;

    for _ in 0..3 {
        s.handle_key(key(KeyCode::Right));
    }
    assert_ne!(s.config.engine.mode, before);

    s.handle_key(ctrl('z'));
    assert_eq!(
        s.config.engine.mode, before,
        "one press undoes the whole run"
    );
}

/// …but two different fields stay two steps.
#[test]
fn edits_of_different_fields_are_separate_steps() {
    let mut s = screen();
    goto_section(&mut s, Section::Interface);
    goto_field(&mut s, FieldId::ITheme);
    let theme = s.config.interface.theme;
    s.handle_key(key(KeyCode::Right));

    goto_field_again(&mut s, FieldId::ICompat);
    let compat = s.config.interface.terminal_compat;
    s.handle_key(key(KeyCode::Char(' ')));
    assert_ne!(s.config.interface.terminal_compat, compat);

    s.handle_key(ctrl('z'));
    assert_eq!(s.config.interface.terminal_compat, compat, "the last edit");
    assert_ne!(s.config.interface.theme, theme, "but not the one before it");

    s.handle_key(ctrl('z'));
    assert_eq!(s.config.interface.theme, theme);
}

/// U3: redo re-applies, and a fresh edit invalidates the redo branch.
#[test]
fn redo_reapplies_and_a_new_edit_clears_it() {
    let mut s = screen();
    goto_section(&mut s, Section::Interface);
    goto_field(&mut s, FieldId::ITheme);
    let before = s.config.interface.theme;
    s.handle_key(key(KeyCode::Right));
    let after = s.config.interface.theme;

    s.handle_key(ctrl('z'));
    assert_eq!(s.config.interface.theme, before);
    let intent = s.handle_key(ctrl('y'));
    assert!(matches!(intent, Some(SettingsIntent::SaveConfig(_))));
    assert_eq!(s.config.interface.theme, after, "redo re-applies");

    s.handle_key(ctrl('z'));
    goto_field_again(&mut s, FieldId::ICompat);
    s.handle_key(key(KeyCode::Char(' '))); // a genuine fresh edit
    assert_eq!(s.handle_key(ctrl('y')), None, "redo was invalidated");
}

#[test]
fn undo_on_an_empty_stack_is_a_no_op() {
    let mut s = screen();
    assert_eq!(s.handle_key(ctrl('z')), None);
    assert_eq!(s.handle_key(ctrl('y')), None);
}

/// The layering that must not regress: while a field editor is open, `Ctrl+Z` is the
/// **text** undo of its `InputBox` — it must not reach past it and revert a setting.
#[test]
fn ctrl_z_inside_the_editor_is_text_undo_not_settings_undo() {
    let mut s = screen();
    goto_section(&mut s, Section::Interface);
    goto_field(&mut s, FieldId::ITheme);
    let theme_before = s.config.interface.theme;
    s.handle_key(key(KeyCode::Right)); // a setting edit worth reverting
    let theme_after = s.config.interface.theme;

    goto_field_again(&mut s, FieldId::IDicts);
    s.handle_key(key(KeyCode::Enter)); // open the text editor
    for c in "abc".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    assert_eq!(s.handle_key(ctrl('z')), None, "consumed by the editor");
    assert_eq!(
        s.config.interface.theme, theme_after,
        "the setting must be untouched"
    );
    assert_ne!(theme_before, theme_after);
}

/// §2.1: an API key is never recorded — the screen doesn't hold it, so there is
/// nothing to restore, and a step would silently undo the *previous* edit instead.
#[test]
fn an_api_key_commit_records_no_undo_step() {
    let mut s = screen();
    // A cloud mode, so the "API key" field exists at all. Set up front, so the undo
    // snapshot taken below is consistent with it.
    s.config.engine.mode = crate::shared::config::ServerMode::OpenAi;
    goto_section(&mut s, Section::Interface);
    goto_field(&mut s, FieldId::ITheme);
    let before = s.config.interface.theme;
    s.handle_key(key(KeyCode::Right));

    goto_section(&mut s, Section::Model);
    goto_field(&mut s, FieldId::XApiKey);
    s.handle_key(key(KeyCode::Enter));
    for c in "sk-test".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    assert!(matches!(
        s.handle_key(key(KeyCode::Enter)),
        Some(SettingsIntent::SetSecret { .. })
    ));

    // The one step on the stack is still the theme edit, not the key.
    s.handle_key(ctrl('z'));
    assert_eq!(s.config.interface.theme, before);
}

/// U4: undoing a change made elsewhere moves the cursor onto it — otherwise the
/// revert happens invisibly.
#[test]
fn undo_jumps_to_the_field_it_reverted() {
    let mut s = screen();
    goto_section(&mut s, Section::Interface);
    goto_field(&mut s, FieldId::ITheme);
    s.handle_key(key(KeyCode::Right));

    // Walk away — a different section entirely.
    goto_section(&mut s, Section::Memory);
    assert_eq!(s.section(), Section::Memory);

    s.handle_key(ctrl('z'));
    assert_eq!(s.section(), Section::Interface, "jumped back to the change");
    assert_eq!(
        s.fields().get(s.field_idx).map(|f| f.id),
        Some(FieldId::ITheme)
    );
    assert!(s.focus == Focus::Fields);
}

/// The two pane markers (`▸` on the sections, `◆` on the parameters) are **always**
/// drawn and only their colour follows the focus. Both halves are asserted: the colours
/// swap, and the glyphs don't move — the menu marker used to appear and disappear, which
/// flickered and shifted the title text sideways on every focus change.
#[test]
fn pane_markers_stay_put_and_swap_colour_with_focus() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let palette = Palette::for_theme(Theme::Auto);
    let sections_glyph = palette.glyphs().collapsed;
    let params_glyph = palette.glyphs().title_marker;

    // (colour, position) of the single cell carrying the glyph.
    let marker = |s: &mut SettingsScreen, glyph: &str| -> (ratatui::style::Color, (u16, u16)) {
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
        let buf = term.backend().buffer().clone();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                if buf[(x, y)].symbol() == glyph {
                    return (buf[(x, y)].fg, (x, y));
                }
            }
        }
        panic!("marker {glyph:?} is not drawn at all");
    };

    let mut s = screen();
    let (menu_fg, menu_at) = marker(&mut s, sections_glyph);
    let (fields_fg, fields_at) = marker(&mut s, params_glyph);
    assert_eq!(menu_fg, palette.success, "sections hold the focus → green");
    assert_eq!(fields_fg, palette.muted, "parameters don't → muted");

    s.handle_key(key(KeyCode::Enter)); // focus moves into the field pane
    let (menu_fg2, menu_at2) = marker(&mut s, sections_glyph);
    let (fields_fg2, fields_at2) = marker(&mut s, params_glyph);
    assert_eq!(menu_fg2, palette.muted, "sections lost the focus → muted");
    assert_eq!(fields_fg2, palette.success, "parameters hold it → green");

    assert_eq!(menu_at, menu_at2, "the `▸` must not move");
    assert_eq!(fields_at, fields_at2, "the `◆` must not move");
}

/// The footer is contextual by focus (docs/history/settings-navigation.md §5.1) — that's the
/// only place the navigation model is stated: on the sections Enter goes in and Esc
/// closes; in the pane Esc steps back to the sections.
#[test]
fn footer_hints_differ_by_focus() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let footer = |s: &mut SettingsScreen| -> String {
        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
        let buf = term.backend().buffer().clone();
        // The hotkey rows sit below the panel — take the trailing ones (they start
        // with a space at column 0, panel rows carry the border character).
        let lines: Vec<String> = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect();
        lines
            .iter()
            .rev()
            .take_while(|l| l.starts_with(' '))
            .cloned()
            .collect::<Vec<_>>()
            .join(" ")
    };

    let mut s = screen();
    let menu = footer(&mut s);
    assert!(menu.contains("параметры"), "Enter → the pane: {menu:?}");
    assert!(menu.contains("закрыть"), "Esc closes: {menu:?}");
    assert!(
        !menu.contains("к секциям"),
        "nothing to step back to yet: {menu:?}"
    );

    s.handle_key(key(KeyCode::Enter));
    let fields = footer(&mut s);
    assert!(fields.contains("к секциям"), "Esc steps back: {fields:?}");
    assert!(!fields.contains("закрыть"), "not a close here: {fields:?}");
    assert!(fields.contains("сброс"), "Del is pane-only: {fields:?}");
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

/// The text of the fields pane (right of the 24-column section menu), rows joined by
/// a space and whitespace-collapsed — so a hint wrapped across rows can be matched as
/// one string. Deliberately not the whole screen: the panel's left border column would
/// land between the rows and break the match.
fn pane_text(s: &mut SettingsScreen, w: u16, h: u16) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
    let buf = term.backend().buffer().clone();
    let text: String = (0..buf.area.height)
        .map(|y| {
            // ...and short of the panel's right border, which would otherwise land
            // between the rows just like the left one.
            (25..buf.area.width.saturating_sub(1))
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" ");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The reported symptom: the API-key hint ran past the bottom panel's last row and was
/// cut mid-sentence. The panel is now as tall as the longest hint of the field set
/// needs, so the hint is shown whole — at a comfortable width and at a narrow one,
/// where it wraps into many more rows.
///
/// Both key rows are checked, because the panel has a row cap (`HINT_MAX_ROWS`) and the
/// **external** hint is the longer of the two: it has to name the two fields' priority
/// as well, and a hint that closes the door only works if its last sentence is on
/// screen.
#[test]
fn a_long_hint_is_shown_whole_in_the_bottom_panel() {
    for mode in [ServerMode::OpenAi, ServerMode::External] {
        let mut s = screen();
        s.config.engine.mode = mode;
        goto_field(&mut s, FieldId::XApiKey);
        let desc = norm(&field_desc(&s, FieldId::XApiKey).expect("api key is described"));
        for (w, h) in [(120u16, 40u16), (146, 46), (70, 40)] {
            let shown = pane_text(&mut s, w, h);
            assert!(
                shown.contains(&desc),
                "hint clipped at {w}x{h} in {mode:?}:\n  want: {desc}\n  got:  {shown}"
            );
        }
    }
}

/// The full-value preview shares the panel with the hint — and gives way to it: the
/// height is reserved for the hint, so a long value can only take what the hint leaves.
#[test]
fn a_long_value_preview_does_not_push_the_hint_out() {
    let mut s = screen();
    s.config.engine.mode = ServerMode::OpenAi;
    // Long enough that the preview alone would fill the panel and leave nothing.
    s.config.engine.openai.api_key_env = Some("VERY_LONG_ENVIRONMENT_VARIABLE_NAME_".repeat(12));
    goto_field(&mut s, FieldId::XApiKeyEnv);
    let desc = norm(&field_desc(&s, FieldId::XApiKeyEnv).expect("described"));
    let shown = pane_text(&mut s, 120, 40);
    assert!(
        shown.contains("VERY_LONG_ENVIRONMENT_VARIABLE_NAME_VERY_LONG"),
        "the preview is gone entirely: {shown}"
    );
    assert!(
        shown.contains(&desc),
        "hint clipped by the preview: {shown}"
    );
}

/// The panel's height: the longest hint of the field set (constant while stepping
/// between its fields — a per-field height would shift the list under the cursor),
/// never below the floor it has always had, never above the cap (an MCP server's tool
/// description is arbitrary text).
#[test]
fn hint_panel_height_follows_the_longest_hint_within_bounds() {
    let s = screen();
    let loc = s.loc();
    let rows = |fields: &[FieldRow], w: usize, cap: usize| hint_panel_rows(fields, w, cap, loc);

    let short = vec![row(FieldId::XPort, "p", FieldKind::Text("1".into()))];
    assert_eq!(rows(&short, 80, HINT_MAX_ROWS), HINT_MIN_ROWS, "floor");

    let long = vec![
        row(FieldId::XPort, "p", FieldKind::Text("1".into())),
        row(FieldId::XUrl, "u", FieldKind::Text(String::new())).describe("слово ".repeat(60)),
    ];
    let need = hint_rows(&long[1], 40, loc);
    assert!(need > HINT_MIN_ROWS, "the fixture must overflow the floor");
    assert_eq!(rows(&long, 40, HINT_MAX_ROWS), need, "grows to the longest");
    assert_eq!(rows(&long, 40, 4), 4, "and stops at the cap");
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
    // Grok (xAI Chat Completions): temperature/top_p/seed/max_tokens + reasoning.
    // The penalties are hidden because xAI answers 400 for them, top_k/min_p because
    // it drops them silently.
    s.config.engine.mode = ServerMode::Grok;
    assert!(has(&s, SamplingParam::Temp));
    assert!(has(&s, SamplingParam::TopP));
    assert!(has(&s, SamplingParam::Seed));
    assert!(has(&s, SamplingParam::MaxTokens));
    assert!(has(&s, SamplingParam::Reasoning));
    assert!(!has(&s, SamplingParam::FreqPen));
    assert!(!has(&s, SamplingParam::PresPen));
    assert!(!has(&s, SamplingParam::TopK));
    assert!(!has(&s, SamplingParam::MinP));
    assert!(!has(&s, SamplingParam::Verbosity));
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
    // The catalog renders the *localized* group title and falls back to
    // `ToolGroup::title()` only on a bundle miss, so resolve the expected set
    // the same way rather than comparing against the English fallbacks.
    let loc = s.loc();
    let known: Vec<&str> = crate::features::tools::meta::ToolGroup::ALL
        .iter()
        .map(|g| loc.get(g.i18n_key()).unwrap_or(g.title()))
        .collect();
    for r in &tool_rows {
        assert!(
            known.contains(&r.group),
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
    goto_section(&mut s, Section::Plugins);
    let fields = s.plugin_fields();
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
    // Enter does what the row needs: a ready server reconnects, one with a
    // changed catalog confirms it.
    assert_eq!(
        s.mcp_server_action(0),
        Some(SettingsIntent::ReconnectMcpServer("fs".into()))
    );
    assert_eq!(
        s.mcp_server_action(1),
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
fn mcp_master_toggle_lives_in_plugins_section() {
    // The "MCP servers" toggle heads the "Plugins" section: toggling it saves
    // the config. It moved out of "Tools" together with the server inventory.
    let mut s = screen();
    assert!(!s.config.mcp.enabled);
    assert!(
        !s.tool_fields().iter().any(|r| r.id == FieldId::TMcpEnabled),
        "the MCP switch left the Tools section"
    );
    goto_section(&mut s, Section::Plugins);
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

/// History compression (spec §6.7) is surfaced as the **first** group of "Memory" —
/// it is about the current conversation, ahead of the long-term memory groups.
#[test]
fn memory_section_opens_with_the_context_group() {
    let mut s = screen();
    goto_section(&mut s, Section::Memory);
    let rows = s.fields();
    let ids: Vec<FieldId> = rows.iter().map(|f| f.id).collect();
    for id in [
        FieldId::CompactEnabled,
        FieldId::CompactWords,
        FieldId::CompactTail,
    ] {
        assert!(ids.contains(&id), "\"Memory\" is missing {id:?}");
        assert!(
            field_desc(&s, id).is_some(),
            "{id:?} carries no description"
        );
    }
    // The group is the section's first, and the three fields are contiguous in it.
    let group = rows[0].group;
    assert_eq!(group, "Контекст", "the Context group opens the section");
    let ctx: Vec<FieldId> = rows
        .iter()
        .filter(|f| f.group == group)
        .map(|f| f.id)
        .collect();
    assert_eq!(
        ctx,
        vec![
            FieldId::CompactEnabled,
            FieldId::CompactWords,
            FieldId::CompactTail,
            FieldId::CompactThreshold,
            FieldId::CompactContext,
            FieldId::CompactPage
        ]
    );
    // …and it comes before the long-term memory groups.
    let rag_at = rows
        .iter()
        .position(|f| f.id == FieldId::RagTarget)
        .unwrap();
    let last_ctx = rows
        .iter()
        .rposition(|f| f.id == FieldId::CompactContext)
        .unwrap();
    assert!(last_ctx < rag_at, "Context sits ahead of the RAG group");
}

/// The master switch persists into the working config (on by default → off).
#[test]
fn compaction_toggle_persists() {
    let mut s = screen();
    assert!(s.config.compaction.enabled, "on by default");
    match s.toggle_field(FieldId::CompactEnabled) {
        Some(SettingsIntent::SaveConfig(c)) => assert!(!c.compaction.enabled),
        other => panic!("expected SaveConfig, got {other:?}"),
    }
    assert!(!s.config.compaction.enabled);
}

/// Both numeric fields commit a value and reject a non-numeric entry without
/// corrupting the config (they go through the `field_spec` access table, so
/// validation and `Del`-reset come from the same place as their neighbours).
#[test]
fn compaction_numeric_fields_commit_and_validate() {
    // Types a value into `id` after a rejected non-numeric entry; returns the
    // committed config so each field asserts on its own path.
    fn edit(id: FieldId, typed: &str) -> AppConfig {
        let mut s = screen();
        goto_section(&mut s, Section::Memory);
        goto_field(&mut s, id);
        assert_eq!(field_num_kind(id), Some(NumKind::Int), "{id:?} is an int");

        // A non-numeric entry keeps the editor open and flags the error.
        s.handle_key(key(KeyCode::Enter));
        s.handle_key(ctrl('k'));
        s.handle_key(key(KeyCode::Char('x')));
        assert!(s.handle_key(key(KeyCode::Enter)).is_none(), "{id:?}");
        assert!(s.editor.as_ref().is_some_and(|e| e.error.is_some()));

        // Fixing it commits normally.
        s.handle_key(ctrl('k'));
        for c in typed.chars() {
            s.handle_key(key(KeyCode::Char(c)));
        }
        match s.handle_key(key(KeyCode::Enter)) {
            Some(SettingsIntent::SaveConfig(c)) => *c,
            other => panic!("{id:?}: expected SaveConfig, got {other:?}"),
        }
    }

    assert_eq!(
        edit(FieldId::CompactWords, "400").compaction.summary_words,
        400
    );
    assert_eq!(
        edit(FieldId::CompactTail, "4096").compaction.tail_tokens,
        4096
    );
    assert_eq!(
        edit(FieldId::CompactThreshold, "60")
            .compaction
            .threshold_pct,
        60
    );
    // Clamped rather than refused: a threshold above 100 could never fire, which
    // reads as "the feature is broken" instead of as a rejected entry.
    assert_eq!(
        edit(FieldId::CompactThreshold, "150")
            .compaction
            .threshold_pct,
        100
    );
    // 0 is how a numeric field spells "work it out yourself" — an empty entry
    // already means "keep the previous value", so it cannot mean this too.
    assert_eq!(
        edit(FieldId::CompactContext, "0").compaction.context_tokens,
        None
    );
    assert_eq!(
        edit(FieldId::CompactContext, "32768")
            .compaction
            .context_tokens,
        Some(32768)
    );
}

/// The `history_read` page size is its own knob (S14): editing it commits into
/// `compaction.page_tokens` and, like its neighbours in the group, goes through
/// the `field_spec` access table as an integer.
#[test]
fn compaction_page_size_commits() {
    let mut s = screen();
    goto_section(&mut s, Section::Memory);
    goto_field(&mut s, FieldId::CompactPage);
    assert_eq!(field_num_kind(FieldId::CompactPage), Some(NumKind::Int));

    s.handle_key(key(KeyCode::Enter));
    s.handle_key(ctrl('k'));
    for c in "1200".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    match s.handle_key(key(KeyCode::Enter)) {
        Some(SettingsIntent::SaveConfig(c)) => assert_eq!(c.compaction.page_tokens, 1200),
        other => panic!("expected SaveConfig, got {other:?}"),
    }
    assert_eq!(s.config.compaction.page_tokens, 1200);
}

/// The projector sits directly under the GGUF path, in the same "Model" group and
/// on both managed engines: it is the second half of the same download, and a user
/// who found one has found the other.
#[test]
fn managed_model_group_pairs_the_projector_with_the_gguf() {
    let mut s = screen();
    // Impersonation defaults to `shared` (no server of its own), and then its tab
    // shows no managed rows at all — put it in managed mode so the pair is there to
    // check on both engines.
    s.config.impersonation_engine.mode = ImpersonationMode::Managed;
    for (tab, model, mmproj) in [
        (ModelTab::Assistant, FieldId::XModel, FieldId::XMmproj),
        (ModelTab::Impersonation, FieldId::IxModel, FieldId::IxMmproj),
    ] {
        let fields = s.model_fields_for(tab);
        let at = |id: FieldId| fields.iter().position(|f| f.id == id);
        let (mi, pi) = (
            at(model).unwrap_or_else(|| panic!("{model:?} missing")),
            at(mmproj).unwrap_or_else(|| panic!("{mmproj:?} missing")),
        );
        assert_eq!(pi, mi + 1, "{mmproj:?} must follow {model:?}");
        assert_eq!(
            fields[pi].group, fields[mi].group,
            "the projector must share the GGUF's group"
        );
        assert!(
            field_desc(&s, mmproj).is_some(),
            "{mmproj:?} has no description"
        );
    }
}

/// Editing the projector row writes `managed.mmproj` — and an empty entry clears it
/// back to `None`, which is how a vision setup is turned off again.
#[test]
fn projector_path_commits_and_clears() {
    let mut s = screen();
    goto_section(&mut s, Section::Model);
    goto_field(&mut s, FieldId::XMmproj);
    s.handle_key(key(KeyCode::Enter));
    s.handle_key(ctrl('k'));
    for c in "mmproj-gemma.gguf".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    match s.handle_key(key(KeyCode::Enter)) {
        Some(SettingsIntent::SaveConfig(c)) => {
            assert_eq!(
                c.engine.managed.mmproj.as_deref(),
                Some("mmproj-gemma.gguf")
            )
        }
        other => panic!("expected SaveConfig, got {other:?}"),
    }

    goto_field_again(&mut s, FieldId::XMmproj);
    s.handle_key(key(KeyCode::Enter));
    s.handle_key(ctrl('k'));
    match s.handle_key(key(KeyCode::Enter)) {
        Some(SettingsIntent::SaveConfig(c)) => assert_eq!(c.engine.managed.mmproj, None),
        other => panic!("expected SaveConfig, got {other:?}"),
    }
}

/// The three image rows share their own group, sit right after the file-attachment
/// budgets, and each carries a description.
#[test]
fn image_group_follows_the_attachment_budgets() {
    let s = screen();
    let fields = s.memory_fields();
    let ids: Vec<FieldId> = fields
        .iter()
        .filter(|f| {
            matches!(
                f.id,
                FieldId::AttachPage
                    | FieldId::ImageMaxCount
                    | FieldId::ImageMaxBytes
                    | FieldId::ImageDownscale
            )
        })
        .map(|f| f.id)
        .collect();
    assert_eq!(
        ids,
        vec![
            FieldId::AttachPage,
            FieldId::ImageMaxCount,
            FieldId::ImageMaxBytes,
            FieldId::ImageDownscale,
        ],
        "the image rows follow the attachment budgets"
    );
    let group = |id: FieldId| fields.iter().find(|f| f.id == id).unwrap().group;
    let images = group(FieldId::ImageMaxCount);
    assert_ne!(
        images,
        group(FieldId::AttachPage),
        "images are their own group — tokens and megabytes are not one scale"
    );
    for id in [
        FieldId::ImageMaxCount,
        FieldId::ImageMaxBytes,
        FieldId::ImageDownscale,
    ] {
        assert_eq!(group(id), images, "{id:?} left the images group");
        assert!(field_desc(&s, id).is_some(), "{id:?} has no description");
    }
}

/// The image budgets commit through the `field_spec` access table like their
/// neighbours — and the size limit converts **both ways**: the row is megabytes, the
/// config is bytes, so a value typed in must come back out of the row unchanged.
#[test]
fn image_budgets_commit_and_the_size_limit_round_trips_through_mb() {
    fn edit(id: FieldId, typed: &str) -> (AppConfig, SettingsScreen) {
        let mut s = screen();
        goto_section(&mut s, Section::Memory);
        goto_field(&mut s, id);
        assert_eq!(field_num_kind(id), Some(NumKind::Int), "{id:?} is an int");

        // A non-numeric entry keeps the editor open and flags the error.
        s.handle_key(key(KeyCode::Enter));
        s.handle_key(ctrl('k'));
        s.handle_key(key(KeyCode::Char('x')));
        assert!(s.handle_key(key(KeyCode::Enter)).is_none(), "{id:?}");
        assert!(s.editor.as_ref().is_some_and(|e| e.error.is_some()));

        s.handle_key(ctrl('k'));
        for c in typed.chars() {
            s.handle_key(key(KeyCode::Char(c)));
        }
        match s.handle_key(key(KeyCode::Enter)) {
            Some(SettingsIntent::SaveConfig(c)) => (*c, s),
            other => panic!("{id:?}: expected SaveConfig, got {other:?}"),
        }
    }

    assert_eq!(edit(FieldId::ImageMaxCount, "3").0.images.max_count, 3);
    assert_eq!(
        edit(FieldId::ImageDownscale, "1024").0.images.downscale_px,
        1024
    );

    // The row is in MB, the config in bytes.
    let (cfg, s) = edit(FieldId::ImageMaxBytes, "5");
    assert_eq!(cfg.images.max_bytes, 5 * BYTES_PER_MB);
    // ...and the row shows what was typed, not the byte count — the value has to
    // survive a redraw, or the next edit would be seeded with megabytes-worth of
    // bytes and multiply the limit again.
    let shown = s
        .memory_fields()
        .into_iter()
        .find(|f| f.id == FieldId::ImageMaxBytes)
        .map(|f| value_text(&f.kind, s.loc()))
        .expect("the size-limit row");
    assert_eq!(shown, "5");

    // Zero is not a limit anyone means — it would refuse every image ever attached.
    assert_eq!(
        edit(FieldId::ImageMaxBytes, "0").0.images.max_bytes,
        BYTES_PER_MB
    );
}

/// The default row shows the shipped ceiling in megabytes, so the units the
/// description promises are the units the user sees before touching anything.
#[test]
fn the_default_size_limit_is_shown_in_megabytes() {
    assert_eq!(
        bytes_to_mb(crate::shared::config::DEFAULT_IMAGE_MAX_BYTES),
        10
    );
    assert_eq!(
        mb_to_bytes(10),
        crate::shared::config::DEFAULT_IMAGE_MAX_BYTES
    );
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
        warn_note: None,
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
        warn_note: None,
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
    // "Tools" now carries an extra "Video (YouTube)" group above "Files" — tall enough
    // that every group (through "Files") stays on screen.
    let mut term = Terminal::new(TestBackend::new(100, 40)).unwrap();
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

/// The "API key" field shows a **status**, not a secret; a stored key is reflected as
/// the value "configured". It exists for every mode that can need a key — the clouds
/// and `external` (see [`external_key_field_targets_its_own_slot`]) — but not for a
/// managed server, which is a local process with no authorization at all.
#[test]
fn api_key_field_shows_status_in_cloud_modes_only() {
    let mut s = screen();
    goto_section(&mut s, Section::Model);
    let has_key_field = |s: &SettingsScreen| s.fields().iter().any(|f| f.id == FieldId::XApiKey);
    // A local managed engine — no key field (nothing to authenticate to).
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
    s.set_secrets_present(vec![SecretKey::Provider(CloudProvider::OpenAi)]);
    let set = value(&s);
    assert_ne!(
        unset, set,
        "the field's status doesn't reflect the key's presence"
    );
    // Another provider's key doesn't affect OpenAI's status.
    s.set_secrets_present(vec![SecretKey::Provider(CloudProvider::Claude)]);
    assert_eq!(value(&s), unset);
    // The secret itself never appears in the field's value under any status.
    assert!(!set.contains("sk-"));
}

/// The backup password behaves exactly like an API key: a status instead of the
/// value, an empty masked editor, a commit as its own intent, and `Del` to clear
/// it. Spec §12.3.
#[test]
fn backup_password_field_is_a_masked_secret_with_its_own_intent() {
    let mut s = screen();
    goto_section(&mut s, Section::Data);
    goto_field(&mut s, FieldId::BackupPassword);

    let value = |s: &SettingsScreen| {
        s.fields()
            .into_iter()
            .find(|f| f.id == FieldId::BackupPassword)
            .map(|f| match f.kind {
                FieldKind::Text(v) => v,
                _ => panic!("the backup password should be a text field"),
            })
            .expect("the field exists in the Data section")
    };
    let unset = value(&s);
    s.set_secrets_present(vec![SecretKey::BackupPassword]);
    assert_ne!(unset, value(&s), "the status doesn't follow the flag");

    // Editing opens empty and masked — a stored password can't be shown.
    s.handle_key(key(KeyCode::Enter));
    let editor = s.editor.as_ref().expect("the editor is open");
    assert_eq!(editor.input.text(), "");
    assert!(editor.input.is_masked());

    for c in "hunter2".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    assert_eq!(
        s.handle_key(key(KeyCode::Enter)),
        Some(SettingsIntent::SetSecret {
            key: SecretKey::BackupPassword,
            value: "hunter2".into()
        })
    );
    // The secret never reaches the screen's config.
    let json = serde_json::to_string(&s.config).unwrap();
    assert!(!json.contains("hunter2"), "the password leaked: {json}");

    // `Del` clears a stored password, and is a no-op when there is none.
    assert_eq!(
        s.handle_key(key(KeyCode::Delete)),
        Some(SettingsIntent::SetSecret {
            key: SecretKey::BackupPassword,
            value: String::new()
        })
    );
    s.set_secrets_present(Vec::new());
    assert_eq!(s.handle_key(key(KeyCode::Delete)), None);
}

/// Editing the key field opens an **empty** masked editor (a stored key
/// can't be shown), and the commit goes as a separate intent — not into the screen's config.
#[test]
fn api_key_editor_is_masked_empty_and_commits_intent() {
    let mut s = screen();
    s.config.engine.mode = ServerMode::Claude;
    s.set_secrets_present(vec![SecretKey::Provider(CloudProvider::Claude)]); // the key is already stored
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
        Some(SettingsIntent::SetSecret {
            key: SecretKey::Provider(CloudProvider::Claude),
            value: "sk-ant-123".into()
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

    s.set_secrets_present(vec![SecretKey::Provider(CloudProvider::Gemini)]);
    assert_eq!(
        s.handle_key(key(KeyCode::Delete)),
        Some(SettingsIntent::SetSecret {
            key: SecretKey::Provider(CloudProvider::Gemini),
            value: String::new()
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
        s.secret_field_key(FieldId::XApiKey),
        Some(SecretKey::Provider(CloudProvider::OpenAi))
    );
    assert_eq!(
        s.secret_field_key(FieldId::IxApiKey),
        Some(SecretKey::Provider(CloudProvider::Claude))
    );
    assert_eq!(
        s.secret_field_key(FieldId::EApiKey),
        Some(SecretKey::Provider(CloudProvider::Gemini))
    );
    assert_eq!(s.secret_field_key(FieldId::XUrl), None);
}

/// In `external` mode every slot shows the key row too — the barrier ADR 0008 removed
/// for the clouds stood in the one mode whose URL is typed by hand — and each addresses
/// **its own** slot rather than sharing one key: the four `external` URLs are four
/// independent servers (docs/history/external-api-key.md F1). The row's behaviour is the cloud
/// row's (status, masked empty editor, `SetSecret`, `Del`), which is why it reuses the
/// same field ids; what is new is which secret those ids resolve to.
#[test]
fn external_key_field_targets_its_own_slot() {
    let mut s = screen();
    s.config.engine.mode = ServerMode::External;
    s.config.impersonation_engine.mode = ImpersonationMode::External;
    s.config.embed.mode = ServerMode::External;
    s.config.tts.mode = crate::shared::config::TtsMode::External;
    for (field, slot) in [
        (FieldId::XApiKey, ExternalSlot::Chat),
        (FieldId::IxApiKey, ExternalSlot::Impersonation),
        (FieldId::EApiKey, ExternalSlot::Embed),
        (FieldId::TtsApiKey, ExternalSlot::Tts),
    ] {
        assert_eq!(
            s.secret_field_key(field),
            Some(SecretKey::External(slot)),
            "{field:?} must address its own external slot"
        );
    }

    // The row is built, and its status follows this slot's key — not another slot's.
    goto_section(&mut s, Section::Model);
    let value = |s: &SettingsScreen| {
        s.fields()
            .into_iter()
            .find(|f| f.id == FieldId::XApiKey)
            .map(|f| value_text(&f.kind, s.loc()))
            .expect("external mode shows the key row")
    };
    let unset = value(&s);
    s.set_secrets_present(vec![SecretKey::External(ExternalSlot::Embed)]);
    assert_eq!(value(&s), unset, "another slot's key is not this slot's");
    s.set_secrets_present(vec![SecretKey::External(ExternalSlot::Chat)]);
    assert_ne!(value(&s), unset, "the status must follow this slot's key");

    // Editing commits as a secret intent for the slot — never into the config.
    goto_field(&mut s, FieldId::XApiKey);
    assert_eq!(
        enter_secret(&mut s, "sk-gateway"),
        Some(SettingsIntent::SetSecret {
            key: SecretKey::External(ExternalSlot::Chat),
            value: "sk-gateway".into()
        })
    );
    let json = serde_json::to_string(&s.config).unwrap();
    assert!(
        !json.contains("sk-gateway"),
        "the key leaked into the screen's config: {json}"
    );
    // The env-name row stays beside it: it is still the route for CI and for a machine
    // where key storage is unavailable (docs/history/external-api-key.md F4).
    assert!(s.fields().iter().any(|f| f.id == FieldId::XApiKeyEnv));
}

/// The video slot's key row stores the **Gemini** key whatever the engines are set
/// to — and that is the point of it existing. The "Model" section shows a key field
/// only for a slot whose mode is that cloud, so on a local or OpenAI setup there was
/// nowhere to enter a Gemini key at all, while `youtube_watch` needs one regardless
/// of the chat engine. Found by a user who had no way to configure the tool.
#[test]
fn video_key_field_targets_gemini_whatever_the_engines_are() {
    let mut s = screen();
    s.config.engine.mode = ServerMode::OpenAi;
    s.config.impersonation_engine.mode = ImpersonationMode::Shared;
    s.config.embed.mode = ServerMode::External;
    assert_eq!(
        s.secret_field_key(FieldId::VideoApiKey),
        Some(SecretKey::Provider(CloudProvider::Gemini)),
        "no engine is on Gemini, yet the video key must still address Gemini"
    );

    goto_section(&mut s, Section::Tools);
    goto_field(&mut s, FieldId::VideoApiKey);
    s.handle_key(key(KeyCode::Enter));
    let editor = s.editor.as_ref().expect("the key field's editor is open");
    assert_eq!(editor.input.text(), "", "a stored key can't be shown");
    assert!(editor.input.is_masked(), "the secret field must be masked");
    for c in "AIzaSy-test".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    assert_eq!(
        s.handle_key(key(KeyCode::Enter)),
        Some(SettingsIntent::SetSecret {
            key: SecretKey::Provider(CloudProvider::Gemini),
            value: "AIzaSy-test".into()
        })
    );
    let json = serde_json::to_string(&s.config).unwrap();
    assert!(
        !json.contains("AIzaSy-test"),
        "the key leaked into the screen's config: {json}"
    );
}

/// The row reports whether a key is stored, and `Del` deletes it — with nothing
/// stored there is nothing to delete.
#[test]
fn video_key_row_shows_status_and_del_removes_it() {
    let mut s = screen();
    goto_section(&mut s, Section::Tools);
    goto_field(&mut s, FieldId::VideoApiKey);
    assert_eq!(s.handle_key(key(KeyCode::Delete)), None);

    s.set_secrets_present(vec![SecretKey::Provider(CloudProvider::Gemini)]);
    let want =
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru).t("ui.settings.value.key_set");
    assert!(
        s.fields()
            .iter()
            .any(|f| f.id == FieldId::VideoApiKey
                && matches!(&f.kind, FieldKind::Text(v) if v == want)),
        "the row must report the key as stored"
    );
    assert_eq!(
        s.handle_key(key(KeyCode::Delete)),
        Some(SettingsIntent::SetSecret {
            key: SecretKey::Provider(CloudProvider::Gemini),
            value: String::new()
        })
    );
}

/// User data (a profile, an impersonation persona) has no "default value", so it must
/// never get the `•` "modified" marker — comparing it against a default config would
/// flag a chosen persona simply because the default config has no personas at all.
#[test]
fn profile_and_persona_rows_are_never_marked_modified() {
    use ratatui::{Terminal, backend::TestBackend};
    let mut s = screen();
    s.config.impersonation_profiles = vec![crate::shared::config::ImpersonationProfile::new(
        "Владимир",
        "Ты — Владимир.",
    )];
    s.profiles[0].impersonation_profile_id = Some(s.config.impersonation_profiles[0].id);
    goto_section(&mut s, Section::Profiles);
    s.handle_key(key(KeyCode::Enter));

    let marked = |s: &mut SettingsScreen| {
        let mut term = Terminal::new(TestBackend::new(96, 26)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..26).any(|y| (0..96).any(|x| buf[(x, y)].symbol() == "•"))
    };
    assert!(
        !marked(&mut s),
        "the assistant subsection marked user data as modified"
    );
    goto_impersonation_sub(&mut s);
    assert!(
        !marked(&mut s),
        "the impersonation subsection marked user data as modified"
    );
}

/// Types `text` into the field's editor and commits it.
fn type_into(s: &mut SettingsScreen, id: FieldId, text: &str) -> Option<SettingsIntent> {
    goto_field_again(s, id);
    s.handle_key(key(KeyCode::Enter)); // open the editor
    s.handle_key(ctrl('k')); // clear the seed
    s.handle_paste(text);
    s.handle_key(key(KeyCode::Enter))
}

/// A stored-value row appears for **each** variable the selected server
/// declares, reports whether this machine holds it, and commits as a
/// `SetSecret` addressed to (server, variable) — never into the screen's config
/// (docs/history/mcp-server-editor.md §9, S1).
#[test]
fn mcp_env_secret_rows_follow_the_declared_variables() {
    let mut s = screen();
    goto_section(&mut s, Section::Plugins);
    s.handle_key(ctrl('n'));
    assert!(type_into(&mut s, FieldId::McpId, "gh").is_some());
    // No variables declared — no value rows.
    assert!(
        !s.plugin_fields()
            .iter()
            .any(|r| matches!(r.id, FieldId::McpEnvSecret(_))),
        "a server with no env has nothing to store"
    );

    assert!(type_into(&mut s, FieldId::McpEnv, "GITHUB_TOKEN, OTHER, FROM_OS=SRC").is_some());
    let all = s.plugin_fields();
    let rows: Vec<&FieldRow> = all
        .iter()
        .filter(|r| matches!(r.id, FieldId::McpEnvSecret(_)))
        .collect();
    assert_eq!(
        rows.len(),
        2,
        "a row per variable declared by name alone — the one naming a source has \
         its answer already"
    );
    // The label is the variable name, in the map's order.
    assert_eq!(rows[0].label, "GITHUB_TOKEN");
    assert_eq!(rows[1].label, "OTHER");
    assert!(
        !rows.iter().any(|r| r.label == "FROM_OS"),
        "offering to store a value for a variable that names a source would be two \
         answers to one question (§9.5b)"
    );

    let ru = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
    assert!(
        matches!(&rows[0].kind, FieldKind::Text(v) if v == ru.t("ui.settings.value.key_unset")),
        "nothing stored yet"
    );
    s.set_secrets_present(vec![SecretKey::McpEnv {
        server: "gh".into(),
        var: "GITHUB_TOKEN".into(),
    }]);
    let rows = s.plugin_fields();
    // Index 1: the map is sorted, so FROM_OS is 0 — and it has no row, which is
    // exactly why the index must stay the position in the whole map.
    let stored = rows
        .iter()
        .find(|r| r.id == FieldId::McpEnvSecret(1))
        .unwrap();
    assert!(
        matches!(&stored.kind, FieldKind::Text(v) if v == ru.t("ui.settings.value.key_set")),
        "the row must report the value as stored"
    );

    // Editing: an empty masked editor, and the commit carries the secret out of
    // the screen rather than into its config. The index is the position in the
    // whole map (`OTHER` is second alphabetically: FROM_OS, GITHUB_TOKEN, OTHER),
    // so skipping a row never shifts the addressing.
    goto_field_again(&mut s, FieldId::McpEnvSecret(2));
    s.handle_key(key(KeyCode::Enter));
    let editor = s.editor.as_ref().expect("the editor is open");
    assert_eq!(editor.input.text(), "", "a stored value cannot be shown");
    assert!(editor.input.is_masked());
    for c in "ghp-live-token".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    assert_eq!(
        s.handle_key(key(KeyCode::Enter)),
        Some(SettingsIntent::SetSecret {
            key: SecretKey::McpEnv {
                server: "gh".into(),
                var: "OTHER".into()
            },
            value: "ghp-live-token".into()
        })
    );
    let json = serde_json::to_string(&s.config).unwrap();
    assert!(
        !json.contains("ghp-live-token"),
        "the secret leaked into the screen's config: {json}"
    );
}

/// The bottom panel's gate explanation belongs to a **gated tool**, not to every
/// flagged row. `warn` is raised for several unrelated reasons — a changed MCP
/// catalog, a missing environment source — and keying one fixed sentence off the
/// flag itself told the user "the tool is enabled in the profile but disabled by
/// a global switch" on rows that are not tools at all (reported from a live run).
///
/// It also names the section that actually holds the switch: the MCP master
/// toggle moved to "Plugins" when the server editor got its own section, so the
/// hardcoded "Tools" was wrong for it.
#[test]
fn the_gate_explanation_is_only_on_gated_tools_and_names_its_section() {
    let mut s = screen();

    // A flagged row that is not a gated tool carries no explanation.
    goto_section(&mut s, Section::Plugins);
    s.handle_key(ctrl('n'));
    assert!(type_into(&mut s, FieldId::McpEnv, "ABSENT=MINDFORK_TEST_NO_SUCH_VAR").is_some());
    let rows = s.plugin_fields();
    let source_row = rows
        .iter()
        .find(|r| matches!(r.id, FieldId::McpEnvSource(_)))
        .expect("the status row");
    assert!(source_row.warn, "a missing source is worth flagging");
    assert!(
        source_row.warn_note.is_none(),
        "…but it has nothing to do with a global tool gate"
    );
    // And its description substitutes the source name rather than showing {src}.
    let desc = source_row.description.as_deref().unwrap_or_default();
    assert!(
        desc.contains("MINDFORK_TEST_NO_SUCH_VAR") && !desc.contains("{src}"),
        "the description must name the source: {desc}"
    );

    // A tool that *is* gated carries the explanation, naming the right section.
    s.config.mcp.enabled = false;
    let mcp_tool = crate::features::tools::meta::ToolInfo {
        id: "mcp__gh__issues".to_string(),
        group: crate::features::tools::meta::ToolGroup::Plugins,
        label: "issues",
        gate: Some(ToolGate::Mcp),
        enabled_by_default: false,
        description: None,
    };
    s.set_mcp(crate::features::tools::mcp::McpSnapshot {
        tools: vec![mcp_tool],
        servers: Vec::new(),
    });
    let idx = s
        .tool_catalog()
        .iter()
        .position(|i| i.id == "mcp__gh__issues")
        .unwrap();
    s.profiles[0].enabled_tools.push("mcp__gh__issues".into());
    goto_section(&mut s, Section::Profiles);
    let gated = s
        .profile_fields()
        .into_iter()
        .find(|r| r.id == FieldId::PTool(idx))
        .expect("the tool toggle");
    let note = gated.warn_note.expect("a gated tool explains itself");
    let ru = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
    assert!(
        note.contains(ru.t("ui.settings.section.plugins")),
        "the MCP switch lives in \"Plugins\", not \"Tools\": {note}"
    );
    assert!(
        !note.contains("{section}"),
        "unsubstituted placeholder: {note}"
    );

    // A web tool still points at "Tools", where its switch really is.
    s.config.tools.web_enabled = false;
    let web = s
        .tool_catalog()
        .iter()
        .position(|i| i.id == "web_search")
        .unwrap();
    s.profiles[0].enabled_tools.push("web_search".into());
    let note = s
        .profile_fields()
        .into_iter()
        .find(|r| r.id == FieldId::PTool(web))
        .and_then(|r| r.warn_note)
        .expect("a gated tool explains itself");
    assert!(note.contains(ru.t("ui.settings.section.tools")), "{note}");
}

/// A variable that names a source gets a **read-only status** instead of a value
/// field: whether that OS variable is actually there. Without it the two routes
/// are asymmetric — the stored one reports "configured / not set" while a named
/// source reports nothing, and a missing variable shows up only as the server
/// failing to work (docs/history/mcp-server-editor.md §9.5c).
#[test]
fn a_sourced_variable_shows_whether_its_source_exists() {
    let mut s = screen();
    goto_section(&mut s, Section::Plugins);
    s.handle_key(ctrl('n'));
    // SAFETY: single-threaded test, scoped to this process.
    unsafe { std::env::set_var("MINDFORK_TEST_SETTINGS_SRC", "x") };
    assert!(
        type_into(
            &mut s,
            FieldId::McpEnv,
            "PRESENT=MINDFORK_TEST_SETTINGS_SRC, ABSENT=MINDFORK_TEST_NO_SUCH_VAR",
        )
        .is_some()
    );

    let rows = s.plugin_fields();
    let row_of = |idx: usize| {
        rows.iter()
            .find(|r| r.id == FieldId::McpEnvSource(idx))
            .unwrap_or_else(|| panic!("no status row for variable {idx}"))
    };
    // Map order: ABSENT(0), PRESENT(1).
    let absent = row_of(0);
    let present = row_of(1);
    assert_eq!(absent.label, "ABSENT");
    assert_eq!(present.label, "PRESENT");
    assert!(
        matches!(&present.kind, FieldKind::Text(v) if v.contains("MINDFORK_TEST_SETTINGS_SRC")
            && v.contains(&ru_t("ui.settings.value.mcp_source_found"))),
        "the row must name the source and say it was found"
    );
    assert!(!present.warn);
    assert!(
        matches!(&absent.kind, FieldKind::Text(v)
            if v.contains(&ru_t("ui.settings.value.mcp_source_missing"))),
        "a missing source must be visible"
    );
    assert!(absent.warn, "a missing source is worth flagging");

    // Neither variable offers to store a value, and the status row is read-only.
    assert!(
        !rows
            .iter()
            .any(|r| matches!(r.id, FieldId::McpEnvSecret(_))),
        "a variable that names a source has its origin already"
    );
    goto_field_again(&mut s, FieldId::McpEnvSource(1));
    assert_eq!(s.handle_key(key(KeyCode::Enter)), None);
    assert!(s.editor.is_none(), "there is nothing to edit here");
}

/// The value part of a localized row, for asserting on a rendered value that
/// carries an interpolated source name.
fn ru_t(key: &str) -> String {
    let ru = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
    // The templates are "{src}: found" / "{src}: not found" — compare on the tail
    // after the placeholder, which is the part that distinguishes them.
    ru.t(key).rsplit("{src}").next().unwrap_or("").to_string()
}

/// `Del` on a stored value deletes it; with nothing stored there is nothing to
/// delete (the API-key row's behaviour, reached through the same field).
#[test]
fn del_on_an_env_secret_row_removes_the_stored_value() {
    let mut s = screen();
    goto_section(&mut s, Section::Plugins);
    s.handle_key(ctrl('n'));
    assert!(type_into(&mut s, FieldId::McpId, "gh").is_some());
    assert!(type_into(&mut s, FieldId::McpEnv, "TOKEN=").is_some());
    goto_field_again(&mut s, FieldId::McpEnvSecret(0));
    assert_eq!(s.handle_key(key(KeyCode::Delete)), None);

    s.set_secrets_present(vec![SecretKey::McpEnv {
        server: "gh".into(),
        var: "TOKEN".into(),
    }]);
    assert_eq!(
        s.handle_key(key(KeyCode::Delete)),
        Some(SettingsIntent::SetSecret {
            key: SecretKey::McpEnv {
                server: "gh".into(),
                var: "TOKEN".into()
            },
            value: String::new()
        })
    );
}

/// A variable name we cannot carry is dropped by the row's parser — it would
/// break both the flat `VAR=SOURCE` text and the secret's storage name.
#[test]
fn env_row_drops_names_it_cannot_carry() {
    let mut s = screen();
    goto_section(&mut s, Section::Plugins);
    s.handle_key(ctrl('n'));
    assert!(type_into(&mut s, FieldId::McpEnv, "OK_1=A, has-dash=B, 1BAD=C").is_some());
    let keys: Vec<&String> = s.config.mcp.servers[0].env.keys().collect();
    assert_eq!(keys, vec!["OK_1"]);
}

/// The import row commits a **path** (the orchestrator reads and parses the
/// file), and shows the last outcome as its value.
#[test]
fn mcp_import_row_commits_a_path_and_shows_the_outcome() {
    let mut s = screen();
    goto_section(&mut s, Section::Plugins);
    assert_eq!(
        type_into(
            &mut s,
            FieldId::McpImport,
            "D:/cfg/claude_desktop_config.json"
        ),
        Some(SettingsIntent::ImportMcpServers(
            "D:/cfg/claude_desktop_config.json".into()
        ))
    );
    // An empty path does nothing.
    assert_eq!(type_into(&mut s, FieldId::McpImport, ""), None);

    s.set_mcp_import_result("Импортировано серверов: 2".into());
    let row = s
        .plugin_fields()
        .into_iter()
        .find(|r| r.id == FieldId::McpImport)
        .unwrap();
    assert!(matches!(&row.kind, FieldKind::Text(v) if v.contains("2")));
    // …and the outcome is not offered back as the value to edit.
    goto_field_again(&mut s, FieldId::McpImport);
    s.handle_key(key(KeyCode::Enter));
    assert_eq!(s.editor.as_ref().unwrap().input.text(), "");
}

#[test]
fn mcp_server_create_edit_delete_round_trip() {
    // The whole point of the track: a server is authored without touching
    // settings.json (docs/history/mcp-server-editor.md).
    let mut s = screen();
    goto_section(&mut s, Section::Plugins);
    assert!(s.config.mcp.servers.is_empty());
    // The empty inventory shows only the selector's placeholder.
    assert!(
        s.plugin_fields()
            .iter()
            .all(|r| r.id != FieldId::McpCommand)
    );

    assert!(matches!(
        s.handle_key(ctrl('n')),
        Some(SettingsIntent::SaveConfig(_))
    ));
    let srv = &s.config.mcp.servers[0];
    assert_eq!(srv.id, "server-1");
    assert!(
        !srv.enabled,
        "a new server is off — nothing spawns while the command is half-typed (F5)"
    );
    assert_eq!(s.mcp_server_idx, 0);

    assert!(type_into(&mut s, FieldId::McpId, "fs").is_some());
    assert!(type_into(&mut s, FieldId::McpCommand, "cmd").is_some());
    assert!(
        type_into(
            &mut s,
            FieldId::McpArgs,
            r#"/c npx -y @modelcontextprotocol/server-filesystem "D:/my work""#,
        )
        .is_some()
    );
    assert!(type_into(&mut s, FieldId::McpEnv, "GITHUB_TOKEN=MINDFORK_PAT").is_some());
    assert!(type_into(&mut s, FieldId::McpTimeout, "90").is_some());
    let srv = &s.config.mcp.servers[0];
    assert_eq!(srv.id, "fs");
    assert_eq!(srv.command, "cmd");
    assert_eq!(
        srv.args,
        [
            "/c",
            "npx",
            "-y",
            "@modelcontextprotocol/server-filesystem",
            "D:/my work"
        ]
    );
    assert_eq!(srv.env["GITHUB_TOKEN"], "MINDFORK_PAT");
    assert_eq!(srv.tool_timeout_secs, 90);

    // Enabling it is a separate, deliberate act.
    goto_field_again(&mut s, FieldId::McpEnabled);
    s.handle_key(key(KeyCode::Char(' ')));
    assert!(s.config.mcp.servers[0].enabled);

    // A second server gets a free id, and Ctrl+D removes the selected one.
    s.handle_key(ctrl('n'));
    assert_eq!(s.config.mcp.servers[1].id, "server-1");
    assert!(matches!(
        s.handle_key(ctrl('d')),
        Some(SettingsIntent::SaveConfig(_))
    ));
    assert_eq!(s.config.mcp.servers.len(), 1);
    assert_eq!(s.mcp_server_idx, 0);
    s.handle_key(ctrl('d'));
    assert!(s.config.mcp.servers.is_empty());
    assert!(s.handle_key(ctrl('d')).is_none(), "nothing left to delete");
}

#[test]
fn mcp_id_must_be_a_unique_slug() {
    // An invalid id makes the host create no slot at all, so without this the
    // server would simply vanish from the status list (F8).
    let mut s = screen();
    goto_section(&mut s, Section::Plugins);
    s.handle_key(ctrl('n'));
    type_into(&mut s, FieldId::McpId, "fs");
    s.handle_key(ctrl('n'));

    for bad in ["Files", "my server", ""] {
        assert!(
            type_into(&mut s, FieldId::McpId, bad).is_none(),
            "{bad:?} committed"
        );
        assert!(s.editor.is_some(), "{bad:?}: the editor must stay open");
        assert!(s.editor.as_ref().unwrap().error.is_some());
        s.handle_key(key(KeyCode::Esc));
    }
    // Taken by the other server.
    assert!(type_into(&mut s, FieldId::McpId, "fs").is_none());
    let err = s.editor.as_ref().unwrap().error.unwrap();
    assert!(err.contains("занят"), "{err}");
    s.handle_key(key(KeyCode::Esc));
    // A free one commits, and keeping its own id is not a duplicate.
    assert!(type_into(&mut s, FieldId::McpId, "gh").is_some());
    assert!(type_into(&mut s, FieldId::McpId, "gh").is_some());
    assert_eq!(s.config.mcp.servers[1].id, "gh");
}

#[test]
fn mcp_command_accepts_a_shim_and_a_plain_name() {
    // The `.bat`/`.cmd` ban is gone (ADR 0007 §2, revisited): `npx` resolves to
    // `npx.cmd` on Windows anyway, and `std` escapes batch arguments — refusing
    // the spelling only pushed users onto `cmd /c`, which is the worse path.
    let mut s = screen();
    goto_section(&mut s, Section::Plugins);
    s.handle_key(ctrl('n'));
    for command in ["npx", "run-server.cmd", "C:/tools/server.exe"] {
        assert!(
            type_into(&mut s, FieldId::McpCommand, command).is_some(),
            "{command} was refused"
        );
        assert!(s.editor.is_none(), "{command} left the editor open");
        assert_eq!(s.config.mcp.servers[0].command, command);
    }
}

#[test]
fn mcp_selector_switches_the_edited_server() {
    let mut s = screen();
    goto_section(&mut s, Section::Plugins);
    s.handle_key(ctrl('n'));
    type_into(&mut s, FieldId::McpId, "fs");
    s.handle_key(ctrl('n'));
    type_into(&mut s, FieldId::McpId, "gh");

    goto_field_again(&mut s, FieldId::McpSelect);
    assert!(
        s.handle_key(key(KeyCode::Left)).is_none(),
        "selection is navigation, not an edit"
    );
    assert_eq!(s.mcp_server_idx, 0);
    let shown = s.fields();
    let id_row = shown.iter().find(|r| r.id == FieldId::McpId).unwrap();
    assert!(matches!(&id_row.kind, FieldKind::Text(v) if v == "fs"));
    // The popup lists every configured server and jumps to the picked one.
    s.handle_key(key(KeyCode::Enter));
    let (opts, cur) = s.choice_menu(FieldId::McpSelect).unwrap();
    assert_eq!(opts, ["fs", "gh"]);
    assert_eq!(cur, 0);
    s.handle_key(key(KeyCode::Down));
    s.handle_key(key(KeyCode::Enter));
    assert_eq!(s.mcp_server_idx, 1);
}

#[test]
fn mcp_server_fields_are_user_data() {
    // No config "default server" exists to reset to, so `Del` is a no-op and the
    // "differs from default" marker never appears — like the persona list.
    let mut s = screen();
    goto_section(&mut s, Section::Plugins);
    s.handle_key(ctrl('n'));
    type_into(&mut s, FieldId::McpId, "fs");
    goto_field_again(&mut s, FieldId::McpId);
    assert!(s.handle_key(key(KeyCode::Delete)).is_none());
    assert_eq!(s.config.mcp.servers[0].id, "fs");
    for id in [
        FieldId::McpSelect,
        FieldId::McpId,
        FieldId::McpCommand,
        FieldId::McpArgs,
        FieldId::McpEnv,
        FieldId::McpEnvSecret(0),
        FieldId::McpImport,
        FieldId::McpEnabled,
        FieldId::McpTimeout,
        FieldId::McpMaxResult,
    ] {
        assert!(is_profile_field(id), "{id:?}");
    }
}

#[test]
fn mcp_undo_restores_a_deleted_server() {
    // Free: `Ctrl+Z` restores a whole older config snapshot
    // (docs/history/settings-undo.md), so it covers create/delete too.
    let mut s = screen();
    goto_section(&mut s, Section::Plugins);
    s.handle_key(ctrl('n'));
    type_into(&mut s, FieldId::McpId, "fs");
    s.handle_key(ctrl('d'));
    assert!(s.config.mcp.servers.is_empty());
    assert!(matches!(
        s.handle_key(ctrl('z')),
        Some(SettingsIntent::SaveConfig(_))
    ));
    assert_eq!(s.config.mcp.servers[0].id, "fs");
}

#[test]
fn mcp_args_and_env_round_trip_as_text() {
    // The field shows what `parse_args` will read back — a value that changed
    // shape on the way through would be edited into something else.
    for args in [
        vec!["-y".to_string(), "@scope/pkg".to_string()],
        vec!["D:/my work".to_string()],
        vec![r#"say "hi""#.to_string()],
        vec!["it's".to_string(), String::new()],
        vec![r"C:\Program Files\srv.exe".to_string()],
    ] {
        assert_eq!(parse_args(&join_args(&args)), args, "{args:?}");
    }
    // A hand-typed line: single quotes and runs of spaces are accepted too.
    assert_eq!(parse_args("  a   'b c'  d "), ["a", "b c", "d"]);
    assert_eq!(parse_args(""), Vec::<String>::new());

    // The usual form is a bare list of names: the value is entered in the row
    // below, or simply inherited. `=SOURCE` is the rare "take it from a
    // differently named variable" case, and both round-trip.
    let declared = std::collections::BTreeMap::from([
        ("GITHUB_TOKEN".to_string(), String::new()),
        ("SLACK_TOKEN".to_string(), String::new()),
    ]);
    assert_eq!(join_env_map(&declared), "GITHUB_TOKEN, SLACK_TOKEN");
    assert_eq!(parse_env_map(&join_env_map(&declared)), declared);
    let mapped = std::collections::BTreeMap::from([
        ("GITHUB_TOKEN".to_string(), "MINDFORK_PAT".to_string()),
        ("HOME".to_string(), "MY_HOME".to_string()),
    ]);
    assert_eq!(parse_env_map(&join_env_map(&mapped)), mapped);
    // Mixed, hand-typed, with spacing.
    assert_eq!(
        parse_env_map(" A = B , C "),
        std::collections::BTreeMap::from([
            ("A".to_string(), "B".to_string()),
            ("C".to_string(), String::new()),
        ])
    );
    // A half-typed entry doesn't destroy the ones already there: a nameless one
    // and one whose name we cannot carry are dropped.
    assert_eq!(parse_env_map("A=B, =C, has-dash").len(), 1);
}

#[test]
fn mcp_status_says_how_many_tools_the_profile_enabled() {
    use crate::features::tools::mcp::{McpServerSnapshot, McpSnapshot};
    // A server can be up while the model sees nothing — MCP tools are opt-in per
    // profile. Saying only "ready" is how a user adds a server and finds it does
    // not work (docs/history/mcp-server-editor.md §5).
    let mut s = screen();
    s.config.mcp.enabled = true;
    s.set_mcp(McpSnapshot {
        tools: Vec::new(),
        servers: vec![McpServerSnapshot {
            id: "fs".into(),
            status: ServerStatus::Ready,
            tool_count: 2,
            pending_catalog: false,
        }],
    });
    let row = |s: &SettingsScreen| {
        s.plugin_fields()
            .into_iter()
            .find(|r| r.id == FieldId::TMcpServer(0))
            .unwrap()
    };
    let off = row(&s);
    assert!(
        matches!(&off.kind, FieldKind::Text(v) if v.contains('2') && v.contains('0')),
        "the count of enabled tools is missing"
    );
    assert!(
        off.hint.is_some_and(|h| h.contains("Профил")),
        "no pointer to where they are enabled"
    );

    // Enabling one in the profile is reflected, and the pointer goes away.
    s.profiles[0].enabled_tools.push("mcp__fs__read".into());
    let on = row(&s);
    assert!(matches!(&on.kind, FieldKind::Text(v) if v.contains("1")));
    assert!(on.hint.is_none(), "the hint should go once tools are on");
    // Another server's tools don't count towards this one.
    s.profiles[0].enabled_tools.push("mcp__other__x".into());
    assert!(matches!(&row(&s).kind, FieldKind::Text(v) if v.contains("1")));
}

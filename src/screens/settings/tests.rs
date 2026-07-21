//! Тесты экрана настроек (через handle_key/render). См. mod.rs.

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

/// Переходит на нужную секцию через Tab (устойчиво к порядку секций).
/// После вызова фокус в меню (Tab сбрасывает его), поля не фокусированы.
fn goto_section(s: &mut SettingsScreen, sec: Section) {
    for _ in 0..SECTIONS.len() {
        if s.section() == sec {
            return;
        }
        s.handle_key(key(KeyCode::Tab));
    }
    assert_eq!(s.section(), sec, "секция {sec:?} не найдена");
}

/// Описание поля по id: строит поля всех секций/подсекций экрана и ищет строку.
/// Описание живёт на `FieldRow` (прикрепляется при построении — см. этап 3.1), а не
/// в отдельном match; поэтому оно есть только у **видимых** строк (draft-поля нужно
/// сделать видимыми, задав spec_type=draft-*).
fn field_desc(s: &SettingsScreen, id: FieldId) -> Option<String> {
    let mut rows = Vec::new();
    for mt in [
        ModelTab::Assistant,
        ModelTab::Impersonation,
        ModelTab::Embeddings,
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

/// Фокусирует поля и доходит вниз до поля `id` (устойчиво к группам/порядку).
/// Предполагает, что фокус в меню (как сразу после [`goto_section`]).
fn goto_field(s: &mut SettingsScreen, id: FieldId) {
    s.handle_key(key(KeyCode::Enter)); // фокус на поля
    for _ in 0..300 {
        if s.fields().get(s.field_idx).map(|f| f.id) == Some(id) {
            return;
        }
        s.handle_key(key(KeyCode::Down));
    }
    panic!("поле {id:?} не найдено в секции {:?}", s.section());
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
    // Поле «Язык интерфейса» присутствует и имеет описание.
    assert!(field_desc(&s, FieldId::ILanguage).is_some());
    // Циклическая правка (←→) меняет язык интерфейса.
    goto_section(&mut s, Section::Interface);
    goto_field(&mut s, FieldId::ILanguage);
    s.handle_key(key(KeyCode::Right));
    assert_eq!(s.config.interface.language, Lang::En);
    // Экран настроек перерисовывается в новом языке: секция «Интерфейс» → «Interface».
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
    // И при открытом редакторе поля — тоже выход (Ctrl+Q поверх редактора).
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
    // Переходим в Инструменты, на тумблер web-поиска.
    goto_section(&mut s, Section::Tools);
    goto_field(&mut s, FieldId::TWeb);
    let intent = s.handle_key(key(KeyCode::Char(' ')));
    match intent {
        Some(SettingsIntent::SaveConfig(c)) => assert!(!c.tools.web_enabled),
        other => panic!("ожидался SaveConfig, получено {other:?}"),
    }
}

#[test]
fn python_group_visibility_follows_mode() {
    // По умолчанию — режим Wasmer: видны «сеть» и «таймаут песочницы», путь скрыт.
    let mut s = screen();
    goto_section(&mut s, Section::Tools);
    let ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
    assert!(ids.contains(&FieldId::TPythonMode));
    assert!(ids.contains(&FieldId::TPython));
    assert!(ids.contains(&FieldId::TPythonNet));
    assert!(ids.contains(&FieldId::TPythonWasmTimeout));
    assert!(ids.contains(&FieldId::TPythonWasmMemory));
    assert!(!ids.contains(&FieldId::TPythonPath));

    // Режим Local: виден путь к интерпретатору, поля песочницы скрыты.
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
    // Правый цикл на 2-вариантном режиме переводит Wasmer → Local.
    let intent = s.handle_key(key(KeyCode::Right));
    match intent {
        Some(SettingsIntent::SaveConfig(c)) => {
            assert_eq!(c.tools.python_mode, PythonMode::Local)
        }
        other => panic!("ожидался SaveConfig, получено {other:?}"),
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
    // Ассистент → чип чат-сервера («готов»).
    let t = render_text(&mut s);
    assert!(t.contains("чат: готов"), "чип чат-сервера: {t}");
    // Переключение подсекции меняет чип на сервер эмбеддингов («подключение»).
    s.model_sub = ModelTab::Embeddings;
    let t = render_text(&mut s);
    assert!(
        t.contains("эмбеддинги: подключение"),
        "чип эмбеддингов: {t}"
    );
    assert!(!t.contains("чат: готов"), "чужой чип не показывается");
    // Другие секции чип не рисуют.
    goto_section(&mut s, Section::Interface);
    let t = render_text(&mut s);
    assert!(!t.contains("готов") && !t.contains("подключение"));
}

#[test]
fn interface_has_terminal_compat_toggle() {
    let mut s = screen();
    // Поле есть в секции «Интерфейс», сразу после темы.
    let rows = s.interface_fields();
    assert!(rows.iter().any(|r| r.id == FieldId::ICompat));
    // Переключение сохраняет конфиг с поднятым флагом…
    match s.toggle_field(FieldId::ICompat) {
        Some(SettingsIntent::SaveConfig(c)) => assert!(c.interface.terminal_compat),
        other => panic!("ожидался SaveConfig, получено {other:?}"),
    }
    // …и палитра рабочей копии тут же переходит на компат-набор глифов.
    assert!(s.palette().compat);
    assert!(field_desc(&s, FieldId::ICompat).is_some());
}

#[test]
fn interface_has_table_separators_toggle() {
    let mut s = screen();
    // Поле есть в секции «Интерфейс» (группа «Оформление»).
    let rows = s.interface_fields();
    assert!(rows.iter().any(|r| r.id == FieldId::ITableSeparators));
    // По умолчанию выключено; переключение сохраняет конфиг с поднятым флагом.
    match s.toggle_field(FieldId::ITableSeparators) {
        Some(SettingsIntent::SaveConfig(c)) => assert!(c.interface.table_row_separators),
        other => panic!("ожидался SaveConfig, получено {other:?}"),
    }
    assert!(field_desc(&s, FieldId::ITableSeparators).is_some());
}

#[test]
fn interface_has_mermaid_toggle() {
    let mut s = screen();
    // Поле есть в секции «Интерфейс» (группа «Оформление»).
    let rows = s.interface_fields();
    assert!(rows.iter().any(|r| r.id == FieldId::IMermaid));
    // По умолчанию включено; переключение сохраняет конфиг со снятым флагом.
    match s.toggle_field(FieldId::IMermaid) {
        Some(SettingsIntent::SaveConfig(c)) => assert!(!c.interface.render_mermaid),
        other => panic!("ожидался SaveConfig, получено {other:?}"),
    }
    assert!(field_desc(&s, FieldId::IMermaid).is_some());
}

#[test]
fn cycle_mode_changes_server_mode() {
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter)); // фокус на поля (ModelSub)
    s.handle_key(key(KeyCode::Down)); // XMode (режим, Choice)
    let intent = s.handle_key(key(KeyCode::Right));
    match intent {
        Some(SettingsIntent::SaveConfig(c)) => assert_eq!(c.engine.mode, ServerMode::External),
        other => panic!("ожидался SaveConfig, получено {other:?}"),
    }
}

#[test]
fn model_subsection_switches_to_impersonation_fields() {
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter)); // фокус на поля (ModelSub — таб-стрип)
    // → переключает подсекцию на «Имперсонация» (без сохранения).
    assert_eq!(s.handle_key(key(KeyCode::Right)), None);
    assert_eq!(s.model_sub, ModelTab::Impersonation);
    // Первое поле подсекции — режим имперсонации (3 значения).
    s.handle_key(key(KeyCode::Down)); // IxMode
    // Цикл shared → managed.
    let intent = s.handle_key(key(KeyCode::Right));
    match intent {
        Some(SettingsIntent::SaveConfig(c)) => {
            assert_eq!(c.impersonation_engine.mode, ImpersonationMode::Managed)
        }
        other => panic!("ожидался SaveConfig, получено {other:?}"),
    }
}

#[test]
fn model_subsection_third_tab_is_embeddings() {
    // Модель имеет третью вкладку «Эмбеддинги» (сервер переехал из «Инструментов»);
    // цикл вкладок вправо: Ассистент → Имперсонация → Эмбеддинги.
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter)); // ModelSub (таб-стрип)
    s.handle_key(key(KeyCode::Right)); // → Имперсонация
    s.handle_key(key(KeyCode::Right)); // → Эмбеддинги
    assert_eq!(s.model_sub, ModelTab::Embeddings);
    let ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
    assert!(ids.contains(&FieldId::EMode));
    assert!(ids.contains(&FieldId::EBinary));
    // В «Инструментах» эмбеддингов больше нет.
    goto_section(&mut s, Section::Tools);
    assert!(!s.fields().iter().any(|f| f.id == FieldId::EMode));
}

#[test]
fn section_counts_sum_matches_search_index() {
    // Счётчики параметров секций привязаны к выбранным режимам и выводятся из
    // того же индекса, что и поиск: сумма счётчиков секций тождественно равна
    // числу полей в поиске (инвариант, о котором просил пользователь). Проверяем
    // при разных сочетаниях режимов движка/провайдера.
    let mut s = screen();
    for &m in SERVER_MODES.iter() {
        s.config.engine.mode = m;
        s.config.embed.mode = m;
        let total: usize = s.section_counts().iter().sum();
        let search_total = s.build_search_index().len();
        assert_eq!(total, search_total, "режим {m:?}");
    }
}

#[test]
fn section_count_tracks_selected_mode() {
    // В отличие от прежнего union-подхода счётчик отражает текущий режим: у
    // managed llama-server полей заметно больше, чем у облачного провайдера (тот
    // показывает лишь модель/ключ/base URL — ADR 0004).
    let mut s = screen();
    s.config.engine.mode = ServerMode::Managed;
    s.config.impersonation_engine.mode = ImpersonationMode::Shared;
    s.config.embed.mode = ServerMode::Managed;
    let managed = s.section_field_count(Section::Model);
    s.config.engine.mode = ServerMode::OpenAi;
    let cloud = s.section_field_count(Section::Model);
    assert!(
        cloud < managed,
        "облако должно показывать меньше полей: managed={managed} cloud={cloud}"
    );

    // Счётчик секции «Модель» совпадает с числом её полей в индексе поиска
    // (сумма трёх вкладок для их текущих режимов, без селекторов подсекций).
    let model_from_search = s
        .build_search_index()
        .iter()
        .filter(|h| SECTIONS[h.section_idx] == Section::Model)
        .count();
    assert_eq!(cloud, model_from_search);
}

#[test]
fn section_field_count_excludes_subsection_selector() {
    // Селектор подсекции (таб-стрип) — навигационный элемент, не параметр, — в
    // счётчик не входит. Семплинг в локальном режиме (дефолт) показывает все
    // параметры каждой подсекции без строки-селектора.
    let s = screen();
    assert_eq!(
        s.section_field_count(Section::Sampling),
        Subsection::ALL.len() * SAMPLING_PARAMS.len(),
        "селектор подсекции попал в счёт семплинга"
    );
}

#[test]
fn impersonation_profile_subsection_has_no_tools() {
    let mut s = screen();
    goto_section(&mut s, Section::Profiles);
    goto_field(&mut s, FieldId::ProfileSub); // таб-стрип подсекции (поле 0)
    s.handle_key(key(KeyCode::Right)); // → Имперсонация
    assert_eq!(s.profile_sub, Subsection::Impersonation);
    let fields = s.fields();
    assert!(
        !fields.iter().any(|f| matches!(f.id, FieldId::PTool(_))),
        "в подсекции имперсонации нет тумблеров инструментов"
    );
    assert!(fields.iter().any(|f| f.id == FieldId::PImpSystem));
}

#[test]
fn editing_model_commits_text() {
    let mut s = screen();
    goto_field(&mut s, FieldId::XModel); // GGUF-модель (группа «Модель»)
    s.handle_key(key(KeyCode::Enter)); // открыть редактор XModel
    assert!(s.editor.is_some());
    for c in "gemma.gguf".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    let intent = s.handle_key(key(KeyCode::Enter));
    match intent {
        Some(SettingsIntent::SaveConfig(c)) => {
            assert_eq!(c.engine.managed.model_path.as_deref(), Some("gemma.gguf"))
        }
        other => panic!("ожидался SaveConfig, получено {other:?}"),
    }
    assert!(s.editor.is_none());
}

#[test]
fn editor_esc_discards() {
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter)); // ModelSub
    s.handle_key(key(KeyCode::Down)); // XMode
    s.handle_key(key(KeyCode::Down)); // XBinary (managed-режим)
    s.handle_key(key(KeyCode::Enter)); // редактор XBinary
    s.handle_key(key(KeyCode::Char('x')));
    let intent = s.handle_key(key(KeyCode::Esc));
    assert_eq!(intent, None);
    assert!(s.editor.is_none());
    // значение не изменилось
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
    // Каталог упорядочен по id (не по дефолт-списку), поэтому берём индекс первого
    // РЕАЛЬНО включённого в профиле инструмента — его тумблер должен выключиться.
    let catalog = crate::features::tools::tool_catalog();
    let enabled = s.profiles[0].enabled_tools.clone();
    let idx = catalog
        .iter()
        .position(|i| enabled.contains(&i.id))
        .expect("в профиле есть включённый инструмент из каталога");
    goto_field(&mut s, FieldId::PTool(idx));
    let before = s.profiles[0].enabled_tools.len();
    let intent = s.handle_key(key(KeyCode::Char(' ')));
    match intent {
        Some(SettingsIntent::SaveProfile { edit, .. }) => {
            let tools = edit.enabled_tools.unwrap();
            assert_eq!(tools.len(), before - 1, "включённый инструмент выключился");
        }
        other => panic!("ожидался SaveProfile, получено {other:?}"),
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
    goto_field(&mut s, FieldId::PSelect); // селектор профиля (после таб-стрипа)
    assert_eq!(s.profile_idx, 0);
    s.handle_key(key(KeyCode::Right));
    assert_eq!(s.profile_idx, 1);
}

#[test]
fn samplers_list_round_trip() {
    // Порядок семплеров: разбор по «;», склейка обратно, пустой → None.
    let v = parse_list("penalties;dry; temperature ", ';').unwrap();
    assert_eq!(v, vec!["penalties", "dry", "temperature"]);
    assert_eq!(join_list(Some(&v), ';'), "penalties;dry;temperature");
    assert_eq!(parse_list("", ';'), None);
    assert_eq!(parse_list("   ;  ", ';'), None);
    assert_eq!(join_list(None, ';'), "—");
}

#[test]
fn dry_breakers_decode_and_encode_escapes() {
    // Эскейпы \n \t декодируются при вводе и кодируются обратно при показе.
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
    s.handle_key(key(KeyCode::Enter)); // открыть редактор
    let editor = s.editor.as_ref().expect("редактор открыт");
    assert!(
        editor.multiline,
        "системное сообщение редактируется многострочно"
    );
    // Shift+Enter вставляет перевод строки, а не коммитит.
    s.handle_key(key(KeyCode::Char('A')));
    s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    s.handle_key(key(KeyCode::Char('B')));
    assert!(s.editor.is_some(), "Shift+Enter не закрывает редактор");
    let intent = s.handle_key(key(KeyCode::Enter)); // коммит
    match intent {
        Some(SettingsIntent::SaveProfile { edit, .. }) => {
            assert_eq!(edit.system_message.unwrap(), "Ты — ассистент.A\nB");
        }
        other => panic!("ожидался SaveProfile, получено {other:?}"),
    }
}

#[test]
fn alt_enter_also_inserts_newline_in_multiline_editor() {
    // Запасной перенос строки для терминалов без kitty-протокола (Shift+Enter там
    // неотличим от Enter). См. п.11 аудита InputBox.
    let mut s = screen();
    goto_section(&mut s, Section::Profiles);
    goto_field(&mut s, FieldId::PSystem);
    s.handle_key(key(KeyCode::Enter)); // открыть редактор (многострочный)
    s.handle_key(key(KeyCode::Char('A')));
    s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    s.handle_key(key(KeyCode::Char('B')));
    assert!(s.editor.is_some(), "Alt+Enter не закрывает редактор");
    let intent = s.handle_key(key(KeyCode::Enter)); // коммит
    match intent {
        Some(SettingsIntent::SaveProfile { edit, .. }) => {
            assert_eq!(edit.system_message.unwrap(), "Ты — ассистент.A\nB");
        }
        other => panic!("ожидался SaveProfile, получено {other:?}"),
    }
}

#[test]
fn ctrl_k_clears_and_ctrl_z_restores_multiline_editor() {
    let mut s = screen();
    goto_section(&mut s, Section::Profiles);
    goto_field(&mut s, FieldId::PSystem);
    s.handle_key(key(KeyCode::Enter)); // открыть редактор (многострочный)
    s.handle_key(key(KeyCode::Char('A')));
    s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    s.handle_key(key(KeyCode::Char('B')));
    let ctrl_k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
    let ctrl_z = KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL);
    s.handle_key(ctrl_k); // очистка
    assert_eq!(s.editor.as_ref().unwrap().input.text(), "");
    s.handle_key(ctrl_z); // возврат удалённого (общая модель отмены, §C)
    assert_eq!(
        s.editor.as_ref().unwrap().input.text(),
        "Ты — ассистент.A\nB"
    );
    assert!(s.editor.is_some(), "Ctrl+K/Ctrl+Z не закрывают редактор");
}

#[test]
fn greeting_editor_is_multiline_and_keeps_newlines() {
    let mut s = screen();
    goto_section(&mut s, Section::Profiles);
    goto_field(&mut s, FieldId::PGreeting);
    s.handle_key(key(KeyCode::Enter)); // открыть редактор
    let editor = s.editor.as_ref().expect("редактор открыт");
    assert!(editor.multiline, "приветствие редактируется многострочно");
    // Shift+Enter вставляет перевод строки, а не коммитит.
    s.handle_key(key(KeyCode::Char('A')));
    s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    s.handle_key(key(KeyCode::Char('B')));
    assert!(s.editor.is_some(), "Shift+Enter не закрывает редактор");
    let intent = s.handle_key(key(KeyCode::Enter)); // коммит
    match intent {
        Some(SettingsIntent::SaveProfile { edit, .. }) => {
            assert_eq!(edit.greeting.unwrap().as_deref(), Some("A\nB"));
        }
        other => panic!("ожидался SaveProfile, получено {other:?}"),
    }
}

#[test]
fn other_fields_edit_single_line() {
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter)); // поля (ModelSub)
    s.handle_key(key(KeyCode::Down)); // XMode
    s.handle_key(key(KeyCode::Down)); // XBinary (текст, managed-режим)
    s.handle_key(key(KeyCode::Enter)); // редактор
    let editor = s.editor.as_ref().expect("редактор открыт");
    assert!(!editor.multiline, "обычное поле редактируется однострочно");
}

#[test]
fn cloud_mode_reveals_model_and_key_fields() {
    // Переключение режима ассистента на облако (openai) показывает поля
    // «Модель» и «API-ключ (env)» и скрывает параметры llama-server.
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter)); // поля (ModelSub)
    s.handle_key(key(KeyCode::Down)); // XMode
    // managed → external → openai (cycle вправо дважды).
    s.handle_key(key(KeyCode::Right));
    s.handle_key(key(KeyCode::Right));
    assert_eq!(s.config.engine.mode, ServerMode::OpenAi);
    let ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
    assert!(ids.contains(&FieldId::XModelName));
    assert!(ids.contains(&FieldId::XApiKeyEnv));
    // Параметры локального сервера в облачном режиме скрыты.
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
    // Строки хоткеев (вне рамки, прижаты вправо) начинаются с пробела в колонке 0,
    // тогда как строки панели несут символ рамки. Считаем хвостовые строки-хоткеи.
    let status_rows = |lines: &[String]| -> usize {
        lines
            .iter()
            .rev()
            .take_while(|l| l.starts_with(' '))
            .count()
    };
    // Широко — хоткеи умещаются в одну строку под панелью, прижаты вправо
    // (заканчиваются на «выход»); строка над ними — нижняя рамка панели (не пробел).
    let wide = rows(120, 24);
    assert_eq!(status_rows(&wide), 1, "широко — одна строка хоткеев");
    let last = wide.last().unwrap();
    assert!(last.contains("Tab") && last.contains("выход"));
    assert!(
        last.trim_end().ends_with("выход"),
        "прижаты вправо: {last:?}"
    );
    assert!(
        !wide[wide.len() - 2].starts_with(' '),
        "над хоткеями — нижняя рамка панели: {:?}",
        wide[wide.len() - 2]
    );
    // Узко — хоткеи переносятся на несколько строк (высота статус-области > 1).
    let narrow = rows(46, 24);
    assert!(
        status_rows(&narrow) > 1,
        "ожидался перенос хоткеев: {}",
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
    s.handle_key(key(KeyCode::Enter)); // редактор
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
}

#[test]
fn flag_fields_have_descriptions() {
    // Черновые поля видны только при spec_type=draft-* — включаем, чтобы проверить их описание.
    let mut s = screen();
    s.config.engine.managed.spec_type = SpecType::DraftMtp;
    // -ngl, --jinja и --no-mmap снабжены человекопонятной подсказкой; обычное поле — нет.
    assert!(field_desc(&s, FieldId::XNgl).is_some());
    assert!(field_desc(&s, FieldId::XJinja).is_some());
    assert!(field_desc(&s, FieldId::XNoMmap).is_some());
    assert!(field_desc(&s, FieldId::XPort).is_none());
    // Новые поля FlashAttention/спекулятивного декодирования тоже описаны.
    assert!(field_desc(&s, FieldId::XFlashAttn).is_some());
    assert!(field_desc(&s, FieldId::XSpecType).is_some());
    assert!(field_desc(&s, FieldId::XDraftModel).is_some());
}

#[test]
fn managed_mode_shows_flash_attn_and_spec_type() {
    // В managed-режиме (дефолт) видны FlashAttention и --spec-type; черновые
    // поля скрыты, пока тип не draft-*.
    let s = screen();
    let ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
    assert!(ids.contains(&FieldId::XFlashAttn));
    assert!(ids.contains(&FieldId::XSpecType));
    assert!(!ids.contains(&FieldId::XDraftModel));
}

#[test]
fn cycling_spec_type_to_draft_reveals_draft_fields() {
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter)); // фокус на поля (XMode)
    while s.fields().get(s.field_idx).map(|f| f.id) != Some(FieldId::XSpecType) {
        s.handle_key(key(KeyCode::Down));
    }
    // none → draft-simple → draft-eagle3 → draft-mtp (три шага вправо).
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
    s.handle_key(key(KeyCode::Enter)); // фокус на поля (XMode)
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
    s.handle_key(key(KeyCode::Enter)); // фокус на поля (XMode)
    // Дойти до тумблера --no-mmap (есть описание-подсказка внизу).
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
    // Бегунок «█» на правой рамке экрана — только когда полей больше высоты.
    let has_thumb = |term: &Terminal<TestBackend>| {
        let buf = term.backend().buffer();
        let x = buf.area.right() - 1; // колонка рамки панели настроек
        (buf.area.top()..buf.area.bottom()).any(|y| buf[(x, y)].symbol() == "█")
    };
    let mut s = screen();
    goto_section(&mut s, Section::Sampling); // полей+заголовков заведомо больше высоты
    let mut term = Terminal::new(TestBackend::new(80, 14)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
    assert!(has_thumb(&term), "переполненная секция — с бегунком");
    // В высоком окне все поля видны — бегунка нет.
    let mut term = Terminal::new(TestBackend::new(80, 50)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
    assert!(!has_thumb(&term), "все поля видны — без бегунка");
}

#[test]
fn editing_new_sampling_field_commits() {
    let mut s = screen();
    goto_section(&mut s, Section::Sampling);
    goto_field(&mut s, FieldId::S(SamplingParam::MinP));
    s.handle_key(key(KeyCode::Enter)); // открыть редактор min_p
    for c in "0.03".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    let intent = s.handle_key(key(KeyCode::Enter)); // коммит
    match intent {
        Some(SettingsIntent::SaveConfig(c)) => {
            assert_eq!(c.default_sampling.min_p, Some(0.03))
        }
        other => panic!("ожидался SaveConfig, получено {other:?}"),
    }
}

#[test]
fn cloud_hides_unsupported_sampling_params() {
    let mut s = screen();
    goto_section(&mut s, Section::Sampling);
    let has =
        |s: &SettingsScreen, p: SamplingParam| s.fields().iter().any(|f| f.id == FieldId::S(p));
    // Локально (managed по умолчанию) — видны все параметры.
    assert!(has(&s, SamplingParam::TopK));
    assert!(has(&s, SamplingParam::Thinking));
    // Облако (Gemini, нативный): расширения llama.cpp (min_p) скрыты, а базовые +
    // top_k + reasoning — видны.
    s.config.engine.mode = ServerMode::Gemini;
    assert!(!has(&s, SamplingParam::MinP));
    assert!(has(&s, SamplingParam::TopK));
    assert!(has(&s, SamplingParam::Thinking));
    assert!(has(&s, SamplingParam::Reasoning));
    assert!(has(&s, SamplingParam::Temp));
    assert!(has(&s, SamplingParam::TopP));
    assert!(has(&s, SamplingParam::MaxTokens));
    // OpenAI (Responses): нет temperature/top_p/penalties/seed; есть reasoning и
    // verbosity (Responses-специфика).
    s.config.engine.mode = ServerMode::OpenAi;
    assert!(!has(&s, SamplingParam::Temp));
    assert!(!has(&s, SamplingParam::TopP));
    assert!(!has(&s, SamplingParam::FreqPen));
    assert!(has(&s, SamplingParam::MaxTokens));
    assert!(has(&s, SamplingParam::Thinking));
    assert!(has(&s, SamplingParam::Reasoning));
    assert!(has(&s, SamplingParam::Verbosity));
    // Verbosity — только у OpenAI: для Gemini/Claude скрыт.
    s.config.engine.mode = ServerMode::Gemini;
    assert!(!has(&s, SamplingParam::Verbosity));
    // Claude 4.x «зафиксировал» сэмплинг: виден только max_tokens.
    s.config.engine.mode = ServerMode::Claude;
    assert!(!has(&s, SamplingParam::Temp));
    assert!(!has(&s, SamplingParam::TopP));
    assert!(!has(&s, SamplingParam::FreqPen));
    assert!(has(&s, SamplingParam::MaxTokens));
}

#[test]
fn impersonation_shared_inherits_assistant_cloud_filter() {
    let mut s = screen();
    // Ассистент в облаке, имперсонация в shared → её сэмплинг фильтруется как облако.
    s.config.engine.mode = ServerMode::OpenAi;
    assert_eq!(
        s.config.impersonation_engine.mode,
        ImpersonationMode::Shared
    );
    goto_section(&mut s, Section::Sampling);
    s.handle_key(key(KeyCode::Enter)); // фокус (SamplingSub)
    s.handle_key(key(KeyCode::Right)); // → подсекция Имперсонация
    assert_eq!(s.sampling_sub, Subsection::Impersonation);
    let has_topk = s
        .fields()
        .iter()
        .any(|f| f.id == FieldId::IS(SamplingParam::TopK));
    assert!(
        !has_topk,
        "облако ассистента фильтрует и shared-имперсонацию"
    );
    // Локальная имперсонация (managed) показывает все параметры, даже если ассистент в облаке.
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
    s.handle_key(key(KeyCode::Enter)); // фокус на поля (Модель)
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
    // Все три вкладки подсекции модели видны как таб-стрип.
    assert!(text.contains("Ассистент"));
    assert!(text.contains("Эмбеддинги"));
    // Псевдо-поле «Подсекция» больше не рисуется строкой списка.
    assert!(
        !text.contains("Подсекция"),
        "селектор подсекции должен быть таб-стрипом, а не строкой списка"
    );
}

#[test]
fn profile_tools_are_grouped_with_descriptions() {
    // Каждый тумблер инструмента размечен смысловой группой (из meta) и несёт
    // короткое инлайн-описание. Группы — из известного набора (`group_titles`).
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
            "инструмент вне известной группы: {}",
            r.label
        );
        assert!(r.hint.is_some(), "нет инлайн-описания у {}", r.label);
    }
    // Инструменты одной группы идут подряд (заголовок не повторяется).
    let groups: Vec<&str> = tool_rows.iter().map(|r| r.group).collect();
    let mut seen = std::collections::HashSet::new();
    let mut prev = "";
    for g in groups {
        if g != prev {
            assert!(seen.insert(g), "группа {g} не непрерывна");
            prev = g;
        }
    }
}

#[test]
fn globally_disabled_tool_is_marked_gated() {
    // python выключен глобально, но включён в профиле → строка помечена гейтом
    // (warn + подсказка «выкл. глобально»); web включён → обычное описание.
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
    assert!(py.warn, "выключенный глобально python_exec — гейт");
    assert!(py.hint.unwrap().contains("глобально"));
    let web = find(FieldId::PTool(idx_of("web_search")));
    assert!(!web.warn, "web включён глобально — не гейт");
    assert_eq!(web.hint, Some("поиск в интернете"));
}

#[test]
fn mcp_tools_extend_profile_toggles_with_honest_gate() {
    use crate::features::tools::meta::{ToolGate, ToolGroup, ToolInfo};
    // Динамический каталог MCP (снимок из Settings) дописывается к статическому:
    // тумблер в группе «Плагины (MCP)», включённый при выключенном мастер-гейте —
    // помечен честным гейтом; переключение пишет id в профиль.
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
    assert!(row.warn, "MCP выключен глобально — честный гейт");
    assert!(row.hint.unwrap().contains("MCP"));
    // Мастер-гейт включён → обычная подсказка (имя инструмента) + ПОЛНОЕ
    // описание сервера в нижней панели (антидот tool-poisoning, spec §9.6).
    s.config.mcp.enabled = true;
    let fields = s.profile_fields();
    let row = fields.iter().find(|r| r.id == FieldId::PTool(idx)).unwrap();
    assert!(!row.warn);
    assert_eq!(row.hint, Some("read_text_file"));
    assert_eq!(
        row.description.as_deref(),
        Some("Read the complete contents of a file")
    );
    // Переключение тумблера убирает id из профиля (и обратно).
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
    // Строки серверов в группе «Плагины (MCP)»: готовый показывает число
    // инструментов; сервер с изменившимся каталогом — warn + Enter подтверждает.
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
    assert!(gh.warn, "изменившийся каталог — предупреждение");
    assert!(gh.hint.unwrap().contains("Enter"));
    // Enter на готовом сервере — no-op; на изменившемся — намерение подтвердить.
    assert!(s.confirm_mcp_catalog(0).is_none());
    assert_eq!(
        s.confirm_mcp_catalog(1),
        Some(SettingsIntent::ConfirmMcpCatalog("github".into()))
    );
    // Enter через handle_key доходит до подтверждения.
    goto_field(&mut s, FieldId::TMcpServer(1));
    assert_eq!(
        s.handle_key(key(KeyCode::Enter)),
        Some(SettingsIntent::ConfirmMcpCatalog("github".into()))
    );
}

#[test]
fn mcp_master_toggle_lives_in_tools_section() {
    // Тумблер «MCP-серверы» в секции «Инструменты»: переключение пишет конфиг.
    let mut s = screen();
    assert!(!s.config.mcp.enabled);
    goto_section(&mut s, Section::Tools);
    goto_field(&mut s, FieldId::TMcpEnabled);
    s.handle_key(key(KeyCode::Char(' ')));
    assert!(s.config.mcp.enabled, "Space включает мастер-гейт MCP");
    assert!(field_desc(&s, FieldId::TMcpEnabled).is_some());
}

#[test]
fn group_header_shows_toggle_count() {
    // Заголовок группы с ≥2 тумблерами несёт счётчик «вкл/всего»; одиночный — нет.
    let palette = Palette::default();
    let line = header_line("Веб-поиск", Some((1, 2)), 60, &palette);
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("1/2"), "нет счётчика: {text:?}");
    let plain = header_line("Сервер", None, 60, &palette);
    let ptext: String = plain.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        !ptext.contains('/'),
        "у группы без счётчика его быть не должно"
    );
}

#[test]
fn group_headers_same_length_with_and_without_count() {
    // Заголовки групп со счётчиком и без него доходят до одной колонки —
    // раньше счётчик давал линию на 1 символ короче (spurious +1).
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
            "ширина {w}: со счётчиком {counted} ≠ без {plain}"
        );
    }
}

#[test]
fn choice_popup_opens_and_applies_selection() {
    // Enter на Choice-поле открывает попап списка; ↓ + Enter применяет выбор.
    let mut s = screen();
    goto_field(&mut s, FieldId::XSpecType);
    s.handle_key(key(KeyCode::Enter));
    assert!(s.choice.is_some(), "Enter на Choice открывает попап");
    assert_eq!(s.config.engine.managed.spec_type, SpecType::None);
    s.handle_key(key(KeyCode::Down)); // none → draft-simple
    let intent = s.handle_key(key(KeyCode::Enter));
    assert!(s.choice.is_none(), "Enter применяет и закрывает попап");
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
        "Esc не меняет значение"
    );
}

#[test]
fn invalid_number_keeps_editor_open() {
    // Нечисло в числовом поле оставляет редактор открытым с ошибкой; правка сбрасывает.
    let mut s = screen();
    goto_field(&mut s, FieldId::XNgl);
    s.handle_key(key(KeyCode::Enter)); // редактор
    for c in "abc".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    let intent = s.handle_key(key(KeyCode::Enter)); // валидация: не закрывать
    assert_eq!(intent, None);
    assert!(s.editor.is_some(), "невалидный ввод не закрывает редактор");
    assert!(s.editor.as_ref().unwrap().error.is_some());
    // Правка сбрасывает ошибку и валидное значение коммитится.
    s.handle_key(ctrl('k')); // очистить
    for c in "42".chars() {
        s.handle_key(key(KeyCode::Char(c)));
    }
    let intent = s.handle_key(key(KeyCode::Enter));
    assert!(s.editor.is_none());
    match intent {
        Some(SettingsIntent::SaveConfig(c)) => assert_eq!(c.engine.managed.gpu_layers, 42),
        other => panic!("ожидался SaveConfig, получено {other:?}"),
    }
}

#[test]
fn field_validation_error_classifies_numbers() {
    assert!(field_validation_error(FieldId::XNgl, "abc").is_some());
    assert!(field_validation_error(FieldId::XNgl, "12").is_none());
    assert!(field_validation_error(FieldId::XNgl, "").is_none()); // пусто допустимо
    assert!(field_validation_error(FieldId::S(SamplingParam::Temp), "x").is_some());
    assert!(field_validation_error(FieldId::S(SamplingParam::Temp), "0.7").is_none());
    // Текстовые/списочные поля не валидируются как числа.
    assert!(field_validation_error(FieldId::XBinary, "любой текст").is_none());
    assert!(field_validation_error(FieldId::S(SamplingParam::Samplers), "top_k;top_p").is_none());
}

#[test]
fn del_resets_field_to_default() {
    let mut s = screen();
    s.config.engine.managed.gpu_layers = 40; // не дефолт
    let default_ngl = AppConfig::default().engine.managed.gpu_layers;
    goto_field(&mut s, FieldId::XNgl);
    let intent = s.handle_key(key(KeyCode::Delete));
    match intent {
        Some(SettingsIntent::SaveConfig(c)) => {
            assert_eq!(c.engine.managed.gpu_layers, default_ngl)
        }
        other => panic!("ожидался SaveConfig, получено {other:?}"),
    }
}

#[test]
fn del_on_default_field_is_noop() {
    // Поле уже в дефолте → Del ничего не делает; профильные поля Del не трогает.
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
    // Дефолтный конфиг — маркеров нет.
    let mut s = screen();
    s.handle_key(key(KeyCode::Enter));
    assert!(!render_text(&mut s).contains('•'), "в дефолте маркеров нет");
    // Изменённое поле — маркер появляется.
    s.config.engine.managed.gpu_layers = 40;
    assert!(
        render_text(&mut s).contains('•'),
        "изменённое поле помечено •"
    );
}

#[test]
fn search_filters_and_jumps_to_field() {
    let mut s = screen();
    // `/` открывает поиск; ввод фильтрует по уникальному слову.
    s.handle_key(key(KeyCode::Char('/')));
    assert!(s.search.is_some(), "`/` открывает оверлей поиска");
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
            "все результаты содержат запрос"
        );
    }
    // Enter — прыжок к полю (секция/фокус/индекс), оверлей закрыт.
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
    // При русской раскладке физическая клавиша `/` отдаёт `.` — поиск всё равно
    // должен открыться.
    let mut s = screen();
    s.handle_key(key(KeyCode::Char('.')));
    assert!(
        s.search.is_some(),
        "`.` (русская раскладка) открывает поиск"
    );
}

#[test]
fn search_jump_switches_subsection() {
    // Прыжок в поле неактивной подсекции переключает её (Модель → Эмбеддинги).
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
        "Esc не двигает навигацию"
    );
}

#[test]
fn search_index_covers_all_subsections() {
    // Индекс поиска содержит поля всех подсекций (напр. и managed-сервер, и
    // облачная модель ассистента доступны через поиск при текущем режиме).
    let s = screen();
    let idx = s.build_search_index();
    assert!(
        idx.len() > 100,
        "индекс охватывает все секции: {}",
        idx.len()
    );
    // Поле имперсонации-модели индексируется, хотя активна подсекция ассистента.
    assert!(
        idx.iter().any(|h| h.crumb.contains("Имперсонация")),
        "в индексе есть поля подсекции имперсонации"
    );
}

#[test]
fn memory_section_gathers_rag_notes_self_model() {
    // Секция «Память» собрала поля, ранее размазанные по «Инструментам».
    let mut s = screen();
    goto_section(&mut s, Section::Memory);
    let ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
    for id in [
        FieldId::RagTarget,
        FieldId::NotesAutoConsolidate,
        FieldId::SmMaxNarrative,
        FieldId::SmProtocol,
    ] {
        assert!(ids.contains(&id), "в «Памяти» нет {id:?}");
    }
    // А в «Инструментах» их больше нет — там только гейты/параметры.
    goto_section(&mut s, Section::Tools);
    let tool_ids: Vec<FieldId> = s.fields().iter().map(|f| f.id).collect();
    assert!(!tool_ids.contains(&FieldId::RagTarget));
    assert!(!tool_ids.contains(&FieldId::SmMaxNarrative));
    // max_tool_rounds переехал из бывшего «Инференса» в «Инструменты».
    assert!(tool_ids.contains(&FieldId::MaxToolRounds));
}

#[test]
fn fields_carry_group_headers() {
    // Поля секции размечены смысловыми группами (заголовки групп в UI).
    let s = screen();
    let groups: Vec<&str> = s.model_fields().iter().map(|f| f.group).collect();
    // Подсекция/режим — вне группы; параметры сервера — в группе «Сервер».
    assert!(groups.iter().any(|g| g.is_empty()));
    assert!(groups.contains(&"Сервер"));
    // Семплинг: параметры сгруппированы по смыслу.
    let sg: Vec<&str> = s.sampling_fields().iter().map(|f| f.group).collect();
    assert!(sg.contains(&"Основные"));
    assert!(sg.contains(&"Рассуждения"));
}

#[test]
fn long_value_is_truncated_with_ellipsis() {
    // Очень длинное значение усекается с «…» под ширину колонки.
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
        "длинное значение усечено: {rendered:?}"
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
    // Выбранное поле — зелёный рейл `▌` в левой колонке (как активная секция меню).
    let sel = render_field_line(&f, 20, 24, false, true, &palette);
    assert_eq!(sel.spans[0].content.as_ref(), "▌ ");
    assert_eq!(sel.spans[0].style.fg, Some(palette.success));
    // Невыбранное изменённое — маркер «•».
    let modf = render_field_line(&f, 20, 24, true, false, &palette);
    assert_eq!(modf.spans[0].content.as_ref(), "• ");
    // Невыбранное немодифицированное — пусто.
    let plain = render_field_line(&f, 20, 24, false, false, &palette);
    assert_eq!(plain.spans[0].content.as_ref(), "  ");
    // У выбранного изменённого поля рейл приоритетнее маркера «•».
    let both = render_field_line(&f, 20, 24, true, true, &palette);
    assert_eq!(both.spans[0].content.as_ref(), "▌ ");
}

#[test]
fn section_label_col_has_floor_cap_and_skips_subsection() {
    let text = |s: &str| FieldKind::Text(s.into());
    // Одни короткие подписи → пол MIN_LABEL_COL.
    let short = vec![row(FieldId::PName, "Имя", text("x"))];
    assert_eq!(section_label_col(&short), MIN_LABEL_COL);
    // Самая длинная подпись в пределах потолка задаёт колонку всей секции.
    let medium = vec![
        row(FieldId::PName, "Имя", text("x")),
        row(FieldId::PGreeting, "Подпись средней длины!", text("y")),
    ];
    assert_eq!(section_label_col(&medium), 22);
    // Сверхдлинная подпись (> LABEL_CAP) колонку не отгоняет…
    let mut with_outlier = medium;
    with_outlier.push(row(
        FieldId::PSystem,
        &"а".repeat(LABEL_CAP + 12),
        text("z"),
    ));
    assert_eq!(section_label_col(&with_outlier), 22);
    // …а селектор подсекции (таб-стрип, не строка списка) исключён вовсе.
    let with_sub = vec![
        row(FieldId::PName, "Имя", text("x")),
        row(FieldId::ModelSub, &"б".repeat(25), text("s")),
    ];
    assert_eq!(section_label_col(&with_sub), MIN_LABEL_COL);
}

#[test]
fn all_labels_fit_alignment_cap() {
    // Все подписи всех секций/подсекций укладываются в потолок выравнивания —
    // значения каждой секции стоят на одной вертикали, без локальных
    // переполнений. Новому длинному имени — сократить подпись, перенеся
    // контекст в заголовок группы (как «Копирование переписки (F5)»).
    let mut cfg = AppConfig::default();
    // Черновые поля спекулятивного декодирования видны только для draft-типов.
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
                "подпись «{}» ({section}) шире LABEL_CAP={LABEL_CAP} — сократите \
                     её или перенесите контекст в заголовок группы",
                f.label
            );
        }
    }
}

#[test]
fn value_column_is_shared_across_groups() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    // Значения разных групп секции стоят на одной вертикали (единая колонка
    // на секцию; колонка на группу давала «пилу» между группами).
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
    // Ячейка первого непробельного символа после подписи (= начало значения).
    let value_cell = |label: &str| -> usize {
        let line = lines
            .iter()
            .find(|l| l.contains(label))
            .unwrap_or_else(|| panic!("нет строки с подписью {label:?}"));
        let chars: Vec<char> = line.chars().collect();
        let needle: Vec<char> = label.chars().collect();
        let start = (0..=chars.len() - needle.len())
            .find(|&i| chars[i..i + needle.len()] == needle[..])
            .unwrap();
        let after = start + needle.len();
        after + chars[after..].iter().take_while(|c| **c == ' ').count()
    };
    // Три поля из трёх разных групп («Агентный цикл»/«Веб-поиск»/«Файлы»).
    let a = value_cell("Лимит раундов инструментов");
    let b = value_cell("Web-поиск");
    let c = value_cell("Доступ к файлам");
    assert_eq!(
        a, b,
        "значения групп «Агентный цикл» и «Веб-поиск» в одной колонке"
    );
    assert_eq!(b, c, "значения группы «Файлы» в той же колонке");
}

#[test]
fn sampling_extensions_have_descriptions() {
    // Каждый параметр семплинга снабжён подсказкой в обеих подсекциях —
    // и расширения llama.cpp, и базовые OpenAI-поля.
    let s = screen();
    for &p in SAMPLING_PARAMS {
        assert!(
            field_desc(&s, FieldId::S(p)).is_some(),
            "нет подсказки для {p:?} (Ассистент)",
        );
        assert!(
            field_desc(&s, FieldId::IS(p)).is_some(),
            "нет подсказки для {p:?} (Имперсонация)",
        );
    }
}

/// Поле «API-ключ» есть только в облачных режимах и показывает **статус**, а не
/// секрет; сохранённый ключ отражается значением «настроен».
#[test]
fn api_key_field_shows_status_in_cloud_modes_only() {
    let mut s = screen();
    goto_section(&mut s, Section::Model);
    let has_key_field = |s: &SettingsScreen| s.fields().iter().any(|f| f.id == FieldId::XApiKey);
    // Локальный managed — поля ключа нет (облачного провайдера не существует).
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
    // Ключ этого провайдера сохранён на машине → статус меняется.
    s.set_api_keys_present(vec![CloudProvider::OpenAi]);
    let set = value(&s);
    assert_ne!(unset, set, "статус поля не отражает наличие ключа");
    // Ключ другого провайдера на статус OpenAI не влияет.
    s.set_api_keys_present(vec![CloudProvider::Claude]);
    assert_eq!(value(&s), unset);
    // Сам секрет в значении поля не появляется ни при каком статусе.
    assert!(!set.contains("sk-"));
}

/// Правка поля ключа открывает **пустой** маскированный редактор (сохранённый ключ
/// показать нельзя), а коммит уходит отдельным намерением — не в конфиг экрана.
#[test]
fn api_key_editor_is_masked_empty_and_commits_intent() {
    let mut s = screen();
    s.config.engine.mode = ServerMode::Claude;
    s.set_api_keys_present(vec![CloudProvider::Claude]); // ключ уже сохранён
    goto_section(&mut s, Section::Model);
    goto_field(&mut s, FieldId::XApiKey);

    s.handle_key(key(KeyCode::Enter));
    let editor = s.editor.as_ref().expect("редактор поля ключа открыт");
    assert_eq!(editor.input.text(), "", "затравка должна быть пустой");
    assert!(
        editor.input.is_masked(),
        "поле секрета должно быть маскировано"
    );

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
    // Секрет не осел в рабочей копии конфига.
    let json = serde_json::to_string(&s.config).unwrap();
    assert!(
        !json.contains("sk-ant-123"),
        "ключ утёк в конфиг экрана: {json}"
    );
}

/// `Del` на поле ключа удаляет сохранённый ключ (пустое значение намерения);
/// если ключа нет — no-op (нечего удалять).
#[test]
fn del_on_api_key_field_removes_stored_key() {
    let mut s = screen();
    s.config.engine.mode = ServerMode::Gemini;
    goto_section(&mut s, Section::Model);
    goto_field(&mut s, FieldId::XApiKey);
    // Ключа нет — сброс ничего не делает.
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

/// Поле ключа есть у всех трёх слотов и адресует провайдера **своего** движка:
/// ключ общий для чата/имперсонации/эмбеддингов одного провайдера.
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

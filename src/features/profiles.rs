//! Операции над профилями ИИ-собеседника: создание, редактирование, валидация
//! имени (чистая логика без I/O). Хранение и каскадное мягкое удаление — в
//! `shared/storage`; UI-секция профилей — на M8. См. spec §10.

use crate::entities::profile::ToolId;
use crate::entities::profile::{CharacterNames, Profile};
use crate::entities::sampling::SamplingConfig;
use crate::features::tools::default_tool_ids;

/// Максимальная длина имени профиля (в символах). Лишнее обрезается.
pub const MAX_NAME_LEN: usize = 80;

/// Нормализует имя профиля: схлопывает пробелы, обрезает края и длину.
/// Возвращает `None`, если после нормализации пусто (операция отклоняется).
pub fn sanitize_name(input: &str) -> Option<String> {
    let collapsed: String = input
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    let mut name = String::with_capacity(collapsed.len());
    let mut prev_space = false;
    for ch in collapsed.trim().chars() {
        let is_space = ch == ' ';
        if is_space && prev_space {
            continue;
        }
        name.push(ch);
        prev_space = is_space;
    }
    if name.is_empty() {
        return None;
    }
    Some(name.chars().take(MAX_NAME_LEN).collect())
}

/// Создаёт новый профиль с валидным именем. `None`, если имя пустое. Новый профиль
/// получает текущий набор инструментов по умолчанию (через [`reconcile_tools`]).
pub fn create(name: &str, system_message: impl Into<String>) -> Option<Profile> {
    let name = sanitize_name(name)?;
    let mut profile = Profile::new(name, system_message);
    reconcile_tools(&mut profile);
    Some(profile)
}

/// Сверяет инструменты профиля с текущим набором по умолчанию: ранее неизвестные
/// профилю инструменты (новые в приложении) **включаются** и записываются в реестр
/// «известных» (`known_tools`). Инструменты, которые пользователь осознанно выключил
/// (они уже в `known_tools`), повторно НЕ включаются. Возвращает `true`, если профиль
/// изменён (требуется сохранение). См. spec §9.4.
///
/// Для профиля, у которого `known_tools` пуст (создан до появления этого реестра),
/// «новыми» считаются лишь инструменты, которых нет в `enabled_tools`, — то есть к
/// уже включённому набору добавляются недостающие (новые в приложении), а сам набор
/// фиксируется как известный.
pub fn reconcile_tools(profile: &mut Profile) -> bool {
    // База «известного» при первом запуске миграции — то, что уже включено
    // (старый снимок дефолтов). Это не даёт повторно включить инструмент, который
    // был включён ранее, но ничего не ломает, если known_tools уже заполнен.
    if profile.known_tools.is_empty() {
        profile.known_tools = profile.enabled_tools.clone();
    }
    let mut changed = false;
    for id in default_tool_ids() {
        if profile.known_tools.iter().any(|t| t == &id) {
            continue; // профиль уже знал инструмент — уважаем выбор пользователя
        }
        profile.known_tools.push(id.clone());
        changed = true;
        if !profile.enabled_tools.iter().any(|t| t == &id) {
            profile.enabled_tools.push(id);
        }
    }
    changed
}

/// Набор правок профиля (любое поле — опционально). Применяется к существующему
/// профилю; не затрагивает уже созданные чаты (у них свои копии, spec §10).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProfileEdit {
    pub name: Option<String>,
    pub system_message: Option<String>,
    /// Системное сообщение режима имперсонации (см. [`Profile::impersonation_system_message`]).
    pub impersonation_system_message: Option<String>,
    /// `Some(None)` — снять приветствие; `Some(Some(..))` — задать.
    pub greeting: Option<Option<String>>,
    pub character_names: Option<CharacterNames>,
    /// `Some(None)` — убрать дефолты семплинга профиля.
    pub default_sampling: Option<Option<SamplingConfig>>,
    pub enabled_tools: Option<Vec<ToolId>>,
    /// Язык служебного каркаса (ось A, docs/history/i18n.md). Правка разрешена только пока у
    /// профиля нет данных — гейт **авторитетно** проверяет оркестратор перед
    /// применением (`handle_update_profile`), а UI дополнительно рисует поле
    /// заблокированным.
    pub language: Option<crate::shared::i18n::Lang>,
}

/// Применяет правки к профилю. Возвращает `false`, если имя задано, но пустое
/// (правки не применяются — профиль остаётся прежним).
pub fn apply_edit(profile: &mut Profile, edit: ProfileEdit) -> bool {
    // Валидируем имя до любых мутаций (атомарность правки).
    let new_name = match &edit.name {
        Some(raw) => match sanitize_name(raw) {
            Some(n) => Some(n),
            None => return false,
        },
        None => None,
    };
    if let Some(name) = new_name {
        profile.name = name;
    }
    if let Some(system) = edit.system_message {
        profile.default_system_message = system;
    }
    if let Some(system) = edit.impersonation_system_message {
        profile.impersonation_system_message = system;
    }
    if let Some(greeting) = edit.greeting {
        profile.greeting = greeting.filter(|g| !g.is_empty());
    }
    if let Some(names) = edit.character_names {
        profile.character_names = names;
    }
    if let Some(sampling) = edit.default_sampling {
        profile.default_sampling = sampling;
    }
    if let Some(tools) = edit.enabled_tools {
        profile.enabled_tools = tools;
    }
    if let Some(lang) = edit.language {
        profile.language = lang;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_collapses_and_trims() {
        assert_eq!(
            sanitize_name("  Джойс   Кэрол ").as_deref(),
            Some("Джойс Кэрол")
        );
        assert_eq!(sanitize_name("   \t\n "), None);
    }

    #[test]
    fn sanitize_truncates() {
        let long = "я".repeat(MAX_NAME_LEN + 20);
        assert_eq!(sanitize_name(&long).unwrap().chars().count(), MAX_NAME_LEN);
    }

    #[test]
    fn create_rejects_empty_name() {
        assert!(create("   ", "sys").is_none());
        let p = create("  Карлос ", "Ты — Карлос.").unwrap();
        assert_eq!(p.name, "Карлос");
        assert_eq!(p.default_system_message, "Ты — Карлос.");
    }

    #[test]
    fn apply_edit_updates_selected_fields() {
        let mut p = Profile::new("X", "sys");
        let ok = apply_edit(
            &mut p,
            ProfileEdit {
                name: Some("  Новый  ".into()),
                system_message: Some("новое sys".into()),
                greeting: Some(Some("Привет!".into())),
                ..Default::default()
            },
        );
        assert!(ok);
        assert_eq!(p.name, "Новый");
        assert_eq!(p.default_system_message, "новое sys");
        assert_eq!(p.greeting.as_deref(), Some("Привет!"));
    }

    #[test]
    fn apply_edit_empty_greeting_clears_it() {
        let mut p = Profile::new("X", "sys");
        p.greeting = Some("было".into());
        apply_edit(
            &mut p,
            ProfileEdit {
                greeting: Some(Some(String::new())),
                ..Default::default()
            },
        );
        assert_eq!(p.greeting, None);
    }

    #[test]
    fn reconcile_adds_new_tools_to_existing_profile() {
        // Профиль со «старым» снимком дефолтов (без новых инструментов).
        let mut p = Profile::new("X", "sys");
        p.enabled_tools = vec!["note_save".into(), "web_search".into()];
        // known_tools пуст (создан до реестра) — миграция должна добавить новые.
        let changed = reconcile_tools(&mut p);
        assert!(changed);
        // Новые безопасные инструменты включены.
        assert!(p.enabled_tools.iter().any(|t| t == "calculate"));
        assert!(p.enabled_tools.iter().any(|t| t == "current_time"));
        // Старые сохранены.
        assert!(p.enabled_tools.iter().any(|t| t == "note_save"));
        // Все текущие дефолты теперь «известны».
        for id in default_tool_ids() {
            assert!(p.known_tools.iter().any(|t| t == &id), "не записан: {id}");
        }
    }

    #[test]
    fn reconcile_does_not_reenable_user_disabled_tool() {
        // Пользователь осознанно выключил calculate: его нет в enabled, но он в known.
        let mut p = Profile::new("X", "sys");
        p.known_tools = default_tool_ids();
        p.enabled_tools = default_tool_ids()
            .into_iter()
            .filter(|t| t != "calculate")
            .collect();
        let changed = reconcile_tools(&mut p);
        assert!(!changed, "ничего нового — известный выключенный не трогаем");
        assert!(
            !p.enabled_tools.iter().any(|t| t == "calculate"),
            "выключенный инструмент не должен переоткрываться"
        );
    }

    #[test]
    fn reconcile_is_idempotent() {
        let mut p = Profile::new("X", "sys");
        assert!(reconcile_tools(&mut p)); // первый прогон включает дефолты
        let after_first = p.clone();
        assert!(!reconcile_tools(&mut p)); // повтор — без изменений
        assert_eq!(p, after_first);
    }

    #[test]
    fn create_gives_default_tools() {
        let p = create("Имя", "sys").unwrap();
        assert!(p.enabled_tools.iter().any(|t| t == "calculate"));
        assert_eq!(p.enabled_tools, default_tool_ids());
    }

    #[test]
    fn apply_edit_rejects_empty_name_without_mutating() {
        let mut p = Profile::new("Имя", "sys");
        let ok = apply_edit(
            &mut p,
            ProfileEdit {
                name: Some("   ".into()),
                system_message: Some("не должно примениться".into()),
                ..Default::default()
            },
        );
        assert!(!ok);
        assert_eq!(p.name, "Имя");
        assert_eq!(p.default_system_message, "sys");
    }
}

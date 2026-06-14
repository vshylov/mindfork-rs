//! Операции над профилями ИИ-собеседника: создание, редактирование, валидация
//! имени (чистая логика без I/O). Хранение и каскадное мягкое удаление — в
//! `shared/storage`; UI-секция профилей — на M8. См. spec §10.

use crate::entities::profile::ToolId;
use crate::entities::profile::{CharacterNames, Profile};
use crate::entities::sampling::SamplingConfig;

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

/// Создаёт новый профиль с валидным именем. `None`, если имя пустое.
pub fn create(name: &str, system_message: impl Into<String>) -> Option<Profile> {
    let name = sanitize_name(name)?;
    Some(Profile::new(name, system_message))
}

/// Набор правок профиля (любое поле — опционально). Применяется к существующему
/// профилю; не затрагивает уже созданные чаты (у них свои копии, spec §10).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProfileEdit {
    pub name: Option<String>,
    pub system_message: Option<String>,
    /// `Some(None)` — снять приветствие; `Some(Some(..))` — задать.
    pub greeting: Option<Option<String>>,
    pub character_names: Option<CharacterNames>,
    /// `Some(None)` — убрать дефолты семплинга профиля.
    pub default_sampling: Option<Option<SamplingConfig>>,
    pub enabled_tools: Option<Vec<ToolId>>,
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

//! Профиль ИИ-собеседника и связанные типы. См. spec §5.1, §10.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entities::sampling::SamplingConfig;

/// Идентификатор инструмента (на M5 может стать перечислением).
pub type ToolId = String;

/// Отображаемые имена ролей.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CharacterNames {
    pub user: String,
    pub assistant: String,
    pub system: String,
}

impl Default for CharacterNames {
    fn default() -> Self {
        Self {
            user: "Вы".to_string(),
            assistant: "Ассистент".to_string(),
            system: "Система".to_string(),
        }
    }
}

/// Профиль ИИ-собеседника: системное сообщение, имена, инструменты, дефолты.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub id: Uuid,
    pub name: String,
    pub default_system_message: String,
    #[serde(default)]
    pub character_names: CharacterNames,
    /// Приветственное сообщение ассистента (первое сообщение в новом чате).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub greeting: Option<String>,
    #[serde(default)]
    pub enabled_tools: Vec<ToolId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_sampling: Option<SamplingConfig>,
    /// Мягкое удаление.
    #[serde(default)]
    pub is_hidden: bool,
}

impl Profile {
    /// Новый профиль с системным сообщением и значениями по умолчанию.
    pub fn new(name: impl Into<String>, system_message: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            default_system_message: system_message.into(),
            character_names: CharacterNames::default(),
            greeting: None,
            enabled_tools: Vec::new(),
            default_sampling: None,
            is_hidden: false,
        }
    }

    /// Краткая карточка профиля (для оверлея выбора без копирования системного
    /// сообщения и набора инструментов).
    pub fn summary(&self) -> ProfileSummary {
        ProfileSummary {
            id: self.id,
            name: self.name.clone(),
        }
    }
}

/// Краткая карточка профиля для оверлея выбора при создании чата. См. spec §11.2.
/// View-проекция домена в `entities`, чтобы её могли использовать и `app`
/// (события), и `widgets` (рендер) — зависимость строго вниз.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileSummary {
    pub id: Uuid,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_roundtrip() {
        let p = Profile::new("Joyce", "Ты — Джойс.");
        let json = serde_json::to_string(&p).unwrap();
        let back: Profile = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn defaults_applied() {
        let raw = format!(
            r#"{{"id":"{}","name":"X","default_system_message":"s"}}"#,
            Uuid::nil()
        );
        let p: Profile = serde_json::from_str(&raw).unwrap();
        assert_eq!(p.character_names, CharacterNames::default());
        assert!(!p.is_hidden);
        assert!(p.enabled_tools.is_empty());
    }
}

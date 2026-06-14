//! Чат: список сообщений, привязка к профилю, активное системное сообщение.
//! См. spec §5.1.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entities::message::Message;
use crate::entities::profile::{CharacterNames, Profile};
use crate::entities::sampling::SamplingConfig;

/// Чат.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chat {
    pub id: Uuid,
    pub profile_id: Uuid,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    /// Активное системное сообщение чата (ассистент может менять его инструментом).
    pub system_message: String,
    #[serde(default)]
    pub character_names: CharacterNames,
    #[serde(default)]
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling_override: Option<SamplingConfig>,
    /// Мягкое удаление.
    #[serde(default)]
    pub is_hidden: bool,
}

impl Chat {
    /// Создаёт чат, привязанный к профилю, копируя из него системное сообщение
    /// и имена ролей (у чата своя копия — правки профиля их не меняют, spec §10).
    /// Приветствие (`greeting`) добавляется вызывающей стороной как первое
    /// сообщение ассистента (use-case, M4).
    ///
    /// Семплинг профиля **не копируется** в `sampling_override`: разрешение
    /// трёхуровневое во время запроса (`Chat → Profile → global`, spec §8.3),
    /// поэтому `sampling_override` остаётся `None` до явного переопределения
    /// пользователем или ассистентом (`set_sampling`, M5).
    pub fn from_profile(profile: &Profile, title: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            profile_id: profile.id,
            title: title.into(),
            created_at: now,
            modified_at: now,
            system_message: profile.default_system_message.clone(),
            character_names: profile.character_names.clone(),
            messages: Vec::new(),
            sampling_override: None,
            is_hidden: false,
        }
    }

    /// Добавляет сообщение и обновляет `modified_at`.
    pub fn push_message(&mut self, message: Message) {
        self.messages.push(message);
        self.modified_at = Utc::now();
    }

    /// Краткая карточка чата (для списка/оверлея без копирования сообщений).
    pub fn summary(&self) -> ChatSummary {
        ChatSummary {
            id: self.id,
            title: self.title.clone(),
            created_at: self.created_at,
            modified_at: self.modified_at,
            message_count: self.messages.len(),
        }
    }
}

/// Краткая карточка чата для списка/оверлея (без сообщений). См. spec §11.2.
/// Это view-проекция домена, живёт в `entities`, чтобы её могли использовать и
/// `app` (события), и `widgets` (рендер) — зависимость строго вниз.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatSummary {
    pub id: Uuid,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    pub message_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::message::Message;

    #[test]
    fn from_profile_copies_fields_but_not_sampling() {
        let mut p = Profile::new("Carlos", "Ты — Карлос.");
        p.default_sampling = Some(SamplingConfig {
            temperature: Some(0.9),
            ..Default::default()
        });
        let chat = Chat::from_profile(&p, "Новый чат");
        assert_eq!(chat.profile_id, p.id);
        assert_eq!(chat.system_message, "Ты — Карлос.");
        assert_eq!(chat.character_names, p.character_names);
        // Семплинг профиля НЕ копируется в override — разрешается трёхуровнево
        // во время запроса (spec §8.3).
        assert_eq!(chat.sampling_override, None);
        assert!(chat.messages.is_empty());
    }

    #[test]
    fn push_message_updates_modified() {
        let p = Profile::new("X", "s");
        let mut chat = Chat::from_profile(&p, "t");
        let before = chat.modified_at;
        chat.push_message(Message::user("hi"));
        assert_eq!(chat.messages.len(), 1);
        assert!(chat.modified_at >= before);
    }

    #[test]
    fn serde_roundtrip() {
        let p = Profile::new("X", "s");
        let mut chat = Chat::from_profile(&p, "t");
        chat.push_message(Message::user("hi"));
        chat.push_message(Message::assistant("hello"));
        let json = serde_json::to_string(&chat).unwrap();
        let back: Chat = serde_json::from_str(&json).unwrap();
        assert_eq!(chat, back);
    }
}

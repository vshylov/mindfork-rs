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
    /// Несохранённый черновик поля ввода (текст, который пользователь набрал, но
    /// ещё не отправил). Хранится в файле чата и восстанавливается в поле ввода при
    /// переключении на чат; у нового чата пустой. См. spec §11.7.
    #[serde(default)]
    pub draft: String,
    /// Удалённые обмены (`Ctrl+E`/`Ctrl+R`). Хранятся в файле чата только ради
    /// **ручного** восстановления (правкой JSON) в редких случаях, когда удалили
    /// что-то важное; в UI не используются и автоматически не восстанавливаются.
    /// См. spec §11.7.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deleted: Vec<DeletedExchange>,
    /// Индекс-водораздел фоновой авто-рефлексии: сколько первых сообщений `messages`
    /// уже охвачено рефлексией. Дайджест строится только по «хвосту» `messages[wm..]`
    /// — чтобы каждый цикл не перечитывал один и тот же ранний материал (иначе
    /// плодятся дубли инсайтов). Живёт с чатом → переживает рестарт; усечение истории
    /// (`Ctrl+R`/`Ctrl+E`) лечится клампом при чтении. `None`/старые файлы — с начала.
    /// См. docs/refinements.md (этап 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflected_upto: Option<usize>,
    /// Когда фоновая авто-рефлексия запускалась в последний раз (ориентир; на будущее
    /// — фильтр поведенческих сигналов по окну). `None`/старые файлы — не запускалась.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflected_at: Option<DateTime<Utc>>,
    /// Мягкое удаление.
    #[serde(default)]
    pub is_hidden: bool,
}

/// Снимок удалённого обмена (`Ctrl+E`/`Ctrl+R`). Это **не** сообщение, а
/// контейнер: удалённые сообщения + черновик поля ввода на момент удаления.
/// Восстановления через UI нет — объект существует лишь для ручной правки JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeletedExchange {
    /// Момент удаления (для ориентира при ручном поиске нужной записи).
    pub deleted_at: DateTime<Utc>,
    /// Удалённые сообщения. Для `Ctrl+E` (удаление обмена) — сообщение
    /// пользователя и ответ ассистента; для `Ctrl+R` (перегенерация) — ответ
    /// ассистента (и связанные tool-сообщения раунда).
    pub messages: Vec<Message>,
    /// Содержимое поля ввода на момент удаления — до того, как туда вернулся текст
    /// удалённого сообщения пользователя (`Ctrl+E`) или начался новый ход (`Ctrl+R`).
    pub draft: String,
    /// Что вызвало удаление — поведенческий сигнал собеседника (или самого агента).
    /// Авто-рефлексия читает его как косвенное свидетельство («перегенерировал =
    /// ответ, вероятно, не устроил»). `None` у старых записей (без миграции).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<DeletedCause>,
}

/// Причина удаления обмена — поведенческий сигнал для авто-рефлексии (§этап 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeletedCause {
    /// `Ctrl+E` — собеседник удалил обмен (ответ его не устроил / передумал спрашивать).
    DeleteExchange,
    /// `Ctrl+R` — собеседник перегенерировал ответ (ответ, вероятно, не устроил).
    Regenerate,
    /// `rewrite_current_message` — сам ассистент переписал свою реплику (сигнал о
    /// собственном поведении, не приписывается собеседнику).
    Rewrite,
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
            draft: String::new(),
            deleted: Vec::new(),
            reflected_upto: None,
            reflected_at: None,
            is_hidden: false,
        }
    }

    /// Добавляет сообщение и обновляет `modified_at`.
    pub fn push_message(&mut self, message: Message) {
        self.messages.push(message);
        self.modified_at = Utc::now();
    }

    /// Записывает удалённый обмен (`Ctrl+E`/`Ctrl+R`) в коллекцию `deleted` ради
    /// ручного восстановления. Пустой набор сообщений игнорируется. Новая запись
    /// добавляется в **начало** коллекции (свежие удаления искать быстрее).
    /// `modified_at` **не** трогаем здесь — его обновляют сами операции усечения.
    pub fn record_deleted(&mut self, messages: Vec<Message>, draft: String, cause: DeletedCause) {
        if messages.is_empty() {
            return;
        }
        self.deleted.insert(
            0,
            DeletedExchange {
                deleted_at: Utc::now(),
                messages,
                draft,
                cause: Some(cause),
            },
        );
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
    fn record_deleted_appends_with_draft_and_skips_empty() {
        let p = Profile::new("X", "s");
        let mut chat = Chat::from_profile(&p, "t");
        chat.record_deleted(vec![], "ignored".into(), DeletedCause::DeleteExchange);
        assert!(chat.deleted.is_empty()); // пустой набор не записывается

        chat.record_deleted(
            vec![Message::user("hi"), Message::assistant("hello")],
            "набранный, но не отправленный текст".into(),
            DeletedCause::DeleteExchange,
        );
        assert_eq!(chat.deleted.len(), 1);
        assert_eq!(chat.deleted[0].messages.len(), 2);
        assert_eq!(chat.deleted[0].draft, "набранный, но не отправленный текст");
        assert_eq!(chat.deleted[0].cause, Some(DeletedCause::DeleteExchange));
    }

    #[test]
    fn deleted_empty_is_not_serialized() {
        let p = Profile::new("X", "s");
        let chat = Chat::from_profile(&p, "t");
        let json = serde_json::to_string(&chat).unwrap();
        assert!(!json.contains("deleted"));
    }

    #[test]
    fn serde_roundtrip() {
        let p = Profile::new("X", "s");
        let mut chat = Chat::from_profile(&p, "t");
        chat.push_message(Message::user("hi"));
        chat.push_message(Message::assistant("hello"));
        chat.record_deleted(
            vec![Message::user("удалённое")],
            "черновик".into(),
            DeletedCause::Regenerate,
        );
        let json = serde_json::to_string(&chat).unwrap();
        let back: Chat = serde_json::from_str(&json).unwrap();
        assert_eq!(chat, back);
    }

    #[test]
    fn deserializes_old_json_without_reflection_watermark() {
        // Старый файл чата (до этапа 3) не имеет полей ватермарка рефлексии — читается
        // без миграции, поля — дефолтные `None`. И не сериализуются, когда пусты.
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "profile_id": "00000000-0000-0000-0000-000000000002",
            "title": "старый чат",
            "created_at": "2026-01-01T00:00:00Z",
            "modified_at": "2026-01-01T00:00:00Z",
            "system_message": "s",
            "messages": []
        }"#;
        let chat: Chat = serde_json::from_str(json).unwrap();
        assert_eq!(chat.reflected_upto, None);
        assert_eq!(chat.reflected_at, None);
        // Пустые поля ватермарка не засоряют JSON (skip_serializing_if).
        let out = serde_json::to_string(&chat).unwrap();
        assert!(!out.contains("reflected_upto"));
        assert!(!out.contains("reflected_at"));
    }
}

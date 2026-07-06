//! Метаданные каталога инструментов для UI настроек (`screens/settings.rs`):
//! смысловая группа, короткое описание и глобальный гейт. Живут в `features/tools`
//! (FSD: `screens` берёт их отсюда, а не хардкодит). Порядок групп — [`TOOL_GROUPS`].

/// Глобальный выключатель, гейтящий инструмент (зеркало [`super::effective_tool_ids`]).
/// Инструмент недоступен модели, пока соответствующий выключатель выключен, даже
/// если он включён в профиле. Человекочитаемое имя выключателя даёт UI-слой
/// (`screens/settings.rs::gate_hint`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolGate {
    Web,
    Python,
    Fs,
}

/// Порядок смысловых групп инструментов (для стабильной раскладки тумблеров профиля).
pub const TOOL_GROUPS: [&str; 8] = [
    "Интроспекция",
    "Память и знания",
    "Внешний мир",
    "Файлы",
    "Утилиты",
    "Субагент",
    "Управление беседой",
    "Модель себя",
];

/// Смысловая группа инструмента (заголовок группы в тумблерах профиля).
pub fn tool_group(id: &str) -> &'static str {
    match id {
        "get_sampling"
        | "set_sampling"
        | "get_system_message"
        | "set_system_message"
        | "get_last_user_message_time" => "Интроспекция",
        "note_save" | "note_recall" | "note_revise" | "note_link" | "note_neighbors"
        | "note_supersede" | "note_merge" | "consolidate_notes" | "note_cite_source"
        | "rag_add" | "rag_search" => "Память и знания",
        "web_search" | "fetch_url" | "python_exec" => "Внешний мир",
        "fs_read" | "fs_write" | "fs_list" => "Файлы",
        "calculate" | "current_time" => "Утилиты",
        "call_subagent" => "Субагент",
        "send_followup_message" | "rewrite_current_message" => "Управление беседой",
        "get_self_model" | "reflect" | "update_self_model" | "update_user_model"
        | "add_insight" => "Модель себя",
        _ => "Прочее",
    }
}

/// Короткое (2–4 слова) описание инструмента для тумблеров профиля. Пусто — нет.
pub fn tool_description(id: &str) -> &'static str {
    match id {
        "get_sampling" => "показать семплинг",
        "set_sampling" => "изменить семплинг",
        "get_system_message" => "показать сис. сообщение",
        "set_system_message" => "изменить сис. сообщение",
        "get_last_user_message_time" => "время посл. сообщения",
        "note_save" => "сохранить заметку",
        "note_recall" => "найти заметки",
        "note_revise" => "переписать заметку",
        "note_link" => "связать заметки",
        "note_neighbors" => "связи заметки",
        "note_supersede" => "заместить заметку",
        "note_merge" => "слить заметки",
        "consolidate_notes" => "консолидация заметок",
        "note_cite_source" => "сослаться на источник",
        "rag_add" => "добавить в базу знаний",
        "rag_search" => "поиск в базе знаний",
        "call_subagent" => "запрос суб-агенту",
        "calculate" => "калькулятор",
        "current_time" => "текущее время",
        "web_search" => "поиск в интернете",
        "fetch_url" => "загрузить страницу",
        "python_exec" => "исполнить Python",
        "fs_read" => "прочитать файл",
        "fs_write" => "записать файл",
        "fs_list" => "список файлов",
        "send_followup_message" => "дописать сообщение",
        "rewrite_current_message" => "переписать ответ",
        "get_self_model" => "показать модель себя",
        "reflect" => "саморефлексия",
        "update_self_model" => "обновить модель себя",
        "update_user_model" => "обновить собеседника",
        "add_insight" => "добавить наблюдение",
        _ => "",
    }
}

/// Глобальный гейт инструмента (если есть). См. [`ToolGate`].
pub fn tool_gate(id: &str) -> Option<ToolGate> {
    match id {
        "web_search" | "fetch_url" => Some(ToolGate::Web),
        "python_exec" => Some(ToolGate::Python),
        "fs_read" | "fs_write" | "fs_list" => Some(ToolGate::Fs),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::tools::all_tool_ids;

    #[test]
    fn every_tool_has_group_and_description() {
        for id in all_tool_ids() {
            assert_ne!(tool_group(&id), "Прочее", "нет группы для {id}");
            assert!(!tool_description(&id).is_empty(), "нет описания для {id}");
            // Группа должна входить в известный порядок групп.
            assert!(
                TOOL_GROUPS.contains(&tool_group(&id)),
                "чужая группа у {id}"
            );
        }
    }

    #[test]
    fn gated_tools_map_to_switches() {
        assert_eq!(tool_gate("web_search"), Some(ToolGate::Web));
        assert_eq!(tool_gate("fetch_url"), Some(ToolGate::Web));
        assert_eq!(tool_gate("python_exec"), Some(ToolGate::Python));
        assert_eq!(tool_gate("fs_write"), Some(ToolGate::Fs));
        assert_eq!(tool_gate("note_save"), None);
    }
}

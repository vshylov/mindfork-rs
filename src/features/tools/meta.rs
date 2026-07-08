//! Типы метаданных каталога инструментов для UI настроек (`screens/settings.rs`):
//! смысловая группа [`ToolGroup`], глобальный гейт [`ToolGate`] и снимок [`ToolInfo`].
//! Живут в `features/tools` (FSD: `screens` берёт их отсюда, а не хардкодит).
//!
//! Сами значения (группа/лейбл/гейт/дефолт) объявляет каждый инструмент в трейте
//! [`super::Tool`] — единый источник истины; каталог [`super::tool_catalog`] снимает
//! их с реестра, а не из match-таблиц.

use crate::entities::profile::ToolId;

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

/// Смысловая группа инструмента (заголовок группы в тумблерах профиля). Порядок
/// вариантов = порядок показа групп (используется `Ord` для стабильной раскладки).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolGroup {
    Introspection,
    Memory,
    ExternalWorld,
    Files,
    Utils,
    Subagent,
    Conversation,
    SelfModel,
}

impl ToolGroup {
    /// Все группы в порядке показа.
    pub const ALL: [ToolGroup; 8] = [
        ToolGroup::Introspection,
        ToolGroup::Memory,
        ToolGroup::ExternalWorld,
        ToolGroup::Files,
        ToolGroup::Utils,
        ToolGroup::Subagent,
        ToolGroup::Conversation,
        ToolGroup::SelfModel,
    ];

    /// Человекочитаемый заголовок группы.
    pub fn title(&self) -> &'static str {
        match self {
            ToolGroup::Introspection => "Интроспекция",
            ToolGroup::Memory => "Память и знания",
            ToolGroup::ExternalWorld => "Внешний мир",
            ToolGroup::Files => "Файлы",
            ToolGroup::Utils => "Утилиты",
            ToolGroup::Subagent => "Субагент",
            ToolGroup::Conversation => "Управление беседой",
            ToolGroup::SelfModel => "Модель себя",
        }
    }
}

/// Заголовки всех групп в порядке показа (для проверки принадлежности в UI/тестах).
#[allow(dead_code)]
pub fn group_titles() -> [&'static str; 8] {
    ToolGroup::ALL.map(|g| g.title())
}

/// Снимок метаданных одного инструмента (без живого `Arc<dyn Tool>`) — для
/// FSD-чистого потребления UI-слоем. Строится [`super::tool_catalog`] из реестра.
#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub id: ToolId,
    pub group: ToolGroup,
    /// Короткое (2–4 слова) описание для тумблеров профиля.
    pub label: &'static str,
    pub gate: Option<ToolGate>,
    pub enabled_by_default: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::tools::tool_catalog;

    #[test]
    fn every_tool_has_group_and_label() {
        let titles = group_titles();
        for info in tool_catalog() {
            assert!(!info.label.is_empty(), "нет описания для {}", info.id);
            assert!(
                titles.contains(&info.group.title()),
                "чужая группа у {}",
                info.id
            );
        }
    }

    #[test]
    fn gated_tools_map_to_switches() {
        let catalog = tool_catalog();
        let gate = |id: &str| catalog.iter().find(|i| i.id == id).and_then(|i| i.gate);
        assert_eq!(gate("web_search"), Some(ToolGate::Web));
        assert_eq!(gate("fetch_url"), Some(ToolGate::Web));
        assert_eq!(gate("python_exec"), Some(ToolGate::Python));
        assert_eq!(gate("fs_write"), Some(ToolGate::Fs));
        assert_eq!(gate("note_save"), None);
    }
}

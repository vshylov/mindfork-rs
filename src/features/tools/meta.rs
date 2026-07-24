//! Metadata types for the tool catalog for the settings UI (`screens/settings.rs`):
//! the semantic group [`ToolGroup`], the global gate [`ToolGate`], and the
//! [`ToolInfo`] snapshot. Live in `features/tools` (FSD: `screens` takes them from
//! here rather than hardcoding).
//!
//! Each tool declares the actual values (group/label/gate/default) in the
//! [`super::Tool`] trait — the single source of truth; the [`super::tool_catalog`]
//! catalog reads them off the registry, not from match tables.

use crate::entities::profile::ToolId;

/// Global switch gating a tool (mirrors [`super::effective_tool_ids`]). A tool is
/// unavailable to the model while the corresponding switch is off, even if it's
/// enabled in the profile. The UI layer supplies the switch's human-readable name
/// (`screens/settings.rs::gate_hint`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolGate {
    Web,
    Python,
    Fs,
    /// MCP-host master switch (`config.mcp.enabled`). Gates all MCP-server tools
    /// (id with the `mcp__` prefix). Checked by id prefix in
    /// [`super::effective_tool_ids`] (the tools are dynamic — absent from the
    /// static `CATALOG`).
    Mcp,
}

/// Semantic group of a tool (the group heading in profile toggles). Variant order
/// = the order groups are shown (uses `Ord` for stable layout).
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
    /// MCP-server tools (plugins, docs/research/plugin-system.md §4).
    Plugins,
}

impl ToolGroup {
    /// All groups in display order.
    pub const ALL: [ToolGroup; 9] = [
        ToolGroup::Introspection,
        ToolGroup::Memory,
        ToolGroup::ExternalWorld,
        ToolGroup::Files,
        ToolGroup::Utils,
        ToolGroup::Subagent,
        ToolGroup::Conversation,
        ToolGroup::SelfModel,
        ToolGroup::Plugins,
    ];

    /// Bundle key for the group's UI header (axis B, resolved in the settings
    /// layer). [`Self::title`] remains the Russian `&'static` fallback.
    pub fn i18n_key(&self) -> &'static str {
        match self {
            ToolGroup::Introspection => "ui.tool.group.introspection",
            ToolGroup::Memory => "ui.tool.group.memory",
            ToolGroup::ExternalWorld => "ui.tool.group.external_world",
            ToolGroup::Files => "ui.tool.group.files",
            ToolGroup::Utils => "ui.tool.group.utils",
            ToolGroup::Subagent => "ui.tool.group.subagent",
            ToolGroup::Conversation => "ui.tool.group.conversation",
            ToolGroup::SelfModel => "ui.tool.group.self_model",
            ToolGroup::Plugins => "ui.tool.group.plugins",
        }
    }

    /// Human-readable group header.
    pub fn title(&self) -> &'static str {
        match self {
            ToolGroup::Introspection => "Introspection",
            ToolGroup::Memory => "Memory and knowledge",
            ToolGroup::ExternalWorld => "External world",
            ToolGroup::Files => "Files",
            ToolGroup::Utils => "Utilities",
            ToolGroup::Subagent => "Subagent",
            ToolGroup::Conversation => "Conversation control",
            ToolGroup::SelfModel => "Self-model",
            ToolGroup::Plugins => "Plugins (MCP)",
        }
    }
}

/// Headers of all groups in display order (for membership checks in UI/tests).
#[allow(dead_code)]
pub fn group_titles() -> [&'static str; 9] {
    ToolGroup::ALL.map(|g| g.title())
}

/// Metadata snapshot of a single tool (without a live `Arc<dyn Tool>`) — for
/// FSD-clean consumption by the UI layer. Built by [`super::tool_catalog`] from
/// the registry.
#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub id: ToolId,
    pub group: ToolGroup,
    /// Short (2-4 word) description for the profile toggle.
    pub label: &'static str,
    pub gate: Option<ToolGate>,
    pub enabled_by_default: bool,
    /// Full tool description for the settings bottom panel. Only filled in for
    /// dynamic MCP tools (server-supplied text — showing the full description in
    /// the UI is mandatory as a tool-poisoning antidote,
    /// docs/research/plugin-system.md §4.5); for built-in ones — `None` (their
    /// descriptions are LLM-oriented and live in the locale bundles).
    pub description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::tools::tool_catalog;

    #[test]
    fn every_tool_has_group_and_label() {
        let titles = group_titles();
        for info in tool_catalog() {
            assert!(!info.label.is_empty(), "no description for {}", info.id);
            assert!(
                titles.contains(&info.group.title()),
                "foreign group for {}",
                info.id
            );
        }
    }

    #[test]
    fn ui_label_and_group_keys_exist_in_all_bundles() {
        // Dynamic keys `ui.tool.label.{id}` / `ui.tool.group.*` are assembled via
        // `format!`/`i18n_key()` in the settings layer and are NOT caught by the
        // generic code scanner (`all_ui_keys_referenced_in_code_exist_in_bundle`).
        // A direct completeness gate: every catalog tool has a label key in both
        // built-in languages, every group has its `i18n_key`.
        use crate::shared::i18n::{Lang, locale};
        for &lang in Lang::ALL {
            let loc = locale(lang);
            for info in tool_catalog() {
                let k = format!("ui.tool.label.{}", info.id);
                assert!(loc.has_key(&k), "{lang:?}: missing key {k}");
            }
            for g in ToolGroup::ALL {
                assert!(
                    loc.has_key(g.i18n_key()),
                    "{lang:?}: missing {}",
                    g.i18n_key()
                );
            }
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

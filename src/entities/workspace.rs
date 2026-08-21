//! The code workspace attached to a chat (spec §9.12,
//! [docs/history/code-workspace.md](../../docs/history/code-workspace.md)): a project directory
//! the assistant may read, search and edit through the `code_*` tools.
//!
//! Deliberately a **struct with one field** rather than a bare
//! `Option<String>` on [`Chat`](super::chat::Chat): stage 3 added the build/run/
//! test command lines here, and growing a struct is additive (ADR 0006 §8)
//! where turning a string field into one would have been a rename — a schema
//! bump and a migration step for something already known to be coming.
//!
//! The three command lines are **one shape with three names**, which is why
//! they are reached through [`CommandSlot`] rather than by field: the tools
//! (`code_build`/`code_run`/`code_test`), the commands
//! (`/project build-cmd|run-cmd|test-cmd`, `/project clear …`) and the system
//! block all do the same thing three times, and each of them differs only in
//! which slot it names.

use serde::{Deserialize, Serialize};

/// One of the three user-authored command lines a project may carry
/// (spec §9.12, design fork F3).
///
/// Three slots and not an open list: the point of the feature is that the model
/// **cannot compose a command**, and a fixed vocabulary is what makes that
/// checkable — the tool schema takes no arguments at all, so there is nothing to
/// inject into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandSlot {
    Build,
    Run,
    Test,
}

impl CommandSlot {
    /// Every slot, so a caller that handles one cannot silently forget the
    /// other two.
    pub const ALL: [CommandSlot; 3] = [Self::Build, Self::Run, Self::Test];

    /// The word the user types (`/project build-cmd`, `/project clear build`)
    /// and the suffix of the tool id (`code_build`). One spelling for both, so
    /// the command and the tool it drives cannot drift apart.
    pub fn key(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Run => "run",
            Self::Test => "test",
        }
    }

    /// The `/project` subcommand that fills this slot. Every refusal and every
    /// "nothing here" note names it, so it is spelled **once** — in the tool's
    /// answer to the model and in the feed's answer to the user alike.
    pub fn setter_command(self) -> String {
        format!("/project {}-cmd", self.key())
    }

    /// Parses the word back, case-insensitively (`/project clear BUILD`).
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|slot| slot.key() == s)
    }
}

/// A project directory attached to one chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    /// Absolute path to the project root, canonicalized when it was attached.
    ///
    /// Stored as it will be *shown* — on Windows `canonicalize` yields the
    /// `\\?\C:\…` verbatim form, which is correct and unreadable, and would
    /// travel into the system prompt and every tool result. The path is
    /// re-resolved on use anyway, so keeping the readable form costs nothing.
    pub root: String,
    /// The line `code_build` runs, verbatim, as the user typed it. `None` — no
    /// line, and then the tool is not offered to the model at all (spec §9.12).
    ///
    /// Additive with `skip_serializing_if`: a project attached before stage 3
    /// reads back unchanged, and one with no commands writes no new key (ADR
    /// 0006 §8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_cmd: Option<String>,
    /// The line `code_run` runs. See [`Self::build_cmd`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_cmd: Option<String>,
    /// The line `code_test` runs. See [`Self::build_cmd`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_cmd: Option<String>,
}

impl Workspace {
    pub fn new(root: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            build_cmd: None,
            run_cmd: None,
            test_cmd: None,
        }
    }

    /// The line stored in `slot`, if any. Whitespace-only counts as unset: a
    /// chat file can be edited by hand, and a blank line is not a command.
    pub fn command(&self, slot: CommandSlot) -> Option<&str> {
        let raw = match slot {
            CommandSlot::Build => &self.build_cmd,
            CommandSlot::Run => &self.run_cmd,
            CommandSlot::Test => &self.test_cmd,
        };
        raw.as_deref().map(str::trim).filter(|s| !s.is_empty())
    }

    /// Sets or clears `slot`. A line that is only whitespace clears it, for the
    /// same reason [`Self::command`] ignores one.
    pub fn set_command(&mut self, slot: CommandSlot, line: Option<String>) {
        let line = line.map(|l| l.trim().to_string()).filter(|l| !l.is_empty());
        match slot {
            CommandSlot::Build => self.build_cmd = line,
            CommandSlot::Run => self.run_cmd = line,
            CommandSlot::Test => self.test_cmd = line,
        }
    }

    /// The last path component — what a status line calls the project.
    /// Falls back to the whole path for a root with no component of its own
    /// (`/`, `C:\`).
    ///
    /// Split by hand rather than through `Path::file_name`, which only knows
    /// **the host's** separators: the app's data is portable between machines,
    /// so a root canonicalized on Windows can be read back on Linux, where
    /// `C:\Projects\app` is one component and the label would come out as the
    /// whole path. Caught by CI — the test passed on Windows and failed on
    /// Linux.
    pub fn name(&self) -> &str {
        let trimmed = self.root.trim_end_matches(['/', '\\']);
        let last = trimmed.rsplit(['/', '\\']).next().unwrap_or("");
        // Nothing but separators (`/`), or a bare drive (`C:\` → `C:`): there is
        // no component to show, and an empty label would read as "no project".
        if last.is_empty() || last.ends_with(':') {
            return &self.root;
        }
        last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both separators, on **both** platforms: the root is stored in a chat file
    /// that can travel to another machine, so a Windows path read on Linux (and
    /// the reverse) must still name the project rather than repeat the path.
    #[test]
    fn name_is_the_last_component_whatever_the_separator() {
        assert_eq!(
            Workspace::new("D:/Projects/mindfork-rs").name(),
            "mindfork-rs"
        );
        assert_eq!(Workspace::new("/home/u/app/").name(), "app");
        assert_eq!(Workspace::new(r"C:\Projects\app").name(), "app");
        assert_eq!(Workspace::new(r"C:\Projects\app\").name(), "app");
    }

    /// The slot accessors, over the whole vocabulary rather than three copies:
    /// a per-slot test trio is the sliding self-duplicate the duplication gate
    /// reads as one block written three times (docs/lessons.md §2), and looping
    /// is also what makes a fourth slot impossible to forget.
    #[test]
    fn a_slot_stores_shows_and_clears_a_line() {
        let mut ws = Workspace::new("/p");
        for slot in CommandSlot::ALL {
            assert_eq!(ws.command(slot), None);
            ws.set_command(slot, Some(format!("cargo {}", slot.key())));
            assert_eq!(ws.command(slot).unwrap(), format!("cargo {}", slot.key()));
            ws.set_command(slot, None);
            assert_eq!(ws.command(slot), None);
        }
        // Every slot is independent: setting one must not be visible in another.
        ws.set_command(CommandSlot::Build, Some("cargo build".into()));
        assert_eq!(ws.command(CommandSlot::Build), Some("cargo build"));
        assert_eq!(ws.command(CommandSlot::Run), None);
        assert_eq!(ws.command(CommandSlot::Test), None);
    }

    /// A chat file is JSON on disk and can be edited by hand. A line of spaces
    /// is not a command, and treating it as one would offer the model a tool
    /// whose only possible outcome is a spawn failure.
    #[test]
    fn a_blank_line_counts_as_unset() {
        let mut ws = Workspace::new("/p");
        ws.set_command(CommandSlot::Build, Some("   ".into()));
        assert_eq!(ws.command(CommandSlot::Build), None);
        assert_eq!(ws.build_cmd, None, "a blank line must not be stored either");
    }

    /// The additive-field property, the same one
    /// `renamed_manually_is_additive_and_round_trips` pins for `Chat`: a project
    /// attached before stage 3 must read back unchanged, and one with no
    /// commands must not write the new keys at all (ADR 0006 §8).
    #[test]
    fn command_slots_are_additive_and_round_trip() {
        let stage1: Workspace = serde_json::from_str(r#"{"root":"/p"}"#).unwrap();
        assert_eq!(stage1.root, "/p");
        assert_eq!(stage1.command(CommandSlot::Build), None);
        assert_eq!(
            serde_json::to_string(&stage1).unwrap(),
            r#"{"root":"/p"}"#,
            "a project with no commands must write no new key"
        );

        let mut ws = Workspace::new("/p");
        ws.set_command(CommandSlot::Test, Some("cargo test".into()));
        let json = serde_json::to_string(&ws).unwrap();
        assert_eq!(serde_json::from_str::<Workspace>(&json).unwrap(), ws);
    }

    /// A root with nothing after the separator has no component to show; the
    /// whole path is the honest answer, and an empty label would read as "no
    /// project attached".
    #[test]
    fn a_rootless_path_falls_back_to_itself() {
        assert_eq!(Workspace::new("/").name(), "/");
        assert_eq!(Workspace::new(r"C:\").name(), r"C:\");
    }
}

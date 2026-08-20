//! The code workspace attached to a chat (spec §9.12,
//! [docs/code-workspace.md](../../docs/code-workspace.md)): a project directory
//! the assistant may read, search and edit through the `code_*` tools.
//!
//! Deliberately a **struct with one field** rather than a bare
//! `Option<String>` on [`Chat`](super::chat::Chat): stage 3 adds the build/run/
//! test command lines here, and growing a struct is additive (ADR 0006 §8)
//! where turning a string field into one would be a rename — a schema bump and
//! a migration step for something we already know is coming.

use serde::{Deserialize, Serialize};

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
}

impl Workspace {
    pub fn new(root: impl Into<String>) -> Self {
        Self { root: root.into() }
    }

    /// The last path component — what a status line calls the project.
    /// Falls back to the whole path for a root with no file name (`C:\`, `/`).
    pub fn name(&self) -> &str {
        std::path::Path::new(self.root.trim_end_matches(['/', '\\']))
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| !n.is_empty())
            .unwrap_or(&self.root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_the_last_component() {
        assert_eq!(
            Workspace::new("D:/Projects/mindfork-rs").name(),
            "mindfork-rs"
        );
        assert_eq!(Workspace::new("/home/u/app/").name(), "app");
        assert_eq!(Workspace::new(r"C:\Projects\app").name(), "app");
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

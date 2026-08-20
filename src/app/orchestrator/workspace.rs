//! The chat's code workspace (`/project attach|detach|status`, spec §9.12):
//! validating the directory and storing it on the chat. Part of the [`super`]
//! module.
//!
//! Deliberately synchronous, unlike `/file attach`: attaching a project reads no
//! file, it only checks that a directory exists — a `stat`, not I/O worth a
//! background task. What the model can then *do* with it is the `code_*` tools'
//! problem, and they run inside a turn.
//!
//! See docs/code-workspace.md.

use std::path::PathBuf;

use crate::app::events::AppEvent;
use crate::entities::workspace::Workspace;
use crate::features::project_command::ProjectProgress;

use super::Orchestrator;

impl Orchestrator {
    /// Attaches a project directory to the active chat (`/project attach`).
    pub(super) fn handle_project_attach(&mut self, path: String) {
        let path = path.trim().to_string();
        if path.is_empty() {
            return;
        }
        let Some(chat_id) = self.active_id else {
            self.fail_project(self.ui_locale().t("ui.err.project_no_active_chat"));
            return;
        };
        let root = match canonical_dir(&path) {
            Ok(root) => root,
            Err(err) => {
                let msg = self
                    .ui_locale()
                    .tf("ui.err.project_bad_dir", &[("path", &path), ("err", &err)]);
                self.fail_project(&msg);
                return;
            }
        };
        let workspace = Workspace::new(root.clone());
        let name = workspace.name().to_string();
        let Some(chat) = self.chat_mut(chat_id) else {
            self.fail_project(self.ui_locale().t("ui.err.project_no_active_chat"));
            return;
        };
        chat.workspace = Some(workspace);
        // Deliberately **not** touching `modified_at`: attaching a project is a
        // change to the chat's setup, not to the conversation, and it must not
        // push the chat up the list (the `draft`/`feed_view` rule).
        self.mark_dirty(chat_id);
        self.emit_project(ProjectProgress::Attached { root, name });
    }

    /// Detaches the active chat's project (`/project detach`).
    pub(super) fn handle_project_detach(&mut self) {
        let Some(chat_id) = self.active_id else {
            self.fail_project(self.ui_locale().t("ui.err.project_no_active_chat"));
            return;
        };
        let Some(chat) = self.chat_mut(chat_id) else {
            self.fail_project(self.ui_locale().t("ui.err.project_no_active_chat"));
            return;
        };
        // Detaching what is not attached is not an error, but it must not be
        // silent either: a note saying "nothing was attached" is what tells the
        // user their earlier attach never landed.
        let Some(previous) = chat.workspace.take() else {
            self.emit_project(ProjectProgress::Status { root: None });
            return;
        };
        self.mark_dirty(chat_id);
        // The journal describes files of a project this chat no longer has, and
        // the changes screen would otherwise offer to revert into a directory
        // the user has let go of. Best-effort: a journal that cannot be removed
        // is a log line, not a reason to refuse the detach.
        let dir = self
            .storage
            .json()
            .workspace_dir()
            .join(chat_id.to_string());
        if let Err(err) = crate::features::workspace_journal::Journal::new(dir).clear() {
            tracing::warn!("could not clear the workspace journal: {err}");
        }
        self.emit_project(ProjectProgress::Detached {
            root: previous.root,
        });
    }

    /// Reports the active chat's project (`/project status`).
    pub(super) fn handle_project_status(&mut self) {
        let root = self
            .active_id
            .and_then(|id| self.chats.iter().find(|c| c.id == id))
            .and_then(|c| c.workspace.as_ref())
            .map(|w| w.root.clone());
        self.emit_project(ProjectProgress::Status { root });
    }

    fn emit_project(&self, progress: ProjectProgress) {
        let _ = self.evt_tx.send(AppEvent::ProjectProgress(progress));
    }

    fn fail_project(&self, msg: &str) {
        self.emit_project(ProjectProgress::Failed(msg.to_string()));
    }
}

/// Canonicalizes `path` and insists it is a directory.
///
/// Returns the **readable** form: on Windows `canonicalize` yields `\\?\C:\…`,
/// which would then travel into the system prompt, into every tool result, and
/// back from the model as a path we would have to accept. Errors are the OS's
/// own text — the caller wraps them in a localized sentence.
fn canonical_dir(path: &str) -> Result<String, String> {
    let expanded = PathBuf::from(path);
    let canonical = expanded.canonicalize().map_err(|e| e.to_string())?;
    if !canonical.is_dir() {
        return Err(String::from("not a directory"));
    }
    let shown = canonical.display().to_string();
    Ok(shown
        .strip_prefix(r"\\?\")
        .map(str::to_string)
        .unwrap_or(shown))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_canonicalizes_to_a_readable_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = canonical_dir(&dir.path().to_string_lossy()).unwrap();
        assert!(
            !root.starts_with(r"\\?\"),
            "the verbatim prefix must not survive: {root}"
        );
        assert!(PathBuf::from(&root).is_dir());
    }

    /// A file is the mistake worth naming separately: `/project attach` on a
    /// path that exists but is not a directory would otherwise read as success.
    #[test]
    fn a_file_is_not_a_project() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(canonical_dir(&file.to_string_lossy()).is_err());
    }

    #[test]
    fn a_missing_path_is_refused() {
        assert!(canonical_dir("definitely-not-a-real-directory-xyz").is_err());
    }
}

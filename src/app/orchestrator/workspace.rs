//! The chat's code workspace (`/project attach|detach|status`, spec §9.12):
//! validating the directory and storing it on the chat. Part of the [`super`]
//! module.
//!
//! Deliberately synchronous, unlike `/file attach`: attaching a project reads no
//! file, it only checks that a directory exists — a `stat`, not I/O worth a
//! background task. What the model can then *do* with it is the `code_*` tools'
//! problem, and they run inside a turn.
//!
//! See docs/history/code-workspace.md.

use std::path::PathBuf;

use crate::app::events::AppEvent;
use crate::entities::workspace::{CommandSlot, Workspace};
use crate::features::project_command::{ProjectProgress, SlotAction};

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
            self.emit_project(Self::nothing_attached());
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

    /// Reports the active chat's project and its command slots
    /// (`/project status`).
    pub(super) fn handle_project_status(&mut self) {
        let ws = self
            .active_id
            .and_then(|id| self.chats.iter().find(|c| c.id == id))
            .and_then(|c| c.workspace.as_ref());
        let progress = match ws {
            Some(ws) => ProjectProgress::Status {
                root: Some(ws.root.clone()),
                commands: CommandSlot::ALL
                    .into_iter()
                    .map(|slot| (slot, ws.command(slot).map(str::to_string)))
                    .collect(),
            },
            None => Self::nothing_attached(),
        };
        self.emit_project(progress);
    }

    /// The "no project here" answer, shared by every command that needs one.
    fn nothing_attached() -> ProjectProgress {
        ProjectProgress::Status {
            root: None,
            commands: Vec::new(),
        }
    }

    /// Sets, shows or clears one of the project's command slots
    /// (`/project build-cmd|run-cmd|test-cmd`, `/project clear <slot>`).
    ///
    /// One handler for the three slots and the three actions: they differ in
    /// which field they touch and in nothing else, and a per-slot copy is the
    /// shape a duplication gate reads as one block written three times
    /// (docs/lessons.md §2).
    pub(super) fn handle_project_slot(&mut self, slot: CommandSlot, action: SlotAction) {
        let Some(chat_id) = self.active_id else {
            self.fail_project(self.ui_locale().t("ui.err.project_no_active_chat"));
            return;
        };
        // Every one of these needs a project: a command line belongs to a
        // directory, and storing one for a chat with nothing attached would be
        // configuring a thing that does not exist.
        let attached = self
            .chats
            .iter()
            .any(|c| c.id == chat_id && c.workspace.is_some());
        if !attached {
            self.emit_project(Self::nothing_attached());
            return;
        }
        // The shell check runs **here**, when the line is set, rather than when
        // a model first tries to run it three turns later: `cargo build | tee
        // log.txt` cannot work in an application that spawns the program itself,
        // and finding that out at set time is the difference between an answer
        // and a mystery (docs/lessons.md §4). `code_build` checks again, because
        // a chat file can be edited by hand.
        if let SlotAction::Set(line) = &action
            && let Some(ch) = crate::shared::cmdline::shell_syntax(line)
        {
            self.emit_project(ProjectProgress::CommandRefused {
                line: line.clone(),
                ch,
            });
            return;
        }
        // Showing changes nothing, so it never reaches the mutation below.
        if action == SlotAction::Show {
            let line = self
                .chats
                .iter()
                .find(|c| c.id == chat_id)
                .and_then(|c| c.workspace.as_ref())
                .and_then(|w| w.command(slot))
                .map(str::to_string);
            self.emit_project(ProjectProgress::CommandShown { slot, line });
            return;
        }
        let Some(ws) = self
            .chat_mut(chat_id)
            .and_then(|chat| chat.workspace.as_mut())
        else {
            self.fail_project(self.ui_locale().t("ui.err.project_no_active_chat"));
            return;
        };
        let had = ws.command(slot).is_some();
        let line = match &action {
            SlotAction::Set(line) => Some(line.clone()),
            _ => None,
        };
        ws.set_command(slot, line.clone());
        // Like attaching: the chat's setup changed, the conversation did not, so
        // `modified_at` stays where it was and the chat keeps its place in the
        // list (the `draft`/`feed_view` rule).
        self.mark_dirty(chat_id);
        // Decided after the write, from what the write knew: clearing a slot
        // that was already empty is not an error and must not be silent either —
        // saying so is what tells the user their earlier `build-cmd` never
        // landed.
        self.emit_project(match line {
            Some(line) => ProjectProgress::CommandSet { slot, line },
            None => ProjectProgress::CommandCleared { slot, had },
        });
    }

    /// Builds the change set for the active chat and sends it to the screen
    /// (`F4` / `/changes`).
    ///
    /// Off the runtime: it reads every journaled file and diffs it, which is
    /// synchronous I/O that would otherwise stall the UI thread's bridge. The
    /// event goes straight out rather than back through the orchestrator —
    /// nothing here changes orchestrator state.
    pub(super) fn handle_open_changes(&mut self) {
        let Some((dir, root)) = self.workspace_paths() else {
            // No chat, or no project: an empty set, and the screen says what
            // would put something in it (docs/lessons.md §4).
            self.emit_changes(Default::default());
            return;
        };
        let tx = self.evt_tx.clone();
        tokio::task::spawn_blocking(move || {
            let set = crate::features::workspace_diff::build(&dir, &root);
            let _ = tx.send(AppEvent::WorkspaceChanges(Box::new(set)));
        });
    }

    /// Puts one file back and re-sends the change set, so the screen shows the
    /// result rather than what it asked for.
    ///
    /// A failure is reported as a `/project` note in the feed: the screen the
    /// user is looking at has no error line of its own, and a revert that
    /// silently did nothing is the worst of the three outcomes.
    pub(super) fn handle_revert_workspace_file(&mut self, path: String) {
        let Some((dir, root)) = self.workspace_paths() else {
            self.emit_changes(Default::default());
            return;
        };
        let tx = self.evt_tx.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(err) = crate::features::workspace_diff::revert(&dir, &root, &path) {
                let _ = tx.send(AppEvent::ProjectProgress(ProjectProgress::Failed(
                    err.to_string(),
                )));
            }
            let set = crate::features::workspace_diff::build(&dir, &root);
            let _ = tx.send(AppEvent::WorkspaceChanges(Box::new(set)));
        });
    }

    /// The active chat's journal directory and project root, when it has both.
    fn workspace_paths(&self) -> Option<(std::path::PathBuf, String)> {
        let chat_id = self.active_id?;
        let root = self
            .chats
            .iter()
            .find(|c| c.id == chat_id)
            .and_then(|c| c.workspace.as_ref())
            .map(|w| w.root.clone())?;
        let dir = self
            .storage
            .json()
            .workspace_dir()
            .join(chat_id.to_string());
        Some((dir, root))
    }

    fn emit_changes(&self, set: crate::features::workspace_diff::ChangeSet) {
        let _ = self.evt_tx.send(AppEvent::WorkspaceChanges(Box::new(set)));
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

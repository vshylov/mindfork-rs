//! The chat screen — the attached code project (`/project attach`): the feed
//! note for a command's outcome. Part of the [`super`] module.
//! See docs/code-workspace.md, spec §9.12.

use super::*;
use crate::features::project_command::ProjectProgress;

impl ChatScreen {
    /// Reports the outcome of a `/project` command as a note in the feed.
    pub fn set_project_progress(&mut self, progress: ProjectProgress) {
        match progress {
            ProjectProgress::Attached { root, name } => {
                // The note names what the assistant can now do, not just what
                // happened: an attach whose consequence is invisible reads as a
                // no-op, and the tools are the consequence (docs/lessons.md §4).
                let msg = self
                    .loc
                    .tf("ui.project.attached", &[("name", &name), ("root", &root)]);
                self.push_note(&msg);
            }
            ProjectProgress::Detached { root } => {
                let msg = self.loc.tf("ui.project.detached", &[("root", &root)]);
                self.push_note(&msg);
            }
            ProjectProgress::Status { root } => {
                let msg = match root {
                    Some(root) => self.loc.tf("ui.project.status", &[("root", &root)]),
                    // "Nothing attached" has to say how to attach one, or the
                    // answer is a dead end.
                    None => self.loc.tf(
                        "ui.project.status_none",
                        &[("usage", self.loc.t("ui.project.usage"))],
                    ),
                };
                self.push_note(&msg);
            }
            ProjectProgress::Failed(err) => {
                self.push_error(&self.loc.tf("ui.project.failed", &[("err", &err)]));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every outcome must produce a visible note: a command that answers with
    /// nothing is the silence the command-only-control track exists to remove
    /// (docs/lessons.md §4).
    #[test]
    fn every_outcome_is_reported() {
        let cases = [
            ProjectProgress::Attached {
                root: "D:/proj".into(),
                name: "proj".into(),
            },
            ProjectProgress::Detached {
                root: "D:/proj".into(),
            },
            ProjectProgress::Status {
                root: Some("D:/proj".into()),
            },
            ProjectProgress::Status { root: None },
            ProjectProgress::Failed("boom".into()),
        ];
        for case in cases {
            let mut s = ChatScreen::new();
            let before = s.feed.len();
            s.set_project_progress(case.clone());
            assert!(s.feed.len() > before, "{case:?} produced no note");
        }
    }

    /// "Nothing attached" must name the route that attaches something, and the
    /// attached note must name the root — a note that only says "done" leaves
    /// the user unable to tell *which* directory landed.
    #[test]
    fn the_notes_carry_what_the_user_needs_next() {
        let mut s = ChatScreen::new();
        s.set_project_progress(ProjectProgress::Status { root: None });
        let empty = s.feed.last().expect("a note").text.clone();
        assert!(empty.contains("/project"), "got: {empty}");

        s.set_project_progress(ProjectProgress::Attached {
            root: "D:/proj".into(),
            name: "proj".into(),
        });
        let attached = s.feed.last().expect("a note").text.clone();
        assert!(attached.contains("D:/proj"), "got: {attached}");
    }
}

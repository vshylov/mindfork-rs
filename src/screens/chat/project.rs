//! The chat screen — the attached code project (`/project attach`): the feed
//! note for a command's outcome. Part of the [`super`] module.
//! See docs/code-workspace.md, spec §9.12.

use super::*;
use crate::entities::workspace::CommandSlot;
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
            ProjectProgress::Status { root, commands } => {
                let msg = match root {
                    Some(root) => {
                        let mut msg = self.loc.tf("ui.project.status", &[("root", &root)]);
                        // Every slot, including the empty ones: the question
                        // being asked is "what can the assistant run here", and
                        // an answer that lists only what is set cannot say "none
                        // of them" (docs/lessons.md §4).
                        for (slot, line) in commands {
                            let value = match line {
                                Some(line) => line,
                                None => self.loc.t("ui.project.slot_unset").to_string(),
                            };
                            msg.push('\n');
                            msg.push_str(&self.loc.tf(
                                "ui.project.status_slot",
                                &[("slot", slot.key()), ("line", &value)],
                            ));
                        }
                        msg
                    }
                    // "Nothing attached" has to say how to attach one, or the
                    // answer is a dead end.
                    None => self.loc.tf(
                        "ui.project.status_none",
                        &[("usage", self.loc.t("ui.project.usage"))],
                    ),
                };
                self.push_note(&msg);
            }
            ProjectProgress::CommandSet { slot, line } => {
                let msg = self.loc.tf(
                    "ui.project.command_set",
                    &[("slot", slot.key()), ("line", &line)],
                );
                self.push_note(&msg);
            }
            ProjectProgress::CommandShown { slot, line } => {
                let msg = match line {
                    Some(line) => self.loc.tf(
                        "ui.project.command_shown",
                        &[("slot", slot.key()), ("line", &line)],
                    ),
                    // "Nothing here" has to name the command that puts something
                    // here, or the answer is a dead end (docs/lessons.md §4).
                    None => self.loc.tf(
                        "ui.project.command_none",
                        &[("slot", slot.key()), ("cmd", &slash_command(slot))],
                    ),
                };
                self.push_note(&msg);
            }
            ProjectProgress::CommandCleared { slot, had } => {
                let key = if had {
                    "ui.project.command_cleared"
                } else {
                    // Clearing an empty slot is not an error, and it must not be
                    // silent either: saying so is what tells the user their
                    // earlier `build-cmd` never landed.
                    "ui.project.command_was_empty"
                };
                let msg = self.loc.tf(key, &[("slot", slot.key())]);
                self.push_note(&msg);
            }
            ProjectProgress::CommandRefused { line, ch } => {
                self.push_error(&self.loc.tf(
                    "ui.project.command_shell",
                    &[("line", &line), ("char", &ch.to_string())],
                ));
            }
            ProjectProgress::Failed(err) => {
                self.push_error(&self.loc.tf("ui.project.failed", &[("err", &err)]));
            }
        }
    }
}

/// The `/project` subcommand that fills `slot` — what a "nothing here" note
/// points at.
fn slash_command(slot: CommandSlot) -> String {
    format!("/project {}-cmd", slot.key())
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
                commands: vec![
                    (CommandSlot::Build, Some("cargo build".into())),
                    (CommandSlot::Run, None),
                    (CommandSlot::Test, None),
                ],
            },
            ProjectProgress::Status {
                root: None,
                commands: Vec::new(),
            },
            ProjectProgress::CommandSet {
                slot: CommandSlot::Build,
                line: "cargo build".into(),
            },
            ProjectProgress::CommandShown {
                slot: CommandSlot::Run,
                line: Some("cargo run".into()),
            },
            ProjectProgress::CommandShown {
                slot: CommandSlot::Test,
                line: None,
            },
            ProjectProgress::CommandCleared {
                slot: CommandSlot::Test,
                had: true,
            },
            ProjectProgress::CommandCleared {
                slot: CommandSlot::Test,
                had: false,
            },
            ProjectProgress::CommandRefused {
                line: "cargo build | tee log".into(),
                ch: '|',
            },
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
        s.set_project_progress(ProjectProgress::Status {
            root: None,
            commands: Vec::new(),
        });
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

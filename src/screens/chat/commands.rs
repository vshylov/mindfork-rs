//! Running a typed command (`features::ui_command`) on the chat screen.
//!
//! **A command is its key, plus a voice.** Every arm below reaches the action
//! through the *same* handler the chord uses — `handle_ctrl_shortcut` for the
//! `Ctrl` keys, the same intent for the function keys — so the two routes cannot
//! drift into two semantics, confirmation popups and gates included. What a
//! command adds is an answer: a chord that does nothing is cheap, while a typed
//! command that vanishes reads as the app refusing to act, so every precondition
//! that blocks a command says so and names the route that works
//! (docs/lessons.md §4).
//!
//! See [docs/research/command-only-control.md](../../../docs/research/command-only-control.md)
//! §4.3 and spec §11.7.

use super::*;
use crate::features::ui_command::{self, UiCommand};

impl ChatScreen {
    /// The registry's commands (`/settings`, `/find`, `/regen`, …) — intercepted
    /// on `Enter` and never sent as a message. Returns `None` when the text is
    /// not one of them.
    ///
    /// The box is cleared first, like every other command: each keystroke has
    /// already reached the orchestrator as a draft, so leaving the command in it
    /// would greet the user with it on the next launch (journal ui-input.md,
    /// `/exit`).
    pub(super) fn try_ui_command(&mut self, text: &str) -> Option<Option<ChatIntent>> {
        let parsed = ui_command::parse(text, self.loc)?;
        self.input.clear();
        self.mark_input_changed();
        Some(match parsed {
            Ok(cmd) => self.run_ui_command(cmd.command, cmd.argument, cmd.alias),
            Err(msg) => {
                self.push_note(&msg);
                None
            }
        })
    }

    /// Dispatches a parsed command. Exhaustive on [`UiCommand`], so a new
    /// registry row cannot ship without an answer here.
    fn run_ui_command(
        &mut self,
        command: UiCommand,
        argument: String,
        alias: &'static str,
    ) -> Option<ChatIntent> {
        match command {
            // The screens. `Ctrl+P` silently does nothing until the first
            // `Settings` event has arrived; the command says why instead.
            UiCommand::Settings => {
                if self.settings_snapshot.is_none() {
                    return self.note("ui.cmd.settings_pending");
                }
                self.handle_ctrl_shortcut('p').flatten()
            }
            UiCommand::SelfModel => Some(ChatIntent::OpenSelfModel),
            // Deliberately not `Esc`'s behaviour: `Esc` cancels a running
            // generation first, and this command is the half that always means
            // "the chat list" (`/stop` is the other half).
            UiCommand::Chats => Some(ChatIntent::OpenChatList),
            UiCommand::Help => {
                self.help = Some(HelpState::open(self.help_last_tab));
                None
            }
            // The conversation.
            UiCommand::NewChat if argument.is_empty() => self.request_new_chat(),
            UiCommand::NewChat => self.new_chat_by_name(&argument),
            UiCommand::Rename => self.rename_active_chat(argument),
            UiCommand::Clone => self.active_chat.map_or_else(
                || self.note("ui.cmd.no_chat"),
                |id| Some(ChatIntent::CloneChat(id)),
            ),
            UiCommand::Copy => self.active_chat.map_or_else(
                || self.note("ui.cmd.no_chat"),
                |id| Some(ChatIntent::CopyChat(id)),
            ),
            // Both are ignored during generation by `trigger_destructive`, and
            // both honour `interface.confirm_destructive_keys` — the popup is
            // the key's, reached through the key's own path.
            UiCommand::Regen | UiCommand::Takeback => {
                if self.generating {
                    return self.busy_note(alias);
                }
                self.handle_ctrl_shortcut(match command {
                    UiCommand::Regen => 'r',
                    _ => 'e',
                })
                .flatten()
            }
            UiCommand::Impersonate => {
                if self.generating {
                    return self.busy_note(alias);
                }
                // The seed is the rest of the line, where the key takes whatever
                // was already in the box — the command consumed the box itself.
                Some(ChatIntent::Impersonate { seed: argument })
            }
            UiCommand::Stop => {
                if self.generating {
                    Some(ChatIntent::Cancel)
                } else {
                    self.note("ui.cmd.not_generating")
                }
            }
            // Finding things.
            UiCommand::Find => {
                if !argument.is_empty() {
                    // `open_feed_search` resumes the chat's last query; seeding
                    // it here is what makes `/find <text>` land on a match
                    // without a second keystroke.
                    self.search_last = argument;
                }
                self.open_feed_search();
                None
            }
            UiCommand::Search => Some(ChatIntent::SearchMessages { query: argument }),
            UiCommand::Links => {
                // Says what a reference looks like when the chat holds none.
                self.open_chat_links();
                None
            }
            // The feed.
            UiCommand::Thoughts => self.handle_ctrl_shortcut('t').flatten(),
            UiCommand::ToolCalls => self.handle_ctrl_shortcut('o').flatten(),
            UiCommand::Mouse => self.handle_ctrl_shortcut('w').flatten(),
            UiCommand::Emoji => self.handle_ctrl_shortcut('b').flatten(),
        }
    }

    /// Renames the open chat. With a title — straight through, as `F2` in the
    /// chat list does. Bare, it hands the **current title back as an editable
    /// command line** rather than opening a popup of its own: the box is right
    /// there and already empty (a bare command is all it holds), so the user
    /// edits the title and presses `Enter`. That also teaches the syntax by
    /// example, which a popup would not.
    fn rename_active_chat(&mut self, title: String) -> Option<ChatIntent> {
        // Not `let id = self.active_chat?` — that swallows the refusal, which is
        // the very silence this track exists to remove (a test caught it here).
        let Some(id) = self.active_chat else {
            return self.note("ui.cmd.no_chat");
        };
        if title.is_empty() {
            self.input.set_text(&format!("/rename {}", self.title));
            self.mark_input_changed();
            return None;
        }
        // The same ceiling the chat list's rename field enforces.
        let title: String = title.chars().take(rename_chat::MAX_TITLE_LEN).collect();
        Some(ChatIntent::RenameChat { id, title })
    }

    /// `/new <profile>`: an exact name first, then an unambiguous prefix
    /// (fork F3). A miss and an ambiguity both answer by naming the candidates
    /// **and** the bare-`/new` route, so neither is a dead end.
    fn new_chat_by_name(&mut self, wanted: &str) -> Option<ChatIntent> {
        let wanted = wanted.to_lowercase();
        let matching = |exact: bool| -> Vec<(Uuid, String)> {
            self.profiles
                .iter()
                .filter(|p| {
                    let name = p.name.to_lowercase();
                    if exact {
                        name == wanted
                    } else {
                        name.starts_with(&wanted)
                    }
                })
                .map(|p| (p.id, p.name.clone()))
                .collect()
        };
        let mut hits = matching(true);
        if hits.is_empty() {
            hits = matching(false);
        }
        if let [(id, _)] = hits.as_slice() {
            return Some(ChatIntent::NewChat {
                profile_id: Some(*id),
            });
        }
        // Nothing matched → list what there is; several matched → list those.
        let (key, names) = if hits.is_empty() {
            ("ui.cmd.no_profile", self.profile_names())
        } else {
            (
                "ui.cmd.many_profiles",
                hits.iter()
                    .map(|(_, name)| name.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
            )
        };
        let msg = self.loc.tf(key, &[("name", &wanted), ("names", &names)]);
        self.push_note(&msg);
        None
    }

    /// The profile names as one list for a note.
    fn profile_names(&self) -> String {
        self.profiles
            .iter()
            .map(|p| p.name.clone())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// A localized note with no arguments, and no intent — the shape most
    /// refusals take here.
    fn note(&mut self, key: &'static str) -> Option<ChatIntent> {
        self.push_note(self.loc.t(key));
        None
    }

    /// "Not while a reply is being generated" — naming the command that was
    /// typed and the two ways to stop the turn, so the refusal ends with a route.
    fn busy_note(&mut self, alias: &'static str) -> Option<ChatIntent> {
        let msg = self.loc.tf("ui.cmd.generating", &[("cmd", alias)]);
        self.push_note(&msg);
        None
    }
}

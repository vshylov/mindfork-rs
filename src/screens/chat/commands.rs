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
//! See [docs/history/command-only-control.md](../../../docs/history/command-only-control.md)
//! §4.3 and spec §11.7.

use super::*;
use crate::features::profile_command::TextEdit;
use crate::features::profiles::ProfileEdit;
use crate::features::ui_command::{self, UiCommand};

/// Which of the assistant profile's two free-text fields a `/profile` text
/// subcommand edits. Both fields are copied into a chat at creation, so both
/// edits apply to new conversations — the notes say so.
#[derive(Clone, Copy, PartialEq)]
enum ProfileText {
    System,
    Greeting,
}

/// Case-insensitive exact match first, then an unambiguous prefix, over
/// `(id, name)` pairs — the one matching rule every command that names a
/// profile or a persona uses (`/new`, `/profile delete`,
/// `/impersonation delete|use`). `Err` carries the candidates that matched
/// (empty — nothing did), for the caller's message.
fn resolve_named(items: &[(Uuid, String)], wanted: &str) -> Result<(Uuid, String), Vec<String>> {
    let wanted = wanted.to_lowercase();
    let matching = |exact: bool| -> Vec<&(Uuid, String)> {
        items
            .iter()
            .filter(|(_, name)| {
                let name = name.to_lowercase();
                if exact {
                    name == wanted
                } else {
                    name.starts_with(&wanted)
                }
            })
            .collect()
    };
    let mut hits = matching(true);
    if hits.is_empty() {
        hits = matching(false);
    }
    if let [hit] = hits.as_slice() {
        return Ok((*hit).clone());
    }
    Err(hits.into_iter().map(|(_, name)| name.clone()).collect())
}

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
    ///
    /// One line per command: an arm that needs a precondition checked delegates
    /// to a named method below rather than spelling the check out here. That
    /// keeps this readable as the dispatch *table* it is — and keeps its
    /// cognitive complexity off the analyzer's bar, which a table of twenty
    /// arms reaches on the strength of a few `if`s alone.
    fn run_ui_command(
        &mut self,
        command: UiCommand,
        argument: String,
        alias: &'static str,
    ) -> Option<ChatIntent> {
        match command {
            // The screens.
            UiCommand::Settings => self.open_settings_screen(),
            // Bare — the screen; `clear` — the wipe it offers behind `Ctrl+K`
            // twice, behind a confirmation here for the same reason.
            UiCommand::SelfModel if argument == "clear" => self.ask_clear_self_model(),
            UiCommand::SelfModel => Some(ChatIntent::OpenSelfModel),
            // Deliberately not `Esc`'s behaviour: `Esc` cancels a running
            // generation first, and this command is the half that always means
            // "the chat list" (`/stop` is the other half).
            UiCommand::Chats => Some(ChatIntent::OpenChatList),
            UiCommand::Changes => Some(ChatIntent::OpenChanges),
            // The runtime owns the overlay; the typed route reports the intent
            // it opens on, and is the chat's only route of its own — `?` was
            // dropped, `F1` never reaches the screen (spec §11.7).
            UiCommand::Help => Some(ChatIntent::OpenHelp),
            // The conversation.
            UiCommand::NewChat if argument.is_empty() => self.request_new_chat(),
            UiCommand::NewChat => self.new_chat_by_name(&argument),
            UiCommand::Rename => self.rename_active_chat(argument),
            UiCommand::AutoTitle => self.typed_autotitle(),
            UiCommand::Clone => self.active_chat.map_or_else(
                || self.note("ui.cmd.no_chat"),
                |id| Some(ChatIntent::CloneChat(id)),
            ),
            UiCommand::Copy => self.active_chat.map_or_else(
                || self.note("ui.cmd.no_chat"),
                |id| Some(ChatIntent::CopyChat(id)),
            ),
            UiCommand::Regen | UiCommand::Takeback => self.typed_destructive(command, alias),
            UiCommand::Impersonate => self.typed_impersonate(argument, alias),
            UiCommand::Stop => self.typed_stop(),
            // Finding things.
            UiCommand::Find => self.typed_find(argument),
            UiCommand::Search => Some(ChatIntent::SearchMessages { query: argument }),
            UiCommand::Links => {
                // Says what a reference looks like when the chat holds none.
                self.open_chat_links();
                None
            }
            // The feed.
            UiCommand::Thoughts => self.handle_ctrl_shortcut('t').flatten(),
            UiCommand::ToolCalls => self.handle_ctrl_shortcut('o').flatten(),
            UiCommand::Subagents => self.typed_subagents(argument),
            UiCommand::Mouse => self.handle_ctrl_shortcut('w').flatten(),
            UiCommand::Emoji => self.handle_ctrl_shortcut('b').flatten(),
        }
    }

    /// `/settings` — `Ctrl+P` silently does nothing until the first `Settings`
    /// event has arrived; the command says why instead.
    fn open_settings_screen(&mut self) -> Option<ChatIntent> {
        if self.settings_snapshot.is_none() {
            return self.note("ui.cmd.settings_pending");
        }
        self.handle_ctrl_shortcut('p').flatten()
    }

    /// `/self clear` — the model belongs to the *active profile*, so with no
    /// chat open the orchestrator would silently drop the edit: say so instead.
    fn ask_clear_self_model(&mut self) -> Option<ChatIntent> {
        if self.active_chat.is_none() {
            return self.note("ui.cmd.no_chat");
        }
        self.confirm = Some(ConfirmAction::ClearSelfModel);
        None
    }

    /// `/subagents [expand|collapse]` — the chat list's `Ctrl+O` for the open
    /// chat: folds or unfolds its sub-agent transcripts in the list
    /// (spec §11.2); bare it toggles like the key, the two words set the state
    /// outright. The effect lives on another screen, so the note says which
    /// way it went and where. From an open transcript the parent's list is
    /// what folds — the state is the parent's, as everywhere. With no
    /// transcripts (or before the list snapshot has arrived) the command
    /// explains instead: the rows appear once the assistant calls a sub-agent.
    fn typed_subagents(&mut self, argument: String) -> Option<ChatIntent> {
        let Some(active) = self.active_chat else {
            return self.note("ui.cmd.no_chat");
        };
        // The parser normalized the word to the registry's spelling; anything
        // else it reported itself, so bare is the only other shape here.
        let want = match argument.as_str() {
            "expand" => Some(true),
            "collapse" => Some(false),
            _ => None,
        };
        let target = self
            .chats
            .iter()
            .find(|c| c.id == active || c.child_ids().any(|k| k == active))
            .filter(|c| !c.children.is_empty())
            .map(|c| (c.id, want.unwrap_or(!c.children_expanded)));
        let Some((id, expanded)) = target else {
            return self.note("ui.cmd.subagents_none");
        };
        self.push_note(self.loc.t(if expanded {
            "ui.cmd.subagents_expanded"
        } else {
            "ui.cmd.subagents_collapsed"
        }));
        Some(ChatIntent::SetChildrenExpanded { id, expanded })
    }

    /// `/regen`·`/retry` and `/takeback`. Both are ignored during generation by
    /// `trigger_destructive`, and both honour `confirm_destructive_keys` — the
    /// popup is the key's, reached through the key's own path. The only thing
    /// added here is the answer the key does not owe.
    fn typed_destructive(&mut self, command: UiCommand, alias: &'static str) -> Option<ChatIntent> {
        if self.generating {
            return self.busy_note(alias);
        }
        let chord = if command == UiCommand::Regen {
            'r'
        } else {
            'e'
        };
        self.handle_ctrl_shortcut(chord).flatten()
    }

    /// `/impersonate [text]` — the seed is the rest of the line, where the key
    /// takes whatever was already in the box (the command consumed the box).
    fn typed_impersonate(&mut self, seed: String, alias: &'static str) -> Option<ChatIntent> {
        if self.generating {
            return self.busy_note(alias);
        }
        Some(ChatIntent::Impersonate { seed })
    }

    /// `/stop` — the explicit half of `Esc`, and the half that has something to
    /// say when there is nothing to cancel.
    fn typed_stop(&mut self) -> Option<ChatIntent> {
        if self.generating {
            return Some(ChatIntent::Cancel);
        }
        self.note("ui.cmd.not_generating")
    }

    /// `/find [text]` — `open_feed_search` resumes the chat's last query, so
    /// seeding it here is what makes `/find <text>` land on a match without a
    /// second keystroke.
    fn typed_find(&mut self, query: String) -> Option<ChatIntent> {
        if !query.is_empty() {
            self.search_last = query;
        }
        self.open_feed_search();
        None
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
        let title: String = title
            .chars()
            .take(crate::shared::title::MAX_TITLE_LEN)
            .collect();
        Some(ChatIntent::RenameChat { id, title })
    }

    /// `/autotitle` — the chat list's `Ctrl+R` for the open conversation: ask
    /// the model to title it (spec §11.2). The key is browser-taken (`Ctrl+R`
    /// reloads the tab), which is what earned this action a typed route. The
    /// task takes seconds on a local model, so the command answers right away;
    /// the result is the visible rename, and a failure falls back to a feed
    /// note (`ChatListError` routing in `runtime::dispatch`).
    fn typed_autotitle(&mut self) -> Option<ChatIntent> {
        let Some(id) = self.active_chat else {
            return self.note("ui.cmd.no_chat");
        };
        self.push_note(self.loc.t("ui.cmd.autotitle_started"));
        Some(ChatIntent::AutoTitleChat(id))
    }

    /// `/new <profile>`: the shared name resolver, then the same intent
    /// `Ctrl+N` produces once a profile is chosen.
    fn new_chat_by_name(&mut self, wanted: &str) -> Option<ChatIntent> {
        let (id, _) = self.resolve_profile(wanted, "ui.cmd.route_new")?;
        Some(ChatIntent::NewChat {
            profile_id: Some(id),
        })
    }

    /// Resolves a profile by name: an exact match first (case-insensitively),
    /// then an unambiguous prefix (fork F3). On a miss or an ambiguity it leaves
    /// a note naming the candidates and the `route` that shows them, and returns
    /// `None`.
    ///
    /// One resolver for every command that names a profile (`/new <profile>`,
    /// `/profile delete <name>`) — the family's *contract* is shared, so its
    /// wording and its prefix rule are shared too, rather than being written
    /// twice and drifting (docs/lessons.md §2). `route` is a whole bundle key,
    /// never a built one: the callers want different next steps. The matching
    /// itself is [`resolve_named`], which `/impersonation` reuses over the
    /// personas.
    fn resolve_profile(&mut self, wanted: &str, route: &'static str) -> Option<(Uuid, String)> {
        let items: Vec<(Uuid, String)> = self
            .profiles
            .iter()
            .map(|p| (p.id, p.name.clone()))
            .collect();
        match resolve_named(&items, wanted) {
            Ok(hit) => Some(hit),
            Err(hits) => {
                let all = items.into_iter().map(|(_, name)| name).collect();
                self.report_unresolved(
                    wanted,
                    hits,
                    all,
                    ("ui.cmd.no_profile", "ui.cmd.many_profiles"),
                    route,
                );
                None
            }
        }
    }

    /// The note for a name [`resolve_named`] could not settle: nothing matched
    /// (`keys.0`, listing everything there is) or several did (`keys.1`,
    /// listing the contenders). Both name the `route` that shows the full list.
    fn report_unresolved(
        &mut self,
        wanted: &str,
        hits: Vec<String>,
        all: Vec<String>,
        keys: (&'static str, &'static str),
        route: &'static str,
    ) {
        let (key, names) = if hits.is_empty() {
            (keys.0, all.join(", "))
        } else {
            (keys.1, hits.join(", "))
        };
        let msg = self.loc.tf(
            key,
            &[
                ("name", &wanted.to_lowercase()),
                ("names", &names),
                ("route", self.loc.t(route)),
            ],
        );
        self.push_note(&msg);
    }

    /// The profile commands (`/profile list|new|delete`) — the settings screen's
    /// `Ctrl+N`/`Ctrl+D`, reachable from a host that keeps those keys. Stage 2 of
    /// docs/history/command-only-control.md (fork F5). Returns `None` when the
    /// text is not a `/profile` command.
    pub(super) fn try_profile_command(&mut self, text: &str) -> Option<Option<ChatIntent>> {
        use crate::features::profile_command::{self, ProfileCommand};
        let parsed = profile_command::parse(text, self.loc)?;
        self.input.clear();
        self.mark_input_changed();
        Some(match parsed {
            Ok(ProfileCommand::List) => {
                self.list_profiles();
                None
            }
            // The same default name the settings screen's `Ctrl+N` uses, so the
            // two routes create the same thing. The persona stays empty either
            // way — it is written in the settings screen afterwards, which is
            // what the orchestrator's notice says.
            Ok(ProfileCommand::New { name }) => Some(ChatIntent::CreateProfile {
                name: name
                    .unwrap_or_else(|| self.loc.t("ui.settings.new_profile_name").to_string()),
            }),
            Ok(ProfileCommand::Delete { name }) => self.confirm_delete_profile(&name),
            // Stage 3 (docs/history/commands-stage3.md §3.2): the profile's two text
            // fields, editable without the settings screen.
            Ok(ProfileCommand::System(edit)) => {
                self.profile_text_command(ProfileText::System, edit)
            }
            Ok(ProfileCommand::Greeting(edit)) => {
                self.profile_text_command(ProfileText::Greeting, edit)
            }
            Err(msg) => {
                self.push_note(&msg);
                None
            }
        })
    }

    /// `/impersonation …` — the impersonation profiles (the user personas,
    /// spec §11.8), plus the active profile's link to one. Stage 3 of
    /// docs/history/command-only-control.md (docs/history/commands-stage3.md §3.3):
    /// their CRUD lives behind the settings screen's browser-taken
    /// `Ctrl+N`/`Ctrl+D`, and the persona's text behind its editor. Returns
    /// `None` when the text is not an `/impersonation` command.
    pub(super) fn try_impersonation_command(&mut self, text: &str) -> Option<Option<ChatIntent>> {
        let parsed = crate::features::impersonation_command::parse(text, self.loc)?;
        self.input.clear();
        self.mark_input_changed();
        Some(match parsed {
            Ok(cmd) => self.run_impersonation_command(cmd),
            Err(msg) => {
                self.push_note(&msg);
                None
            }
        })
    }

    /// Dispatches a parsed `/impersonation` command. Every arm works over the
    /// settings snapshot (the personas live in the config), so its absence is
    /// answered once here — the `/settings` rule.
    fn run_impersonation_command(
        &mut self,
        cmd: crate::features::impersonation_command::ImpersonationCommand,
    ) -> Option<ChatIntent> {
        use crate::features::impersonation_command::ImpersonationCommand as Cmd;
        if self.settings_snapshot.is_none() {
            return self.note("ui.cmd.settings_pending");
        }
        match cmd {
            Cmd::List => {
                self.list_impersonations();
                None
            }
            Cmd::New { name } => self.create_impersonation(name),
            Cmd::Delete { name } => self.confirm_delete_impersonation(&name),
            Cmd::Use { name } => self.link_impersonation(&name),
            Cmd::UseDefault => self.unlink_impersonation(),
            Cmd::System(edit) => self.impersonation_text_command(edit),
        }
    }

    /// The export command (`/export [md|json] [path]`) — the conversation to a
    /// file, for when the clipboard cannot reach the user's machine at all
    /// (JupyterLab drops OSC 52, spec §11.7). The orchestrator writes it: it
    /// owns the conversation and the disk. Returns `None` when the text is not
    /// an `/export` command.
    pub(super) fn try_export_command(&mut self, text: &str) -> Option<Option<ChatIntent>> {
        let parsed = crate::features::export_command::parse(text, self.loc)?;
        self.input.clear();
        self.mark_input_changed();
        Some(match parsed {
            Ok(cmd) => match self.active_chat {
                Some(id) => Some(ChatIntent::ExportChat {
                    id,
                    format: cmd.format,
                    path: cmd.path,
                }),
                None => self.note("ui.cmd.no_chat"),
            },
            Err(msg) => {
                self.push_note(&msg);
                None
            }
        })
    }

    /// `/profile list` — the names, with the open chat's own profile marked.
    /// The screen already holds this snapshot, so it answers locally.
    fn list_profiles(&mut self) {
        if self.profiles.is_empty() {
            self.note("ui.profile.none");
            return;
        }
        let active = self
            .active_chat
            .and_then(|id| self.chats.iter().find(|c| c.id == id))
            .map(|c| c.profile_id);
        // The same marker the screens use for "this is the selected one"; it is
        // one column wide in both glyph sets, so the names stay aligned.
        let marker = self.palette.glyphs().title_marker;
        let names: Vec<String> = self
            .profiles
            .iter()
            .map(|p| {
                if Some(p.id) == active {
                    format!("{marker} {}", p.name)
                } else {
                    format!("  {}", p.name)
                }
            })
            .collect();
        let msg = self
            .loc
            .tf("ui.profile.list", &[("names", &names.join("\n"))]);
        self.push_note(&msg);
    }

    /// `/profile delete <name>` — resolve the name, then **always** ask.
    ///
    /// The key does not ask (`Ctrl+D` in the settings screen deletes the
    /// selected row outright), and this is the one place stage 2 departs from
    /// "a command is its key" — deliberately. The screen shows you the profile
    /// you are about to delete, and a typed prefix can resolve to one you did
    /// not picture; the popup is what puts the target, and the conversations
    /// going with it, back in front of you. Independent of
    /// `confirm_destructive_keys`, which is about the two chat-level keys.
    fn confirm_delete_profile(&mut self, wanted: &str) -> Option<ChatIntent> {
        let (id, name) = self.resolve_profile(wanted, "ui.cmd.route_profile_list")?;
        // The orchestrator refuses to delete the last profile (there would be
        // nothing to create chats from), so say so here instead of asking a
        // question whose "yes" is then declined.
        if self.profiles.len() <= 1 {
            return self.note("ui.profile.last");
        }
        let chats = self.chats.iter().filter(|c| c.profile_id == id).count();
        self.confirm = Some(ConfirmAction::DeleteProfile { id, name, chats });
        None
    }

    /// The active chat's full profile, cloned out of the settings snapshot —
    /// the working copy the text commands read and prefill from. `Err` is the
    /// note key that says which precondition failed: no chat open (a sub-agent
    /// transcript's id is not in the chat list, so it lands here too), or the
    /// snapshot not yet arrived / not yet carrying the profile.
    fn active_full_profile(&self) -> Result<Profile, &'static str> {
        let Some(chat_id) = self.active_chat else {
            return Err("ui.cmd.no_chat");
        };
        let Some(profile_id) = self
            .chats
            .iter()
            .find(|c| c.id == chat_id)
            .map(|c| c.profile_id)
        else {
            return Err("ui.cmd.no_chat");
        };
        let Some((_, profiles, ..)) = self.settings_snapshot.as_ref() else {
            return Err("ui.cmd.settings_pending");
        };
        profiles
            .iter()
            .find(|p| p.id == profile_id)
            .cloned()
            .ok_or("ui.cmd.settings_pending")
    }

    /// A working copy of the snapshot's config for a persona edit — the same
    /// clone-and-commit the settings screen's persona editors do.
    fn snapshot_config(&self) -> Option<AppConfig> {
        self.settings_snapshot
            .as_ref()
            .map(|(config, ..)| config.clone())
    }

    /// The personas as `(id, name)` pairs, for listing and resolving.
    fn personas(&self) -> Vec<(Uuid, String)> {
        self.settings_snapshot
            .as_ref()
            .map(|(config, ..)| {
                config
                    .impersonation_profiles
                    .iter()
                    .map(|p| (p.id, p.name.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `/profile system|greeting [text|clear]` on the active chat's profile.
    /// `Show` hands the current text back as an editable command line (the
    /// `/rename` pattern); `Set`/`Clear` commit through the same
    /// `UpdateProfile` the settings editors use, and the note states the
    /// scope honestly: both fields are copied into a chat at creation, so the
    /// edit reaches **new** conversations (spec §5.1, §10).
    fn profile_text_command(&mut self, field: ProfileText, edit: TextEdit) -> Option<ChatIntent> {
        let profile = match self.active_full_profile() {
            Ok(p) => p,
            Err(key) => return self.note(key),
        };
        if edit == TextEdit::Show {
            return self.show_profile_text(field, &profile);
        }
        let text = match edit {
            TextEdit::Set(t) => Some(t),
            _ => None,
        };
        let (profile_edit, key) = match field {
            ProfileText::System => (
                ProfileEdit {
                    system_message: Some(text.clone().unwrap_or_default()),
                    ..Default::default()
                },
                if text.is_some() {
                    "ui.profile.system_set"
                } else {
                    "ui.profile.system_cleared"
                },
            ),
            ProfileText::Greeting => (
                ProfileEdit {
                    greeting: Some(text.clone()),
                    ..Default::default()
                },
                if text.is_some() {
                    "ui.profile.greeting_set"
                } else {
                    "ui.profile.greeting_cleared"
                },
            ),
        };
        let msg = self.loc.tf(key, &[("name", &profile.name)]);
        self.push_note(&msg);
        Some(ChatIntent::UpdateProfile {
            id: profile.id,
            edit: Box::new(profile_edit),
        })
    }

    /// The bare (`Show`) half of a `/profile` text subcommand: prefill the
    /// current value for editing, or say there is nothing yet — and what sets
    /// one — when the field is empty (an empty prefill would teach nothing).
    fn show_profile_text(&mut self, field: ProfileText, profile: &Profile) -> Option<ChatIntent> {
        let (word, current, empty_key) = match field {
            ProfileText::System => (
                "system",
                profile.default_system_message.clone(),
                "ui.profile.system_empty",
            ),
            ProfileText::Greeting => (
                "greeting",
                profile.greeting.clone().unwrap_or_default(),
                "ui.profile.greeting_empty",
            ),
        };
        if current.is_empty() {
            let msg = self.loc.tf(empty_key, &[("name", &profile.name)]);
            self.push_note(&msg);
        } else {
            self.input.set_text(&format!("/profile {word} {current}"));
            self.mark_input_changed();
        }
        None
    }

    /// `/impersonation list` — the personas, with the active chat's profile's
    /// one marked (the `/profile list` shape).
    fn list_impersonations(&mut self) {
        let personas = self.personas();
        if personas.is_empty() {
            self.note("ui.imp.none");
            return;
        }
        let used = self
            .active_full_profile()
            .ok()
            .and_then(|p| p.impersonation_profile_id);
        let marker = self.palette.glyphs().title_marker;
        let names: Vec<String> = personas
            .iter()
            .map(|(id, name)| {
                if Some(*id) == used {
                    format!("{marker} {name}")
                } else {
                    format!("  {name}")
                }
            })
            .collect();
        let msg = self.loc.tf("ui.imp.list", &[("names", &names.join("\n"))]);
        self.push_note(&msg);
    }

    /// `/impersonation new [name]` — create a persona with an empty system
    /// message (the settings screen's `Ctrl+N`, same default name). Creating
    /// does not link it (fork F4): the note names the two next steps instead.
    fn create_impersonation(&mut self, name: Option<String>) -> Option<ChatIntent> {
        let Some(mut config) = self.snapshot_config() else {
            return self.note("ui.cmd.settings_pending");
        };
        let name = name.unwrap_or_else(|| self.loc.t("ui.settings.new_profile_name").to_string());
        config
            .impersonation_profiles
            .push(crate::shared::config::ImpersonationProfile::new(
                name.clone(),
                String::new(),
            ));
        let msg = self.loc.tf("ui.imp.created", &[("name", &name)]);
        self.push_note(&msg);
        Some(ChatIntent::UpdateConfig(Box::new(config)))
    }

    /// `/impersonation delete <name>` — resolve the name, then **always** ask
    /// (the `/profile delete` rule: a typed prefix can resolve to a persona the
    /// user did not picture, and its system message is unrecoverable).
    fn confirm_delete_impersonation(&mut self, wanted: &str) -> Option<ChatIntent> {
        let (id, name) = self.resolve_impersonation(wanted)?;
        self.confirm = Some(ConfirmAction::DeleteImpersonation { id, name });
        None
    }

    /// The confirmed half of `/impersonation delete`: build the config without
    /// the persona from the **current** snapshot (it may have refreshed while
    /// the popup was open). Referencing profiles are left alone — a dangling
    /// id reads as "not set" (spec §11.8), which the popup's question said.
    pub(super) fn delete_impersonation(&mut self, id: Uuid, name: &str) -> Option<ChatIntent> {
        let Some(mut config) = self.snapshot_config() else {
            return self.note("ui.cmd.settings_pending");
        };
        let before = config.impersonation_profiles.len();
        config.impersonation_profiles.retain(|p| p.id != id);
        if config.impersonation_profiles.len() == before {
            // Gone between the question and the answer — nothing to delete.
            return self.note("ui.imp.none");
        }
        let msg = self.loc.tf("ui.imp.deleted", &[("name", name)]);
        self.push_note(&msg);
        Some(ChatIntent::UpdateConfig(Box::new(config)))
    }

    /// `/impersonation use <name>` — link the persona to the active chat's
    /// profile, through the same `UpdateProfile` the settings field commits.
    /// Applies live: the next impersonation resolves the link at `Ctrl+U` time.
    fn link_impersonation(&mut self, wanted: &str) -> Option<ChatIntent> {
        let profile = match self.active_full_profile() {
            Ok(p) => p,
            Err(key) => return self.note(key),
        };
        let (persona_id, persona_name) = self.resolve_impersonation(wanted)?;
        let msg = self.loc.tf(
            "ui.imp.linked",
            &[("profile", &profile.name), ("name", &persona_name)],
        );
        self.push_note(&msg);
        Some(ChatIntent::UpdateProfile {
            id: profile.id,
            edit: Box::new(ProfileEdit {
                impersonation_profile_id: Some(Some(persona_id)),
                ..Default::default()
            }),
        })
    }

    /// `/impersonation use default` — unlink: impersonation falls back to the
    /// shared default text (the settings choice's "not set" option).
    fn unlink_impersonation(&mut self) -> Option<ChatIntent> {
        let profile = match self.active_full_profile() {
            Ok(p) => p,
            Err(key) => return self.note(key),
        };
        let msg = self
            .loc
            .tf("ui.imp.unlinked", &[("profile", &profile.name)]);
        self.push_note(&msg);
        Some(ChatIntent::UpdateProfile {
            id: profile.id,
            edit: Box::new(ProfileEdit {
                impersonation_profile_id: Some(None),
                ..Default::default()
            }),
        })
    }

    /// `/impersonation system [text|clear]` — the system message of the persona
    /// the active profile is linked to, resolved the way impersonation itself
    /// resolves it. No link (or a dangling one) answers with the two routes
    /// that create one; an edit applies to the **next** impersonation
    /// everywhere, unlike the profile texts, and the notes reflect that.
    fn impersonation_text_command(&mut self, edit: TextEdit) -> Option<ChatIntent> {
        let profile = match self.active_full_profile() {
            Ok(p) => p,
            Err(key) => return self.note(key),
        };
        let persona = profile.impersonation_profile_id.and_then(|id| {
            self.settings_snapshot.as_ref().and_then(|(config, ..)| {
                config
                    .impersonation_profiles
                    .iter()
                    .find(|p| p.id == id)
                    .cloned()
            })
        });
        let Some(persona) = persona else {
            let msg = self
                .loc
                .tf("ui.imp.not_linked", &[("profile", &profile.name)]);
            self.push_note(&msg);
            return None;
        };
        match edit {
            TextEdit::Show => {
                if persona.system_message.is_empty() {
                    let msg = self
                        .loc
                        .tf("ui.imp.system_empty", &[("name", &persona.name)]);
                    self.push_note(&msg);
                } else {
                    self.input
                        .set_text(&format!("/impersonation system {}", persona.system_message));
                    self.mark_input_changed();
                }
                None
            }
            TextEdit::Clear | TextEdit::Set(_) => {
                let text = match edit {
                    TextEdit::Set(t) => t,
                    _ => String::new(),
                };
                let key = if text.is_empty() {
                    "ui.imp.system_cleared"
                } else {
                    "ui.imp.system_set"
                };
                let Some(mut config) = self.snapshot_config() else {
                    return self.note("ui.cmd.settings_pending");
                };
                let Some(p) = config
                    .impersonation_profiles
                    .iter_mut()
                    .find(|p| p.id == persona.id)
                else {
                    return self.note("ui.cmd.settings_pending");
                };
                p.system_message = text;
                let msg = self.loc.tf(key, &[("name", &persona.name)]);
                self.push_note(&msg);
                Some(ChatIntent::UpdateConfig(Box::new(config)))
            }
        }
    }

    /// Resolves a persona by name — [`resolve_named`], the profile resolver's
    /// rule, over the snapshot's personas. An empty list answers with the
    /// route that creates one instead of an empty "there is:" enumeration.
    fn resolve_impersonation(&mut self, wanted: &str) -> Option<(Uuid, String)> {
        let personas = self.personas();
        if personas.is_empty() {
            self.note("ui.imp.none");
            return None;
        }
        match resolve_named(&personas, wanted) {
            Ok(hit) => Some(hit),
            Err(hits) => {
                let all = personas.into_iter().map(|(_, name)| name).collect();
                self.report_unresolved(
                    wanted,
                    hits,
                    all,
                    ("ui.imp.no_persona", "ui.imp.many_personas"),
                    "ui.cmd.route_imp_list",
                );
                None
            }
        }
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

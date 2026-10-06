//! The TUI render loop and the bridge to the orchestrator. See spec §4.4.1, §11.
//!
//! The loop is synchronous (on the main thread): it polls input with a timeout,
//! non-blockingly drains orchestrator events, and repaints [`ChatScreen`].
//! `app` is the only one that knows both sides of the contract: incoming [`AppEvent`]
//! is applied to the screen by mutators, outgoing [`ChatIntent`] is translated into
//! [`AppCommand`]. The screen itself knows nothing about `app`/channels (FSD,
//! dependencies point downward).

use std::io::stdout;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use anyhow::{Result, bail};
use ratatui::backend::Backend;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers,
};
#[cfg(unix)]
use ratatui::crossterm::event::{
    EnableBracketedPaste, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
#[cfg(unix)]
use ratatui::crossterm::terminal::supports_keyboard_enhancement;
use ratatui::crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::{DefaultTerminal, Terminal};
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

use crate::app::events::{AppCommand, AppEvent, BackgroundKind, ClipboardImage, TaskList};
use crate::features::spellcheck::{SpellChecker, dict};
use crate::screens::changes::{ChangesIntent, ChangesScreen};
use crate::screens::chat::{ChatIntent, ChatScreen};
use crate::screens::chat_list::{ChatListIntent, ChatListScreen};
use crate::screens::search::{SearchIntent, SearchScreen};
use crate::screens::self_model::{SelfModelIntent, SelfModelScreen};
use crate::screens::settings::{SettingsIntent, SettingsScreen};
use crate::screens::tasks::{TasksIntent, TasksScreen};
use crate::shared::theme::Palette;
use crate::shared::ui::{self, MinSize, SPINNER_STEP, WayOut, dim_background};
use crate::widgets::help_dialog::{
    self, DEFAULT_HELP_TAB, HelpContext, HelpKeyOutcome, HelpSection, HelpState, HelpTab,
};
use crate::widgets::status_bar::EscTarget;

/// The screen open on top of the chat. `ChatScreen` always exists as the base (the
/// feed, generation, the input box); the chat list (`Esc`) or settings (`Ctrl+P`) can
/// be open on top of it. See the UI architecture (architecture.md §9): three screens.
enum ActiveScreen {
    /// Chat only — no overlaid screen.
    Chat,
    /// A fullscreen chat list. Boxed — screens are large, keeping them inline in the
    /// enum variant would bloat every value (clippy::large_enum_variant).
    ChatList(Box<ChatListScreen>),
    /// The settings screen.
    Settings(Box<SettingsScreen>),
    /// The "self-model" viewer screen (read-only, `F3`).
    SelfModel(Box<SelfModelScreen>),
    /// The message-level search results (`Ctrl+G` from the chat list's content
    /// mode). See docs/history/chat-search-stage2.md.
    Search(Box<SearchScreen>),
    /// What the assistant changed in the attached project (`F4` / `/changes`).
    /// See docs/history/code-workspace.md §3.5, spec §9.12.
    Changes(Box<ChangesScreen>),
    /// Everything the app is doing in the background (`F7` / `/tasks`):
    /// every sub-agent and dialogue run, and the silent tasks. See spec §11.10,
    /// docs/research/tasks-screen.md.
    Tasks(Box<TasksScreen>),
}

impl ActiveScreen {
    /// Is the chat itself in front (not the list/settings)? Gates work that only
    /// applies to the chat: the spellcheck recheck, spinner animation, wheel
    /// scrolling.
    fn is_chat(&self) -> bool {
        matches!(self, ActiveScreen::Chat)
    }

    /// Updates the palette and interface locale of an open overlay screen (chat list /
    /// self-model) on a theme/compatibility-mode/UI-language change. The settings
    /// screen is updated separately (`refresh` is broader than the palette), the chat
    /// — via its own base (`ChatScreen::set_settings`). The canonical place
    /// enumerating screens for the theme broadcast (palette + locale). See
    /// docs/i18n-ui.md §3.3.
    fn set_theme(&mut self, palette: Palette, loc: &'static crate::shared::i18n::Locale) {
        match self {
            ActiveScreen::ChatList(list) => {
                list.set_palette(palette);
                list.set_loc(loc);
            }
            ActiveScreen::SelfModel(view) => {
                view.set_palette(palette);
                view.set_loc(loc);
            }
            ActiveScreen::Search(search) => {
                search.set_palette(palette);
                search.set_loc(loc);
            }
            ActiveScreen::Changes(changes) => {
                changes.set_palette(palette);
                changes.set_loc(loc);
            }
            ActiveScreen::Tasks(tasks) => {
                tasks.set_palette(palette);
                tasks.set_loc(loc);
            }
            ActiveScreen::Chat | ActiveScreen::Settings(_) => {}
        }
    }

    /// Routes a clipboard paste to the target screen: settings/list/self-model —
    /// into their own fields; the base chat — into the input box. The canonical place
    /// routing pastes across screens. `chat` — the base screen (needed for the `Chat`
    /// variant).
    fn handle_paste(&mut self, chat: &mut ChatScreen, text: &str) {
        match self {
            ActiveScreen::Settings(settings) => settings.handle_paste(text),
            ActiveScreen::Chat => chat.handle_paste(text),
            ActiveScreen::ChatList(list) => list.handle_paste(text),
            ActiveScreen::SelfModel(view) => view.handle_paste(text),
            // Read-only results — nothing to paste into.
            ActiveScreen::Search(_) | ActiveScreen::Changes(_) | ActiveScreen::Tasks(_) => {}
        }
    }
}

/// A one-deep back-stack: where `Esc` in the chat goes when that chat was
/// reached by drilling **down** rather than by picking it. Without it the step
/// the user just took is thrown away and they land in the chat list.
///
/// Three ways down exist, and they differ only in what "back" *is*:
///
/// - a **message-level search hit** ([`SearchIntent::OpenHit`]) — back is the
///   results screen;
/// - a **`chat://` reference** followed in the feed (spec §11.3) — back is the
///   conversation it was followed from;
/// - a **run's transcript or parent chat opened from the tasks screen**
///   ([`TasksIntent::OpenRun`], [`TasksIntent::OpenParent`], spec §11.10) —
///   back is the task list.
///
/// **One deep, and consumed on use**, for both: the next `Esc` goes on to the
/// chat list as it always did. So a chain of followed references (A → B → C)
/// steps back to B and no further — deliberate, and the same shape the search
/// half has always had. A true stack would have to answer what an ordinary chat
/// switch does to the *middle* of it, which is a question nothing has asked yet
/// (docs/roadmap.md).
///
/// It is deliberately session state — a local of [`run_loop`], never persisted.
///
/// The chat screen knows nothing about any of this (FSD: `screens` may not depend
/// on `app`). [`ChatIntent::OpenChatList`] already means "go back" from its point
/// of view; what that resolves to is decided in [`dispatch`].
enum Back {
    /// Back to the hits the chat was opened from. The **screen itself** is
    /// stashed, not the query: re-running the search would lose the selection
    /// and the scroll position, and working through a list of hits one by one is
    /// exactly what going back is for.
    Search {
        /// Boxed as it is inside [`ActiveScreen`] — the screen is large
        /// (clippy::large_enum_variant).
        screen: Box<SearchScreen>,
        chat: Uuid,
    },
    /// Back to the conversation a `chat://` reference was followed from. Only
    /// the id is kept: a chat is reopened from storage in full, so there is no
    /// screen state a re-activation would lose.
    Link { origin: Uuid, chat: Uuid },
    /// Back to the tasks screen a run was opened from — the screen itself,
    /// like the search half, so the selection survives; its rows are
    /// re-requested on the way back, since they may have moved meanwhile.
    Tasks {
        /// Boxed as it is inside [`ActiveScreen`] (clippy::large_enum_variant).
        screen: Box<TasksScreen>,
        chat: Uuid,
    },
}

impl Back {
    /// The chat this way back leads *out of* — the one the jump opened.
    ///
    /// Activating a **different** chat means the user left by an ordinary route
    /// (picking one in the list, `Ctrl+N`, a clone…), at which point the stash
    /// is no longer where they came from — see the `ChatActivated` arm of
    /// [`apply_event`], the single funnel every one of those routes ends in.
    /// Re-activating the *same* chat (regeneration, deleting an exchange, a
    /// repeat jump) is not leaving it, so it keeps the way back.
    fn chat(&self) -> Uuid {
        match self {
            Back::Search { chat, .. } | Back::Link { chat, .. } | Back::Tasks { chat, .. } => *chat,
        }
    }
}

/// Where `Esc` currently goes, for the status bar's hint — **derived** from the
/// back-stack, never mirrored into a second flag.
///
/// Setting a flag at each place the stash changes would mean writing the same
/// rule twice (and the clearing three times, counting the `ChatActivated`
/// funnel), which is exactly how a hint drifts away from the key it describes.
/// Deriving it in the draw path instead makes "the bar says where `Esc` goes"
/// true of every frame by construction.
/// The help overlay above whatever screen is active (spec §11.7,
/// docs/history/help-hotkeys-context.md stage 2): the open dialog plus the tab
/// remembered between opens. Runtime-owned so `F1` means the same thing on
/// every screen — the screens keep no `F1` handler of their own.
struct HelpOverlay {
    /// The open dialog, drawn over the active screen ([`draw_frame`]).
    open: Option<HelpState>,
    /// The tab restored on the next chat-side open; a non-chat open forces
    /// "Shortcuts" anchored to its section instead (fork F2).
    last_tab: HelpTab,
}

impl HelpOverlay {
    fn new() -> Self {
        Self {
            open: None,
            last_tab: DEFAULT_HELP_TAB,
        }
    }

    /// Open for the given screen: the chat restores the remembered tab at the
    /// top (its section is right under the short "Everywhere" block); any
    /// other screen gets "Shortcuts" scrolled to its own section.
    fn open_for(&mut self, context: HelpContext) {
        self.open = Some(match context {
            HelpContext::Chat => HelpState::open(self.last_tab),
            other => HelpState::open_at(other),
        });
    }

    /// Close, remembering the tab for the next open.
    fn close(&mut self) {
        if let Some(state) = self.open.take() {
            self.last_tab = state.tab;
        }
    }
}

/// The "Shortcuts" tab's sections in display order: "Globally", then the
/// screens by how often the user is on them. Composed here — the app layer is
/// the one place that knows every screen exists — from tables owned by the
/// code they document (proximity to the `match` is the anti-drift force;
/// docs/history/help-hotkeys-context.md §6). `help_sections_cover_every_context`
/// closes the loop [`help_context`] opens: one section per screen.
pub(super) static HELP_SECTIONS: [&HelpSection; 8] = [
    &help_dialog::GLOBAL,
    &crate::screens::chat::HELP_SECTION,
    &crate::widgets::chat_list::HELP_SECTION,
    &crate::screens::settings::HELP_SECTION,
    &crate::screens::self_model::HELP_SECTION,
    &crate::screens::changes::HELP_SECTION,
    &crate::screens::tasks::HELP_SECTION,
    &crate::screens::search::HELP_SECTION,
];

/// The active screen's help context — the section the dialog marks "you are
/// here" and anchors to. An exhaustive match on purpose: a new screen cannot
/// join `ActiveScreen` without deciding its help section (the same rule the
/// enum's other match sites enforce, architecture.md §10).
fn help_context(active: &ActiveScreen) -> HelpContext {
    match active {
        ActiveScreen::Chat => HelpContext::Chat,
        ActiveScreen::ChatList(_) => HelpContext::ChatList,
        ActiveScreen::Settings(_) => HelpContext::Settings,
        ActiveScreen::SelfModel(_) => HelpContext::SelfModel,
        ActiveScreen::Search(_) => HelpContext::Search,
        ActiveScreen::Changes(_) => HelpContext::Changes,
        ActiveScreen::Tasks(_) => HelpContext::Tasks,
    }
}

fn esc_target(back: &Option<Back>) -> EscTarget {
    match back {
        Some(Back::Search { .. }) => EscTarget::SearchResults,
        Some(Back::Link { .. }) => EscTarget::PreviousChat,
        Some(Back::Tasks { .. }) => EscTarget::Tasks,
        None => EscTarget::ChatList,
    }
}

/// The input polling period (the repaint tick).
const TICK: Duration = Duration::from_millis(50);

// A spinner frame is drawn on the first tick after its step's boundary, so two
// of them can land as little as `SPINNER_STEP - TICK` apart — and that gap has
// to outlast Windows Terminal's 100 ms output-idle debounce, or the terminal
// never re-finds the links it underlines while a spinner runs (see the constant).
const _: () = assert!(
    SPINNER_STEP.as_millis() - TICK.as_millis() > 100,
    "a spinner frame must leave the terminal more than 100 ms of quiet"
);

/// Puts the terminal into the modes the app reads keys in, and records what it
/// turned out to deliver. Shared with `mindfork keys` (`app::key_echo`), so the
/// echo sees each key exactly as the app does. Returns whether the kitty
/// keyboard protocol was answered — `None` on Windows, where it is not asked.
///
/// On unix we enable bracketed paste: crossterm delivers a clipboard paste as ONE
/// `Event::Paste` event (whole, with line breaks as text, not Enter). Windows has no
/// such mode in crossterm (input is read via the Console API), there a paste arrives
/// as a batch of regular key events — we collect it in the loop
/// (`process_input_batch`), so there's nothing to enable here. See spec §11.5.
///
/// Here (unix) we also enable the kitty keyboard protocol at the "disambiguate"
/// level: the terminal's legacy encoding sends the same CR for `Shift+Enter` and
/// `Enter`, so a line break in the input box was unavailable on a "bare" unix
/// terminal. With `DISAMBIGUATE_ESCAPE_CODES` the terminal reports modifiers for
/// special keys (Enter/arrows/…), and `Shift+Enter` becomes distinguishable from
/// `Enter` (and `Shift`+arrows — from bare arrows, which enables keyboard-driven
/// selection). We push it only if the terminal supports the protocol (otherwise a
/// no-op); the caller pops it on exit (and the app's panic hook). Text input and a
/// lone `Shift`+character don't touch this flag (text arrives as is), so
/// layout-independent parsing of Ctrl shortcuts (`shared::keys`) and typing
/// `?`/emoji don't regress. Not needed on Windows — the Console API already reports
/// modifiers. `Alt+Enter` in the input box is a fallback line break for terminals
/// without this protocol (see spec §11.5, audit item 11).
pub(crate) fn enable_key_modes() -> Option<bool> {
    #[cfg(unix)]
    {
        let _ = execute!(stdout(), EnableBracketedPaste);
        let reported = supports_keyboard_enhancement().unwrap_or(false);
        if reported {
            let _ = execute!(
                stdout(),
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            );
        }
        // Whether the push happened decides which chord the input box's footer
        // advertises: `Shift+Enter` is only *deliverable* here if it did (spec
        // §11.5). Konsole is why the footer had to stop asserting it — its
        // default keytab answers `Shift+Return` with `\EOM`, which crossterm
        // drops on the floor, so the advertised key did visibly nothing.
        crate::shared::keys::set_modified_enter_reported(reported);
        Some(reported)
    }
    // Windows needs no protocol — the Console API reports modifiers itself, so
    // `Shift+Enter` is always the right thing to advertise there.
    #[cfg(windows)]
    {
        crate::shared::keys::set_modified_enter_reported(true);
        None
    }
}

/// Initializes the terminal, runs the loop, and restores the terminal on exit
/// (including on panic — `ratatui::init` sets a panic hook). `app` loads the
/// spellcheck dictionaries itself in the background per settings
/// (`dict_dir`/`personal`) and reloads them when they change.
pub fn run(
    cmd_tx: UnboundedSender<AppCommand>,
    evt_rx: UnboundedReceiver<AppEvent>,
    dict_dir: PathBuf,
    bundled_dict_dir: Option<PathBuf>,
    personal: PathBuf,
    // Named in the message a dead backend ends the session with (D1) — the loop
    // has no `Paths` of its own, and the caller does.
    log_dir: PathBuf,
    background_query: Option<crate::shared::osc11::Pending>,
) -> Result<()> {
    let mut terminal = ratatui::init();
    // Collect the terminal's background before the first `Palette` is built —
    // `Theme::Auto` reads it (spec §11.6). This is also where the raw mode the
    // query needed stops being ours: `ratatui::init` owns the terminal now and
    // restores it on exit, so `Pending` hands over rather than reverting.
    crate::shared::theme::set_detected_background(crate::shared::osc11::resolve(background_query));
    // The terminal window title = the brand name + version (matches the "About"
    // popup's title, `F1`). On Windows this works via the Console API
    // (`SetConsoleTitle` behind crossterm's `SetTitle`). On unix a console
    // application's title (outside a graphical emulator) doesn't change, so we set it
    // only on Windows.
    #[cfg(windows)]
    let _ = execute!(
        stdout(),
        ratatui::crossterm::terminal::SetTitle(format!(
            "{} v{}",
            crate::shared::credits::APP_NAME,
            env!("CARGO_PKG_VERSION"),
        )),
    );
    enable_key_modes();
    // Mouse capture is OFF by default: then native mouse text selection works. Feed
    // wheel scrolling is enabled via a toggle (`Ctrl+W`) — it sends
    // `EnableMouseCapture`/`DisableMouseCapture` (see `dispatch`). We augment ratatui's
    // panic hook by disabling the mouse and bracketed paste: otherwise after a panic
    // with these modes still on, the terminal would keep sending escape codes to the
    // shell. We also lift synchronized output here (DEC 2026, see the loop): a panic
    // inside `terminal.draw` happens between `?2026h` and `?2026l`, and without lifting
    // it the terminal would hold the frame frozen (the panic message not visible) until
    // its own timeout. DECRST of an unset mode is a no-op, the extra `?2026l` is
    // harmless.
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(
            stdout(),
            EndSynchronizedUpdate,
            DisableMouseCapture,
            DisableBracketedPaste
        );
        // Pop the kitty protocol if we pushed it (unix); harmless on an empty stack.
        #[cfg(unix)]
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
        prev_hook(info);
    }));
    let result = run_loop(
        &mut terminal,
        &cmd_tx,
        evt_rx,
        dict_dir,
        bundled_dict_dir,
        personal,
        log_dir,
    );
    // Lift the modes on exit (harmless if already off).
    let _ = execute!(
        stdout(),
        EndSynchronizedUpdate,
        DisableMouseCapture,
        DisableBracketedPaste
    );
    #[cfg(unix)]
    let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    ratatui::restore();
    // Ask the orchestrator to stop (in case of exiting other than via the Quit command).
    let _ = cmd_tx.send(AppCommand::Quit);
    result
}

/// The state of the background (re)loading of spellcheck dictionaries. A reload is
/// triggered by a change to `interface.spellcheck_enabled`/`selected_dictionaries`
/// (the `Settings` event); `generation` drops stale results. See spec §11.6.
struct SpellLoader {
    dict_dir: PathBuf,
    /// A fallback dictionary directory next to the binary (P1) — the source in
    /// non-portable storage mode, when there are no dictionaries in the data root.
    bundled_dir: Option<PathBuf>,
    personal: PathBuf,
    tx: Sender<(u64, SpellChecker)>,
    rx: Receiver<(u64, SpellChecker)>,
    /// The last applied settings `(enabled, dictionaries)` (None — not loaded yet).
    applied: Option<(bool, Vec<String>)>,
    /// The number of the last started load (only its result is applied).
    generation: u64,
}

impl SpellLoader {
    fn new(dict_dir: PathBuf, bundled_dir: Option<PathBuf>, personal: PathBuf) -> Self {
        let (tx, rx) = channel();
        Self {
            dict_dir,
            bundled_dir,
            personal,
            tx,
            rx,
            applied: None,
            generation: 0,
        }
    }

    /// If spellcheck settings changed — starts a background (re)load.
    fn maybe_reload(&mut self, enabled: bool, selected: &[String]) {
        let changed = self
            .applied
            .as_ref()
            .is_none_or(|(e, s)| *e != enabled || s.as_slice() != selected);
        if !changed {
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        let (dir, bundled, personal, tx) = (
            self.dict_dir.clone(),
            self.bundled_dir.clone(),
            self.personal.clone(),
            self.tx.clone(),
        );
        let selected = selected.to_vec();
        let sel_for_thread = selected.clone();
        std::thread::spawn(move || {
            let checker = dict::load(
                &dir,
                bundled.as_deref(),
                &personal,
                enabled,
                &sel_for_thread,
            );
            let _ = tx.send((generation, checker));
        });
        self.applied = Some((enabled, selected));
    }

    /// The finished checker of the latest load (stale ones are dropped), if any.
    fn poll(&self) -> Option<SpellChecker> {
        let mut latest = None;
        while let Ok((generation, checker)) = self.rx.try_recv() {
            if generation == self.generation {
                latest = Some(checker);
            }
        }
        latest
    }
}

/// What one pass over the orchestrator's event queue found.
enum Drain {
    /// Nothing waiting — the ordinary idle tick, which deliberately does not repaint.
    Idle,
    /// At least one event was applied, so the screen is dirty.
    Applied,
    /// The channel is **closed**: the orchestrator task has ended, and a panic is
    /// the case that matters — the hook restores the terminal and the task dies.
    ///
    /// Until this existed the loop read a closed channel exactly as an empty one
    /// (`while let Ok(event) = rx.try_recv()`), so the UI kept running and
    /// repainting with nothing on the other end: every command went into a channel
    /// with no reader, nothing ever answered, and the session could only be quit
    /// (docs/research/robustness-and-defaults.md D1).
    BackendGone,
}

/// Drains the queue, handing each event to `apply`, and says which of the three
/// things happened. Separated from [`run_loop`] because that loop needs a real
/// terminal and this decision is the part worth a test.
fn drain_events(rx: &mut UnboundedReceiver<AppEvent>, mut apply: impl FnMut(AppEvent)) -> Drain {
    let mut applied = false;
    loop {
        match rx.try_recv() {
            Ok(event) => {
                apply(event);
                applied = true;
            }
            Err(TryRecvError::Empty) => {
                return if applied { Drain::Applied } else { Drain::Idle };
            }
            // A closed channel wins over anything drained in the same pass: the
            // events are applied (the screen is correct), and the session ends.
            Err(TryRecvError::Disconnected) => return Drain::BackendGone,
        }
    }
}

fn run_loop(
    terminal: &mut DefaultTerminal,
    cmd_tx: &UnboundedSender<AppCommand>,
    mut evt_rx: UnboundedReceiver<AppEvent>,
    dict_dir: PathBuf,
    bundled_dict_dir: Option<PathBuf>,
    personal: PathBuf,
    log_dir: PathBuf,
) -> Result<()> {
    let mut screen = ChatScreen::new();
    // The chat list (Esc) or settings (Ctrl+P) can be open on top of the chat.
    // Orchestrator events keep applying to the chat (generation isn't interrupted).
    let mut active = ActiveScreen::Chat;
    // One step back from a chat opened out of the search results (see
    // `SearchReturn`). Lives beside `active` rather than inside it: it has to
    // survive while another screen is in front.
    let mut back: Option<Back> = None;
    // The clipboard is created lazily on the first copy (on headless Linux without
    // X11/Wayland the constructor may fail — then we show an error, not panic).
    let mut clipboard: Option<arboard::Clipboard> = None;
    let mut spell = SpellLoader::new(dict_dir, bundled_dict_dir, personal);
    // The help dialog, drawn over whatever screen is active (`F1` anywhere).
    let mut help = HelpOverlay::new();
    let mut quit = false;
    // We repaint ONLY on change (the `dirty` flag), not on every tick.
    // Otherwise `terminal.draw` is called ~20 times/sec and repositions the cursor
    // every time (`frame.set_cursor_position`), and the terminal (especially Windows
    // Terminal) resets the blink phase on every cursor move → the cursor blinks more
    // often and unevenly, even though CPU stays ~0% (the buffer diff is empty). The
    // one timer-driven animation is a spinner, and it asks for a frame only when its
    // glyph changes (`spinner_frame_needed`), so idle ticks don't repaint. See spec §11.
    let mut dirty = true;
    // What the previous frame DREW — the facts a full repaint is decided from
    // (see `Drawn`, and below, at `prime_full_redraw`).
    let mut drawn = Drawn::nothing_yet(&active);
    // Set when the event channel turns out to be **closed** rather than empty (see
    // [`Drain`]).
    let mut backend_gone = false;
    while !quit {
        let drained = drain_events(&mut evt_rx, |event| {
            apply_event(
                &mut screen,
                &mut active,
                &mut back,
                &mut clipboard,
                cmd_tx,
                event,
            );
        });
        match drained {
            Drain::Idle => {}
            Drain::Applied => dirty = true,
            Drain::BackendGone => {
                backend_gone = true;
                break;
            }
        }
        if spellcheck_upkeep(&mut spell, &mut screen, &mut active) {
            dirty = true;
        }
        // While background indexing or impersonation runs, its spinner asks for a
        // frame each time its glyph changes — once a `SPINNER_STEP`, not every tick
        // (outside them, idle ticks don't repaint — `dirty`).
        if spinner_frame_needed(&active, &screen) {
            dirty = true;
            refresh_task_rows(&active, cmd_tx);
        }
        // The input-box draft changed — save it on the active chat (the orchestrator
        // writes it to disk with a debounce). This doesn't need a repaint. See spec §11.7.
        if let Some(draft) = screen.take_dirty_draft() {
            let _ = cmd_tx.send(AppCommand::SetDraft(draft));
        }
        if dirty {
            draw_frame(
                terminal,
                &mut screen,
                &mut active,
                &mut help,
                &back,
                &mut drawn,
            )?;
            dirty = false;
        }
        if handle_input_tick(
            &mut screen,
            &mut active,
            &mut help,
            &mut back,
            cmd_tx,
            &mut clipboard,
            &mut dirty,
            drawn.placeholder,
        )? {
            quit = true;
        }
    }
    // The draft is read at the TOP of the loop, so an edit made by the very tick
    // that quit would never be sent — and `/exit` is exactly that edit: the
    // command clears the box, and without this flush the box would come back
    // holding `/exit` on the next launch. The orchestrator applies commands in
    // order and `AppCommand::Quit` (sent by the caller) writes the chat out, so
    // this reaches disk. See spec §11.7.
    if let Some(draft) = screen.take_dirty_draft() {
        let _ = cmd_tx.send(AppCommand::SetDraft(draft));
    }
    if backend_gone {
        // The terminal is restored by `ratatui::init`'s hook on the way out of
        // this function, so the message belongs to the caller: `main` prints it
        // with the usual CLI prefix and exits non-zero. What is on disk is what
        // the orchestrator wrote before it died — it is the sole writer of a
        // chat (spec §4.4.2), so nothing half-written is left behind.
        let loc = screen.loc();
        bail!(
            "{}",
            loc.tf(
                "cli.err.backend_gone",
                &[("path", &log_dir.display().to_string())]
            )
        );
    }
    Ok(())
}

/// Per-tick spellcheck maintenance: dictionary (re)loading per settings, plugging
/// in a finished checker, the debounced recheck of the chat input, and the
/// chat-list rename field's recheck. Returns `true` when the highlighting or the
/// checker actually changed and a repaint is needed.
fn spellcheck_upkeep(
    spell: &mut SpellLoader,
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
) -> bool {
    let mut dirty = false;
    // Spellcheck settings received/changed — (re)load dictionaries in the background.
    if let Some((enabled, selected)) = screen.spell_config() {
        spell.maybe_reload(enabled, selected);
    }
    // A finished (re)load — plug in the checker (a disabled one flags nothing).
    if let Some(checker) = spell.poll() {
        screen.set_spellchecker(checker);
        dirty = true;
    }
    // A debounced spellcheck recheck: the loop runs every tick (the `poll`
    // timeout) even when it isn't drawing, so this is exactly where the debounce
    // wakeup happens. We repaint only when the highlighting actually got
    // recomputed. Chat input isn't active on the settings screen — skip it.
    if active.is_chat() && screen.maybe_recheck_spelling() {
        dirty = true;
    }
    // The rename field (`F2`) on the chat-list screen is also spellchecked — the
    // checker is borrowed from the chat screen (the owner). See spec §11.5.
    if let ActiveScreen::ChatList(list) = active
        && let Some(spell) = screen.spellchecker()
        && list.recheck_spelling(spell)
    {
        dirty = true;
    }
    dirty
}

/// Whether a frame must repaint without input: a spinner on the chat screen
/// (background indexing or impersonation) repaints when its glyph changes,
/// once a [`SPINNER_STEP`] — drawing on every tick wrote to the terminal so
/// often that Windows Terminal never re-found the links it underlines; the
/// tasks screen repaints once a second while a run is out, so its elapsed
/// column moves (spec §11.10) — and not at all once every run has landed.
fn spinner_frame_needed(active: &ActiveScreen, screen: &ChatScreen) -> bool {
    match active {
        ActiveScreen::Chat => screen.spinner_due(),
        ActiveScreen::Tasks(tasks) => tasks.needs_repaint(),
        _ => false,
    }
}

/// A silent task's *waiting* state flips inside the task (spec §11.10), so
/// while one runs the once-a-second tick re-asks for the tasks screen's rows
/// and the screen says which task streams and which waits.
fn refresh_task_rows(active: &ActiveScreen, cmd_tx: &UnboundedSender<AppCommand>) {
    if let ActiveScreen::Tasks(tasks) = active
        && tasks.has_app_task_running()
    {
        let _ = cmd_tx.send(AppCommand::RequestTasks);
    }
}

/// What the previous frame put on the terminal — tracked by the actual draw, so
/// switching "there and back" between two frames changes nothing.
struct Drawn {
    /// Which screen was drawn: a switch requires a full repaint.
    screen: std::mem::Discriminant<ActiveScreen>,
    /// Whether the help overlay was drawn — its toggle is a screen switch for
    /// repaint purposes (the dialog's arrows/keycaps are exactly the wide-glyph
    /// risk group a cell diff leaves artifacts of).
    overlay: bool,
    /// The canvas the terminal was last erased to (`Color::Reset` — its own
    /// background, which is what `ratatui::init` leaves). See [`canvas_repaint`].
    canvas: Color,
    /// Whether the frame was the "window too small" placeholder
    /// ([`too_small`]), and the keys it named as working. It decides two
    /// things: the keys of the next tick act on what the user was looking at
    /// — under the placeholder only those it names ([`route_batch`]) — and
    /// the placeholder coming or going is a wholesale change of the frame,
    /// like a screen switch.
    placeholder: Option<WayOut>,
}

impl Drawn {
    fn nothing_yet(active: &ActiveScreen) -> Self {
        Self {
            screen: std::mem::discriminant(active),
            overlay: false,
            canvas: Color::Reset,
            placeholder: None,
        }
    }
}

/// What a frame has to do about the canvas before it draws (the full colour
/// mode, spec §11.6, docs/history/theme-modes.md §4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CanvasRepaint {
    /// The terminal already has this frame's canvas under it.
    None,
    /// The canvas changed — the first full-mode frame, another theme, another
    /// mode: erase the terminal to the new one and repaint every cell.
    Erase,
    /// The terminal was resized while a canvas is painted: ratatui is about to
    /// erase it, and the erase has to be made with the canvas current, or the
    /// cells it never writes come out in the terminal's own colour.
    Resize,
}

/// The decision, apart from the terminal it is carried out on. A resize wins
/// over a change: ratatui's resize erases the screen and resets its buffers,
/// which is everything a change of canvas needs as well.
fn canvas_repaint(last: Color, canvas: Color, resized: bool) -> CanvasRepaint {
    if resized && canvas != Color::Reset {
        CanvasRepaint::Resize
    } else if canvas != last {
        CanvasRepaint::Erase
    } else {
        CanvasRepaint::None
    }
}

/// Whether the terminal is no longer the size ratatui last laid a frame out
/// for — what `Terminal::draw` is about to find out and erase the screen over.
fn terminal_resized(terminal: &mut DefaultTerminal) -> bool {
    let known = terminal.get_frame().area();
    terminal
        .size()
        .is_ok_and(|now| (now.width, now.height) != (known.width, known.height))
}

/// Erases the terminal with `canvas` as the current background, so that every
/// cell — the ones no frame ever writes included — starts from it.
///
/// `terminal.clear()` is what the project does not use for a repaint (it
/// flickers, docs/lessons.md §5); this is an erase of the same kind, kept to
/// the rare moments the canvas itself changes, because it is the only way to
/// colour a cell that is never written. The style is reset whatever happened:
/// ratatui's backend starts every draw assuming a reset terminal.
fn erase_to_canvas(
    terminal: &mut DefaultTerminal,
    canvas: Color,
    repaint: CanvasRepaint,
) -> std::io::Result<()> {
    use std::io::Write;
    let mut out = stdout();
    out.write_all(ui::set_background(canvas).as_bytes())?;
    let erased = match repaint {
        // ratatui's own erase, made now rather than inside `draw` — where it
        // would run with the terminal's default background current.
        CanvasRepaint::Resize => out.flush().and_then(|()| terminal.autoresize()),
        CanvasRepaint::Erase | CanvasRepaint::None => out.write_all(ui::ERASE_SCREEN.as_bytes()),
    };
    let reset = out
        .write_all(ui::RESET_STYLE.as_bytes())
        .and_then(|()| out.flush());
    erased.and(reset)
}

/// Tells crossterm whether it may write colours — by the **colour mode**, not
/// by the environment (spec §11.6).
///
/// crossterm honours `NO_COLOR` on its own: with the variable set it writes no
/// colour at all, and in place of every colour change it writes `ESC[;m` — a
/// reset of every attribute, the ones ratatui's backend set for that same cell
/// a moment earlier included. Left to itself that made `NO_COLOR` a mode of
/// its own, nobody's design: no colours, and whichever bold shared a cell with
/// a colour gone too. Here the variable decides only the mode nobody chose
/// (`ThemeMode::for_environment`), and a mode chosen in settings wins over it
/// — which has to be true on the screen, not only in `settings.json`. So
/// colours are written in every mode but the monochrome one, where no cell
/// carries any and nothing is written either way.
///
/// Called for every frame: the mode changes in settings, and the call is an
/// atomic store.
fn set_colour_output(palette: &Palette) {
    ratatui::crossterm::style::force_color_output(!palette.mono);
}

/// The palette of the screen in front: the settings screen draws with its
/// **working copy** — a theme picked there is on screen before the saved
/// configuration has come back as an event — and every other screen with the
/// palette the chat screen holds, which the same event keeps in step.
fn front_palette(active: &ActiveScreen, screen: &ChatScreen) -> Palette {
    match active {
        ActiveScreen::Settings(settings) => settings.palette(),
        _ => screen.palette(),
    }
}

/// Draws one frame for the active screen (the `dirty` branch of [`run_loop`]'s
/// tick): the `Esc` hint, the full-repaint decision ([`Drawn`] tracks what the
/// previous frame put on the terminal), and the draw itself wrapped in
/// synchronized output. The comments inside are load-bearing.
fn draw_frame(
    terminal: &mut DefaultTerminal,
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
    help: &mut HelpOverlay,
    back: &Option<Back>,
    last: &mut Drawn,
) -> Result<()> {
    // The status bar's `Esc` hint, derived from the back-stack for this
    // frame (see `esc_target`). The stash only ever changes while
    // handling an event or a keypress, i.e. in an iteration that is
    // already `dirty`, so the hint is never a frame behind.
    screen.set_esc_target(esc_target(back));
    // The frame is wrapped in synchronized output (DEC private mode 2026):
    // `?2026h` before drawing, `?2026l` after — the terminal buffers everything
    // in between and applies the frame ATOMICALLY. Without this, the hardware
    // cursor was visible at intermediate write states: ratatui writes the diff
    // with the cursor visible (the terminal cursor = the write position) and
    // returns it to the input box via separate writes AFTER the diff
    // (`show_cursor`/`set_cursor_position` on CrosstermBackend are `execute!`
    // with an immediate flush; a large diff is also chopped up by stdout's small
    // buffer). Windows Terminal renders asynchronously and would show the cursor
    // at the diff's last written cell: during generation that's the token
    // counter (the status bar's bottom lines are written last), during RAG
    // indexing — the banner spinner. The cursor "jumped" between the input box
    // and these cells at the frame rate (~20/s).
    //
    // Terminals without 2026 support (conhost's compat mode) ignore the
    // unfamiliar private mode — graceful degradation (the jump stays, as
    // before). The draw error is propagated AFTER lifting the mode, so the
    // terminal doesn't stay in buffering mode. See spec §4.4.1.
    // A FULL repaint is needed wherever a wide glyph leaves its spot or
    // appears at a new one, leaving a "hanging" artifact: a cell-by-cell diff
    // sometimes doesn't send that glyph's trailing half, sometimes sends it
    // without `MoveTo` and shifts the row (open upstream issue ratatui#2651).
    // Every cell needs to be explicitly rewritten, including spaces in empty
    // spots.
    //
    // The "how" mechanics — in `ui::prime_full_redraw` (a sentinel in the
    // buffer + `swap_buffers` without flushing to the screen, instead of
    // `terminal.clear()` with its flickering `ESC[2J`). The internal swap
    // inside `draw` restores the invariant "back buffer = screen".
    //
    // Two triggers:
    //  * the chat screen — scrolling/a feed change with risk-group glyphs and
    //    closing the emoji/spellcheck popups (`take_full_redraw`);
    //  * SCREEN SWITCH — the frame's content changes wholesale, and a VS16
    //    glyph (`❤️`, `🗂️`) on the new screen lands where a foreign character
    //    used to be. Then the diff sends its trailing half (the character did
    //    change), the backend prints half without `MoveTo`, and the rest of
    //    the row shifts right — after a feed with `❤️`, returning from the
    //    chat list/`F3` left an extra space, which only went away on scroll
    //    (which triggers this same repaint). Confirmed by
    //    `ui::screen_switch_emits_vs16_tail_without_full_redraw`.
    //
    //  * a CHANGE OF CANVAS (the full colour mode, spec §11.6) — every cell of
    //    the frame gets another background, and the terminal is erased to it
    //    first (`erase_to_canvas`), so the diff's idea of what is on the
    //    screen is void.
    //
    // Outside these cases, plain text always goes through the regular diff.
    // See spec §11.3, §11.5.
    let requested = if matches!(active, ActiveScreen::Chat) {
        screen.take_full_redraw()
    } else {
        false
    };
    let now_screen = std::mem::discriminant(active);
    let switched = now_screen != last.screen;
    last.screen = now_screen;
    // The help overlay's toggle is a screen switch for repaint purposes: the
    // dialog appearing or vanishing changes the frame wholesale, and its
    // keycap/arrow glyphs are the risk group the cell diff mishandles.
    let overlay = help.open.is_some();
    let overlay_toggled = overlay != last.overlay;
    last.overlay = overlay;
    // So is the placeholder's coming or going — and that needs no resize: a
    // popup opening over a screen raises what the window has to hold. Decided
    // from the size the frame is about to get, before the render, like the
    // other triggers (docs/lessons.md §5).
    let cramped = terminal.size().is_ok_and(|size| {
        too_small(
            Rect::new(0, 0, size.width, size.height),
            active,
            screen,
            help,
        )
        .is_some()
    });
    let cramped_toggled = cramped != last.placeholder.is_some();
    // The palette of this frame: the screen in front, the help dialog over it
    // and the canvas under both are drawn with the same one.
    let palette = front_palette(active, screen);
    let repaint = canvas_repaint(
        last.canvas,
        palette.canvas,
        // Asked only while a canvas is painted: in the system mode ratatui's
        // own erase on a resize is already the right one.
        palette.canvas != Color::Reset && terminal_resized(terminal),
    );
    last.canvas = palette.canvas;
    set_colour_output(&palette);
    if requested
        || switched
        || overlay_toggled
        || cramped_toggled
        || repaint == CanvasRepaint::Erase
    {
        ui::prime_full_redraw(terminal.current_buffer_mut());
        terminal.swap_buffers();
    }
    let _ = execute!(stdout(), BeginSynchronizedUpdate);
    // Inside the synchronized update, so a terminal that honours it shows the
    // erase and the frame as one step.
    let erased = match repaint {
        CanvasRepaint::None => Ok(()),
        _ => erase_to_canvas(terminal, palette.canvas, repaint),
    };
    // What the frame turned out to be is read back from the render itself:
    // the keys of the next tick are judged by what was on screen, and the
    // size asked about above can be a resize behind the one `draw` sees.
    let drawn = present(terminal, screen, active, help, &palette);
    if let Ok(placeholder) = drawn {
        last.placeholder = placeholder;
    }
    let _ = execute!(stdout(), EndSynchronizedUpdate);
    erased?;
    drawn?;
    Ok(())
}

/// Composes one frame into `terminal` ([`compose_frame`]) and puts it on the
/// screen, returning which keys the placeholder named, if the frame was the
/// placeholder.
///
/// **The cursor is the frame's only while nothing covers the screen that
/// placed it.** An input box places the terminal's cursor when it has the
/// focus, and the screen under the help dialog does not know the dialog is
/// there: the runtime draws it over whatever is in front (spec §11.7), from
/// every screen and every sub-mode — a rename field, an editor. So the cursor
/// of the box under the dialog kept blinking through it
/// (docs/research/small-terminal.md §7.3). ratatui's `Frame` can be given a
/// cursor and not have it taken back, so with the dialog open the frame is
/// presented the way `Terminal::draw` presents one — resize, render, apply —
/// with the cursor left out (`apply_buffer` hides it). The dialog has no
/// input of its own, so nothing is lost. One place for every screen: a box
/// added under the dialog later is covered without knowing it.
fn present<B: Backend>(
    terminal: &mut Terminal<B>,
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
    help: &mut HelpOverlay,
    palette: &Palette,
) -> Result<Option<WayOut>, B::Error> {
    if help.open.is_none() {
        let mut placeholder = None;
        terminal.draw(|frame| {
            placeholder = compose_frame(frame, screen, active, help, palette);
        })?;
        return Ok(placeholder);
    }
    terminal.autoresize()?;
    let placeholder = compose_frame(&mut terminal.get_frame(), screen, active, help, palette);
    terminal.apply_buffer()?;
    Ok(placeholder)
}

/// The window the frame in front needs, when `area` is smaller than that
/// (spec §11.1.1, docs/research/small-terminal.md F4): the active screen's
/// minimum — which already counts the popup open over it — and the help
/// dialog's while it is up. `None` — the frame fits.
///
/// Every minimum is a function of the window and of **which** layers are
/// open, never of what they hold at the moment, so the placeholder comes and
/// goes with a resize or with a popup and not with a token counter.
///
/// With the size comes the way out of the placeholder ([`WayOut`]): `Esc`
/// works, beside quit, exactly while something is open over the chat — the
/// help, a popup, another screen. That something may be all that does not
/// fit, and opening it took a key, not a resize: a picker opened in a window
/// the chat fits has to be closable from the keyboard.
fn too_small(
    area: Rect,
    active: &ActiveScreen,
    screen: &ChatScreen,
    help: &HelpOverlay,
) -> Option<(MinSize, WayOut)> {
    let mut need = match active {
        ActiveScreen::Chat => screen.min_size(area),
        ActiveScreen::ChatList(list) => list.min_size(),
        ActiveScreen::Settings(settings) => settings.min_size(),
        ActiveScreen::SelfModel(view) => view.min_size(),
        ActiveScreen::Search(search) => search.min_size(),
        ActiveScreen::Changes(changes) => changes.min_size(area),
        ActiveScreen::Tasks(tasks) => tasks.min_size(),
    };
    if help.open.is_some() {
        need = need.max(help_dialog::min_size(screen.loc()));
    }
    let way_out = if help.open.is_some() || !active.is_chat() || screen.has_popup() {
        WayOut::EscOrQuit
    } else {
        WayOut::Quit
    };
    (!need.fits(area)).then_some((need, way_out))
}

/// What one frame is made of, in the order it is drawn: the active screen,
/// then — modal above every one of them — the help dialog (spec §11.7), then
/// the canvas under everything either of them left at the terminal's default
/// (the full colour mode, spec §11.6). Apart from [`draw_frame`] because that
/// needs a real terminal, and the order is the part worth a test.
///
/// A window smaller than the frame in front needs gets the placeholder
/// instead of a frame with parts missing ([`too_small`]); the return value
/// says that it did, and which keys it named.
fn compose_frame(
    frame: &mut ratatui::Frame,
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
    help: &mut HelpOverlay,
    palette: &Palette,
) -> Option<WayOut> {
    // The locale comes from the chat screen, the base that always exists and
    // receives every settings event.
    let loc = screen.loc();
    if let Some((need, way_out)) = too_small(frame.area(), active, screen, help) {
        ui::render_too_small(frame, palette, loc, need, way_out);
        ui::finish_frame(frame.buffer_mut(), palette);
        return Some(way_out);
    }
    match active {
        ActiveScreen::Chat => screen.render(frame),
        ActiveScreen::ChatList(list) => list.render(frame),
        ActiveScreen::Settings(settings) => settings.render(frame),
        ActiveScreen::SelfModel(view) => view.render(frame),
        ActiveScreen::Search(search) => search.render(frame),
        ActiveScreen::Changes(changes) => changes.render(frame),
        ActiveScreen::Tasks(tasks) => tasks.render(frame),
    }
    if let Some(state) = help.open.as_mut() {
        dim_background(frame, palette);
        help_dialog::render_help(frame, state, &HELP_SECTIONS, palette, loc);
    }
    // Last, so it also catches what the overlay's `Clear` reset — and, in the
    // monochrome mode, what the overlay drew.
    ui::finish_frame(frame.buffer_mut(), palette);
    None
}

/// One input tick: polls the terminal for [`TICK`], collects the available
/// events into a batch (chasing a paste's tail — see the comments inside) and
/// processes it. Sets `dirty` when any terminal event arrived; returns `true`
/// if quitting was requested.
///
/// `placeholder` — the frame on screen is the "window too small" placeholder
/// ([`Drawn::placeholder`]): the batch is then read for the keys it names and
/// for nothing else ([`route_batch`]).
// The loop's state, one borrow each — the same set `process_input_batch`
// takes, plus the two flags the tick itself reads and writes.
#[allow(clippy::too_many_arguments)]
fn handle_input_tick(
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
    help: &mut HelpOverlay,
    back: &mut Option<Back>,
    cmd_tx: &UnboundedSender<AppCommand>,
    clipboard: &mut Option<arboard::Clipboard>,
    dirty: &mut bool,
    placeholder: Option<WayOut>,
) -> Result<bool> {
    if !event::poll(TICK)? {
        return Ok(false);
    }
    // Any terminal event (input, scroll, resize) may change the view.
    *dirty = true;
    let batch = read_batch()?;
    Ok(route_batch(
        placeholder,
        batch,
        screen,
        active,
        help,
        back,
        cmd_tx,
        clipboard,
    ))
}

/// Reads the next batch of input, waiting for its first event. Shared with
/// `mindfork keys` (`app::key_echo`), so a burst reads there as it reads here.
///
/// Drains ALL currently available events at once. On Windows a clipboard
/// paste arrives as a batch of regular key events (there's no Event::Paste
/// there — see `run`). Without batching this is a repaint per character
/// (laggy), and an Enter inside the text = a send. We coalesce the batch in
/// `process_input_batch` ([`chunk_batch`]). Only presses are kept
/// ([`collect_press`]).
pub(crate) fn read_batch() -> Result<Vec<Event>> {
    let mut batch = Vec::new();
    collect_press(&mut batch, event::read()?);
    while event::poll(Duration::ZERO)? {
        collect_press(&mut batch, event::read()?);
    }
    // Looks like a paste (a burst of events in one drain) — we chase its tail
    // with a short pause-detector (`PASTE_GAP`), so a large paste made of
    // several console chunks gets collected into ONE batch. Otherwise a chunk
    // boundary breaks the run and a lone `Enter` slips through as a send
    // (Windows).
    if batch.len() >= PASTE_BURST {
        while event::poll(PASTE_GAP)? {
            collect_press(&mut batch, event::read()?);
        }
    }
    Ok(batch)
}

// ---------- submodules (god-object breakup: docs/history/refactoring-god-objects.md, stage 7) ----------

mod clipboard;
mod dispatch;
mod input;

// Internal wiring: run_loop calls input batching (input), event application and
// dispatch (dispatch), the clipboard (clipboard). The external surface is run —
// and, for `mindfork keys`, how a batch is read and split.
pub(crate) use self::input::{Chunk, chunk_batch};
use self::{clipboard::*, dispatch::*, input::*};

#[cfg(test)]
mod demo_reel;
#[cfg(test)]
mod small_window_tests;
#[cfg(test)]
mod tests;

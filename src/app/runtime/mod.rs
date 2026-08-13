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

use anyhow::Result;
use ratatui::DefaultTerminal;
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
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

use crate::app::events::{AppCommand, AppEvent, BackgroundKind, ClipboardImage};
use crate::features::spellcheck::{SpellChecker, dict};
use crate::screens::chat::{ChatIntent, ChatScreen};
use crate::screens::chat_list::{ChatListIntent, ChatListScreen};
use crate::screens::search::{SearchIntent, SearchScreen};
use crate::screens::self_model::{SelfModelIntent, SelfModelScreen};
use crate::screens::settings::{SettingsIntent, SettingsScreen};
use crate::shared::theme::Palette;
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
            ActiveScreen::Search(_) => {}
        }
    }
}

/// A one-deep back-stack for the message-level search results: where `Esc` in the
/// chat goes when that chat was reached by opening a hit
/// ([`SearchIntent::OpenHit`]). Without it the user drilled down from the results
/// and `Esc` threw them away, dropping into the chat list.
///
/// The **screen itself** is stashed, not the query: re-running the search would
/// lose the selection and the scroll position, and working through a list of hits
/// one by one is exactly what going back is for. It is deliberately session state
/// — a local of [`run_loop`], never persisted.
///
/// The chat screen knows nothing about any of this (FSD: `screens` may not depend
/// on `app`). [`ChatIntent::OpenChatList`] already means "go back" from its point
/// of view; which screen that is gets resolved in [`dispatch`].
struct SearchReturn {
    /// The live results screen, with its selection and scroll intact. Boxed as it
    /// is inside [`ActiveScreen`] — the screen is large
    /// (clippy::large_enum_variant).
    screen: Box<SearchScreen>,
    /// The chat the jump opened. Activating a **different** chat means the user
    /// left by an ordinary route (picking one in the list, `Ctrl+N`, a clone…),
    /// at which point the results are no longer where they came from — see the
    /// `ChatActivated` arm of [`apply_event`], the single funnel every one of
    /// those routes ends in. Re-activating the *same* chat (regeneration,
    /// deleting an exchange, a repeat jump) is not leaving it, so it keeps the
    /// way back.
    chat: Uuid,
}

/// Where `Esc` currently goes, for the status bar's hint — **derived** from the
/// back-stack, never mirrored into a second flag.
///
/// Setting a flag at each place the stash changes would mean writing the same
/// rule twice (and the clearing three times, counting the `ChatActivated`
/// funnel), which is exactly how a hint drifts away from the key it describes.
/// Deriving it in the draw path instead makes "the bar says where `Esc` goes"
/// true of every frame by construction.
fn esc_target(back: &Option<SearchReturn>) -> EscTarget {
    match back {
        Some(_) => EscTarget::SearchResults,
        None => EscTarget::ChatList,
    }
}

/// The input polling period (the repaint tick).
const TICK: Duration = Duration::from_millis(50);

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
) -> Result<()> {
    let mut terminal = ratatui::init();
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
    // On unix we enable bracketed paste: crossterm delivers a clipboard paste as ONE
    // `Event::Paste` event (whole, with line breaks as text, not Enter). Windows has no
    // such mode in crossterm (input is read via the Console API), there a paste arrives
    // as a batch of regular key events — we collect it in the loop
    // (`process_input_batch`), so there's nothing to enable here. See spec §11.5.
    //
    // Here (unix) we also enable the kitty keyboard protocol at the "disambiguate"
    // level: the terminal's legacy encoding sends the same CR for `Shift+Enter` and
    // `Enter`, so a line break in the input box was unavailable on a "bare" unix
    // terminal. With `DISAMBIGUATE_ESCAPE_CODES` the terminal reports modifiers for
    // special keys (Enter/arrows/…), and `Shift+Enter` becomes distinguishable from
    // `Enter` (and `Shift`+arrows — from bare arrows, which enables keyboard-driven
    // selection). We push it only if the terminal supports the protocol (otherwise a
    // no-op); we pop it on exit and in the panic hook. Text input and a lone
    // `Shift`+character don't touch this flag (text arrives as is), so
    // layout-independent parsing of Ctrl shortcuts (`shared::keys`) and typing
    // `?`/emoji don't regress. Not needed on Windows — the Console API already reports
    // modifiers. `Alt+Enter` in the input box is a fallback line break for terminals
    // without this protocol (see spec §11.5, audit item 11).
    #[cfg(unix)]
    {
        let _ = execute!(stdout(), EnableBracketedPaste);
        if supports_keyboard_enhancement().unwrap_or(false) {
            let _ = execute!(
                stdout(),
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            );
        }
    }
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

fn run_loop(
    terminal: &mut DefaultTerminal,
    cmd_tx: &UnboundedSender<AppCommand>,
    mut evt_rx: UnboundedReceiver<AppEvent>,
    dict_dir: PathBuf,
    bundled_dict_dir: Option<PathBuf>,
    personal: PathBuf,
) -> Result<()> {
    let mut screen = ChatScreen::new();
    // The chat list (Esc) or settings (Ctrl+P) can be open on top of the chat.
    // Orchestrator events keep applying to the chat (generation isn't interrupted).
    let mut active = ActiveScreen::Chat;
    // One step back from a chat opened out of the search results (see
    // `SearchReturn`). Lives beside `active` rather than inside it: it has to
    // survive while another screen is in front.
    let mut back: Option<SearchReturn> = None;
    // The clipboard is created lazily on the first copy (on headless Linux without
    // X11/Wayland the constructor may fail — then we show an error, not panic).
    let mut clipboard: Option<arboard::Clipboard> = None;
    let mut spell = SpellLoader::new(dict_dir, bundled_dict_dir, personal);
    let mut quit = false;
    // We repaint ONLY on change (the `dirty` flag), not on every tick.
    // Otherwise `terminal.draw` is called ~20 times/sec and repositions the cursor
    // every time (`frame.set_cursor_position`), and the terminal (especially Windows
    // Terminal) resets the blink phase on every cursor move → the cursor blinks more
    // often and unevenly, even though CPU stays ~0% (the buffer diff is empty). There
    // are no timer-driven animations in rendering, so idle ticks don't need to
    // repaint. See spec §11.
    let mut dirty = true;
    // Which screen was DRAWN in the previous frame: a switch requires a full
    // repaint (see below, at `prime_full_redraw`). We track this by the actual draw —
    // switching "there and back" between frames changes nothing visually.
    let mut last_screen = std::mem::discriminant(&active);
    while !quit {
        while let Ok(event) = evt_rx.try_recv() {
            apply_event(
                &mut screen,
                &mut active,
                &mut back,
                &mut clipboard,
                cmd_tx,
                event,
            );
            dirty = true;
        }
        if spellcheck_upkeep(&mut spell, &mut screen, &mut active) {
            dirty = true;
        }
        // While background RAG indexing or impersonation is running — repaint every
        // tick for the spinner animation (outside them, idle ticks don't repaint —
        // `dirty`).
        if spinner_frame_needed(&active, &screen) {
            dirty = true;
        }
        // The input-box draft changed — save it on the active chat (the orchestrator
        // writes it to disk with a debounce). This doesn't need a repaint. See spec §11.7.
        if let Some(draft) = screen.take_dirty_draft() {
            let _ = cmd_tx.send(AppCommand::SetDraft(draft));
        }
        if dirty {
            draw_frame(terminal, &mut screen, &mut active, &back, &mut last_screen)?;
            dirty = false;
        }
        if handle_input_tick(
            &mut screen,
            &mut active,
            &mut back,
            cmd_tx,
            &mut clipboard,
            &mut dirty,
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

/// Whether a spinner animation is on screen (background RAG indexing or
/// impersonation on the chat screen) — those frames repaint every tick.
fn spinner_frame_needed(active: &ActiveScreen, screen: &ChatScreen) -> bool {
    active.is_chat() && (screen.is_rag_active() || screen.is_impersonating())
}

/// Draws one frame for the active screen (the `dirty` branch of [`run_loop`]'s
/// tick): the `Esc` hint, the full-repaint decision (`last_screen` tracks which
/// screen was drawn in the previous frame), and the draw itself wrapped in
/// synchronized output. The comments inside are load-bearing.
fn draw_frame(
    terminal: &mut DefaultTerminal,
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
    back: &Option<SearchReturn>,
    last_screen: &mut std::mem::Discriminant<ActiveScreen>,
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
    // Outside these cases, plain text always goes through the regular diff.
    // See spec §11.3, §11.5.
    let requested = if matches!(active, ActiveScreen::Chat) {
        screen.take_full_redraw()
    } else {
        false
    };
    let now_screen = std::mem::discriminant(active);
    let switched = now_screen != *last_screen;
    *last_screen = now_screen;
    if requested || switched {
        crate::shared::ui::prime_full_redraw(terminal.current_buffer_mut());
        terminal.swap_buffers();
    }
    let _ = execute!(stdout(), BeginSynchronizedUpdate);
    let drawn = match active {
        ActiveScreen::Chat => terminal.draw(|frame| screen.render(frame)),
        ActiveScreen::ChatList(list) => terminal.draw(|frame| list.render(frame)),
        ActiveScreen::Settings(settings) => terminal.draw(|frame| settings.render(frame)),
        ActiveScreen::SelfModel(view) => terminal.draw(|frame| view.render(frame)),
        ActiveScreen::Search(search) => terminal.draw(|frame| search.render(frame)),
    };
    let _ = execute!(stdout(), EndSynchronizedUpdate);
    drawn?;
    Ok(())
}

/// One input tick: polls the terminal for [`TICK`], collects the available
/// events into a batch (chasing a paste's tail — see the comments inside) and
/// processes it. Sets `dirty` when any terminal event arrived; returns `true`
/// if quitting was requested.
fn handle_input_tick(
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
    back: &mut Option<SearchReturn>,
    cmd_tx: &UnboundedSender<AppCommand>,
    clipboard: &mut Option<arboard::Clipboard>,
    dirty: &mut bool,
) -> Result<bool> {
    if !event::poll(TICK)? {
        return Ok(false);
    }
    // Any terminal event (input, scroll, resize) may change the view.
    *dirty = true;
    // Drain ALL currently available events at once. On Windows a clipboard
    // paste arrives as a batch of regular key events (there's no Event::Paste
    // there — see `run`). Without batching this is a repaint per character
    // (laggy), and an Enter inside the text = a send. We coalesce the batch in
    // `process_input_batch`.
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
    Ok(process_input_batch(
        batch, screen, active, back, cmd_tx, clipboard,
    ))
}

// ---------- submodules (god-object breakup: docs/history/refactoring-god-objects.md, stage 7) ----------

mod clipboard;
mod dispatch;
mod input;

// Internal wiring: run_loop calls input batching (input), event application and
// dispatch (dispatch), the clipboard (clipboard). The external surface is run.
use self::{clipboard::*, dispatch::*, input::*};

#[cfg(test)]
mod tests;

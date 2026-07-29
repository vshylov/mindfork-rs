//! Runtime — input batching and clipboard paste (Windows path): key coalescing, Chunk, reconciliation against the clipboard. Part of the [`super`] module, split out of the
//! runtime.rs monolith (see docs/history/refactoring-god-objects.md, stage 7).

use super::*;

/// The "burst" threshold for events in a single drain, at which we suspect a paste and
/// chase its tail (see `run_loop`). A human doesn't type that many keys in one
/// zero-timeout drain — several events at once mean a paste.
pub(super) const PASTE_BURST: usize = 2;

/// The pause detector for the end of a paste. On Windows a large paste arrives in
/// several chunks through the console buffer, and the loop drains them over different
/// iterations — at a chunk boundary a character run would break, and a lone `Enter`
/// at the boundary would slip through as a send. While events keep arriving with a gap
/// smaller than this threshold, we treat them as one paste; real human typing has
/// pauses much larger than that (>100ms reaction time).
pub(super) const PASTE_GAP: Duration = Duration::from_millis(20);

/// Reconciles a paste reconstructed from key events against the clipboard and, if it's
/// the same paste, returns its full version from the clipboard (with emoji restored).
///
/// **Why (Windows):** when pasting from the clipboard, the Windows console delivers the
/// text as ordinary key events, and crossterm 0.29 **loses** supplementary-plane
/// characters (emoji like 😊, U+1F60A): they're encoded as a UTF-16 surrogate pair, and
/// the console's key-down/key-up records break the pair assembly in crossterm — the
/// character is lost before it even reaches our layer. BMP characters (letters, ❤
/// U+2764, the U+FE0F selector) pass through. So the reconstruction = the paste minus
/// supplementary-plane emoji.
///
/// To recover the emoji, we read the clipboard and check: if we drop from it exactly the
/// characters the console loses (code points > U+FFFF), and normalize line breaks/tabs,
/// does it match the reconstruction? A match → it's the same paste, we hand out the full
/// clipboard text. No match (a stale clipboard/a different paste/unavailable) → the
/// reconstruction (without emoji, but with no risk of pasting someone else's data). On
/// non-Windows this is the identity function (a correct bracketed paste `Event::Paste`
/// arrives there).
#[cfg(windows)]
pub(super) fn reconcile_paste(
    reconstructed: String,
    clipboard: &mut Option<arboard::Clipboard>,
) -> String {
    match read_clipboard_text(clipboard) {
        Some(clip) if paste_projection_matches(&clip, &reconstructed) => clip,
        _ => reconstructed,
    }
}

/// On non-Windows a paste arrives as correct UTF-8 (`Event::Paste`) — no reconciliation needed.
#[cfg(not(windows))]
pub(super) fn reconcile_paste(
    reconstructed: String,
    _clipboard: &mut Option<arboard::Clipboard>,
) -> String {
    reconstructed
}

/// Does the clipboard match the paste reconstruction by its "BMP projection" (a pure
/// function, testable with no clipboard/terminal). Characters the Windows console loses
/// (code points > U+FFFF — surrogate pairs) are dropped from the clipboard, then both
/// sides are normalized on line breaks/tabs (as in [`InputBox::insert_str`]). See
/// [`reconcile_paste`].
#[cfg(windows)]
pub(super) fn paste_projection_matches(clipboard: &str, reconstructed: &str) -> bool {
    fn normalize(s: &str) -> String {
        s.replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\t', "    ")
    }
    let projected: String = clipboard
        .chars()
        .filter(|c| (*c as u32) <= 0xFFFF)
        .collect();
    !reconstructed.is_empty() && normalize(&projected) == normalize(reconstructed)
}

/// Puts an event into the batch, dropping "release"/repeat key events: the app
/// ignores them anyway (`handle_key` only takes `Press`), and during paste coalescing
/// they would break the character run between keypresses (on Windows a paste arrives as
/// down/up pairs). Mouse/resize/`Paste` are passed through as is.
pub(super) fn collect_press(batch: &mut Vec<Event>, ev: Event) {
    if let Event::Key(k) = &ev
        && k.kind != KeyEventKind::Press
    {
        return;
    }
    batch.push(ev);
}

/// A character for paste coalescing: a text key / Enter / Tab without Ctrl/Alt.
/// `Enter → '\r'` (CRLF from the clipboard later collapses in `InputBox::insert_str`),
/// `Tab → '\t'`. Returns `None` for everything else (arrows, Ctrl shortcuts,
/// function keys) — those break the paste run.
pub(super) fn paste_char(key: &KeyEvent) -> Option<char> {
    if key.kind != KeyEventKind::Press
        || key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return None;
    }
    match key.code {
        KeyCode::Char(c) => Some(c),
        KeyCode::Enter => Some('\r'),
        KeyCode::Tab => Some('\t'),
        _ => None,
    }
}

/// A piece of the input batch: either a coalesced paste (a run of ≥2 text keys), or
/// a single event (a regular key/mouse/unix `Paste`).
pub(super) enum Chunk {
    Paste(String),
    Event(Event),
}

/// Splits an event batch into chunks, coalescing consecutive text keys
/// (see [`paste_char`]) into a single paste when there are ≥2 of them. A pure function —
/// testable with no screen/terminal. A run of length 1 (regular character typing / a
/// single Enter) stays a single event, so Enter still works as a send.
pub(super) fn chunk_batch(batch: Vec<Event>) -> Vec<Chunk> {
    let mut out = Vec::new();
    let mut iter = batch.into_iter().peekable();
    while let Some(ev) = iter.next() {
        if let Event::Key(key) = &ev
            && let Some(first) = paste_char(key)
        {
            let mut run = String::new();
            run.push(first);
            while let Some(Event::Key(k)) = iter.peek() {
                match paste_char(k) {
                    Some(c) => {
                        run.push(c);
                        iter.next();
                    }
                    None => break,
                }
            }
            if run.chars().count() >= 2 {
                out.push(Chunk::Paste(run));
                continue;
            }
            // A run of one key — hand it out as a regular event (below).
        }
        out.push(Chunk::Event(ev));
    }
    out
}

/// Processes a batch of terminal events in one pass. Coalesced pastes
/// go into the active editor (the settings screen) or into the chat's input box as
/// text — never sending, even if they contain line breaks. Single events — the regular way.
/// Returns `true` if quitting was requested.
pub(super) fn process_input_batch(
    batch: Vec<Event>,
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
    cmd_tx: &UnboundedSender<AppCommand>,
    clipboard: &mut Option<arboard::Clipboard>,
) -> bool {
    let mut quit = false;
    for chunk in chunk_batch(batch) {
        match chunk {
            // A clipboard paste: on the settings screen — into the active field editor; in
            // the chat — into the input box (never sends); the list has no paste target.
            // `Chunk::Paste` — a reconstruction from key events (Windows); we reconcile it
            // against the clipboard to recover crossterm's lost supplementary-plane
            // emoji (see [`reconcile_paste`]). `Event::Paste` — a real
            // bracketed paste (unix), already correct UTF-8.
            Chunk::Paste(text) | Chunk::Event(Event::Paste(text)) => {
                // `Chunk::Paste` — a reconstruction from key events (Windows); we reconcile it
                // against the clipboard to recover the emoji (see [`reconcile_paste`]).
                // Routing by screen is handled by `ActiveScreen::handle_paste`.
                let text = reconcile_paste(text, clipboard);
                active.handle_paste(screen, &text);
            }
            Chunk::Event(Event::Key(key)) => {
                // Take the intent off the active screen (the borrow ends at the
                // owned `AnyIntent` value), then dispatch through a single ownership —
                // otherwise `active`/`screen` borrows would conflict.
                let intent = match active {
                    ActiveScreen::Chat => screen.handle_key(key).map(AnyIntent::Chat),
                    ActiveScreen::ChatList(list) => list.handle_key(key).map(AnyIntent::List),
                    ActiveScreen::Settings(settings) => {
                        settings.handle_key(key).map(AnyIntent::Settings)
                    }
                    ActiveScreen::SelfModel(view) => view.handle_key(key).map(AnyIntent::SelfModel),
                    ActiveScreen::Search(search) => search.handle_key(key).map(AnyIntent::Search),
                };
                match intent {
                    // Copying the selection to the clipboard (`Ctrl+C`/`Ctrl+X`) —
                    // a UI-layer side effect: we already have the text, no need to go to the orchestrator.
                    // The `arboard` slot is here (`dispatch` doesn't have it). Success is silent,
                    // a failure (headless Linux with no X11) — a note in the feed.
                    Some(AnyIntent::Chat(ChatIntent::CopyToClipboard(text))) => {
                        if let Err(e) = write_clipboard(clipboard, &text) {
                            let loc = screen.loc();
                            screen.push_error(&loc.tf("ui.err.copy_failed", &[("err", &e)]));
                        }
                    }
                    Some(intent) => {
                        if dispatch_any(intent, cmd_tx, screen, active) {
                            quit = true;
                        }
                    }
                    None => {}
                }
            }
            // The mouse wheel scrolls the chat feed. On the list/settings (which have their own
            // navigation) scrolling is ignored.
            Chunk::Event(Event::Mouse(mouse)) if active.is_chat() => screen.handle_mouse(mouse),
            Chunk::Event(_) => {}
        }
    }
    quit
}

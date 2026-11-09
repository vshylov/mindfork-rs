//! Runtime — reading/writing the system clipboard (the arboard slot). Part of the [`super`] module, split out of the
//! runtime.rs monolith (see docs/history/refactoring-god-objects.md, stage 7).
//!
//! A copy has **two** halves: the local clipboard (`arboard`) and, when the
//! terminal is somewhere else, the terminal's own over OSC 52
//! ([`crate::shared::osc52`], docs/history/osc52-clipboard.md). Both copy
//! routes — the selection (`Ctrl+C`/`Ctrl+X`) and the whole conversation
//! (`F5`/`/copy`) — funnel through [`copy_text`], so they cannot disagree about
//! either half.

use std::io;
use std::path::PathBuf;

use crate::features::image_prepare::has_image_extension;
use crate::shared::i18n::Locale;
use crate::shared::osc52::{self, Osc52Mode};

/// Reads the system clipboard's text (lazily creating the client). `None` if the clipboard
/// is unavailable/empty/non-text. Used only on Windows to restore
/// emoji in a paste (see [`reconcile_paste`]).
#[cfg(windows)]
pub(super) fn read_clipboard_text(slot: &mut Option<arboard::Clipboard>) -> Option<String> {
    if slot.is_none() {
        *slot = arboard::Clipboard::new().ok();
    }
    slot.as_mut().and_then(|c| c.get_text().ok())
}

/// Reads any text on the clipboard, on **every** platform — the fallback half of the
/// `Ctrl+V` handler: with no image on the clipboard the key must still do what the help
/// overlay has always promised it does.
///
/// Separate from [`read_clipboard_text`], which is Windows-only and exists for a
/// different job (reconstructing a paste's lost supplementary-plane characters).
pub(super) fn clipboard_text(slot: &mut Option<arboard::Clipboard>) -> Option<String> {
    if slot.is_none() {
        *slot = arboard::Clipboard::new().ok();
    }
    slot.as_mut()
        .and_then(|c| c.get_text().ok())
        .filter(|t| !t.is_empty())
}

/// Reads an image off the system clipboard as `(width, height, RGBA8 row-major)`.
///
/// `None` covers every "not an image" case alike — an unavailable clipboard, text on it,
/// nothing on it — because the caller's next step is the same in all of them and arboard
/// does not distinguish them usefully either.
///
/// The pixels come back raw: arboard normalizes whatever the platform stores (`CF_DIB` on
/// Windows, `image/png` on X11, an `NSImage` on macOS) into RGBA, so there is no container
/// format to sniff and no decoder to run here — the encode happens in
/// [`crate::features::image_prepare::prepare_rgba`].
pub(super) fn clipboard_image(
    slot: &mut Option<arboard::Clipboard>,
) -> Option<(u32, u32, Vec<u8>)> {
    if slot.is_none() {
        *slot = arboard::Clipboard::new().ok();
    }
    let data = slot.as_mut()?.get_image().ok()?;
    let (width, height) = (
        u32::try_from(data.width).ok()?,
        u32::try_from(data.height).ok()?,
    );
    Some((width, height, data.bytes.into_owned()))
}

/// The files a file manager put on the clipboard — "Copy" on a file in Explorer, Finder
/// or a Linux file manager (`CF_HDROP`, `NSURL`, `text/uri-list`, all read by arboard).
/// Empty for every "no files" case alike, as [`clipboard_image`] is `None` for every "no
/// image" case.
///
/// A copied picture *file* puts its path on the clipboard, not its pixels, so the paste
/// asks for files once the pixels are not there (docs/research/pasted-image-names.md A1).
pub(super) fn clipboard_files(slot: &mut Option<arboard::Clipboard>) -> Vec<PathBuf> {
    if slot.is_none() {
        *slot = arboard::Clipboard::new().ok();
    }
    slot.as_mut()
        .and_then(|c| c.get().file_list().ok())
        .unwrap_or_default()
}

/// Why a paste found no clipboard to read — not an empty one: one that would not open.
/// Every reader above folds this into "nothing there", which is right for routing and
/// wrong for the refusal: "copy one" is no advice to a person whose copy the app cannot
/// reach (docs/research/pasted-image-names.md §3.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Unreachable {
    /// A Linux session with no display server at all — neither `DISPLAY` nor
    /// `WAYLAND_DISPLAY`: a pod's web terminal, a console, a `docker exec`. There is no
    /// clipboard here to read, and the picture is on some other machine.
    NoDisplay,
    /// A Wayland session with no X server named, whose compositor answers yet would not
    /// open: it offers no data-control protocol — the one way a program without a window
    /// of its own reads the clipboard there — and there is no XWayland to fall back
    /// through. GNOME with XWayland turned off.
    NoBridge,
    /// The clipboard would not open, in the platform's words.
    Failed(String),
}

/// Opens the clipboard if the slot holds none, and says why it would not — `None` once it
/// is open. The readers above have already tried and dropped the error; asking again
/// costs nothing where it fails (no display: at once) and is only asked by a refusal.
pub(super) fn clipboard_unreachable(slot: &mut Option<arboard::Clipboard>) -> Option<Unreachable> {
    if slot.is_some() {
        return None;
    }
    match arboard::Clipboard::new() {
        Ok(c) => {
            *slot = Some(c);
            None
        }
        Err(e) if cfg!(all(unix, not(target_os = "macos"))) => Some(closed_because(
            e.to_string(),
            std::env::var_os("DISPLAY").is_some(),
            std::env::var_os("WAYLAND_DISPLAY"),
            std::env::var_os("XDG_RUNTIME_DIR"),
        )),
        Err(e) => Some(Unreachable::Failed(e.to_string())),
    }
}

/// Why a Linux clipboard would not open, read off the session's display variables.
/// arboard's error there is the X11 fallback's ("X11 server connection timed out"),
/// which names a server the person may never have had, so the session speaks first:
///
/// - no display variable at all → [`Unreachable::NoDisplay`];
/// - Wayland alone, its compositor answering on its socket → [`Unreachable::NoBridge`]:
///   the refusal was the compositor's, not a display that has gone away (a stale
///   `WAYLAND_DISPLAY` inside an old tmux);
/// - anything else → arboard's words.
fn closed_because(
    err: String,
    x11: bool,
    wayland: Option<std::ffi::OsString>,
    runtime_dir: Option<std::ffi::OsString>,
) -> Unreachable {
    if x11 {
        return Unreachable::Failed(err);
    }
    let Some(display) = wayland else {
        return Unreachable::NoDisplay;
    };
    if wayland_socket(display, runtime_dir).is_some_and(|s| answers(&s)) {
        Unreachable::NoBridge
    } else {
        Unreachable::Failed(err)
    }
}

/// Where a Wayland client connects — libwayland's rule: `WAYLAND_DISPLAY` itself when it
/// is a path, else that name under `XDG_RUNTIME_DIR`.
fn wayland_socket(
    display: std::ffi::OsString,
    runtime_dir: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let display = PathBuf::from(display);
    if display.is_absolute() {
        return Some(display);
    }
    Some(PathBuf::from(runtime_dir?).join(display))
}

/// Whether something listens on this Unix socket.
#[cfg(unix)]
fn answers(socket: &std::path::Path) -> bool {
    std::os::unix::net::UnixStream::connect(socket).is_ok()
}

#[cfg(not(unix))]
fn answers(_socket: &std::path::Path) -> bool {
    false
}

/// What a paste reads off the clipboard, in the order it asks — the seam that lets the
/// paste's routing (`input::paste_from_clipboard`) be tested without the system
/// clipboard. The one real source is the runtime's arboard slot.
pub(super) trait PasteSource {
    /// Pixels: a screenshot, a browser's *Copy image* ([`clipboard_image`]).
    fn image(&mut self) -> Option<(u32, u32, Vec<u8>)>;
    /// The paths a file manager's *Copy* put there ([`clipboard_files`]).
    fn files(&mut self) -> Vec<PathBuf>;
    /// Text, for `Ctrl+V`'s fallback ([`clipboard_text`]).
    fn text(&mut self) -> Option<String>;
    /// Whether this clipboard is on another machine than the person's ([`clipboard_is_far`]).
    fn far(&self) -> bool;
    /// Why the clipboard would not open at all ([`clipboard_unreachable`]) — asked only
    /// once everything above came back empty.
    fn unreachable(&mut self) -> Option<Unreachable>;
}

impl PasteSource for Option<arboard::Clipboard> {
    fn image(&mut self) -> Option<(u32, u32, Vec<u8>)> {
        clipboard_image(self)
    }
    fn files(&mut self) -> Vec<PathBuf> {
        clipboard_files(self)
    }
    fn text(&mut self) -> Option<String> {
        clipboard_text(self)
    }
    fn far(&self) -> bool {
        clipboard_is_far()
    }
    fn unreachable(&mut self) -> Option<Unreachable> {
        clipboard_unreachable(self)
    }
}

/// Splits copied files into the ones a paste stages — pictures, judged by the extension
/// before anything is read ([`has_image_extension`]) — and the rest, which it only names.
/// Order is kept: the files stage in the order the file manager listed them.
pub(super) fn split_copied_files(files: Vec<PathBuf>) -> (Vec<PathBuf>, Vec<PathBuf>) {
    files.into_iter().partition(|p| has_image_extension(p))
}

/// Is the clipboard this process reads on a machine other than the person's?
///
/// Over SSH it is the remote one, which on a pod has no display and no clipboard at all,
/// while the screenshot sits on the machine the person types on — no terminal protocol
/// carries an image the other way (OSC 52 is text). The exception is a forwarded display
/// (`ssh -X`, waypipe): then arboard talks to the person's own display server, and the
/// clipboard is theirs. macOS has no such forwarding for its pasteboard.
pub(super) fn clipboard_is_far() -> bool {
    let forwarded = cfg!(all(unix, not(target_os = "macos")))
        && (std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some());
    far(osc52::session_looks_remote(), forwarded)
}

/// [`clipboard_is_far`]'s rule, apart from the environment it reads.
fn far(remote: bool, forwarded_display: bool) -> bool {
    remote && !forwarded_display
}

/// Why a typed `/image paste` found nothing to stage, in words that close the door
/// (docs/lessons.md §4): each case names what the person actually did and the route
/// that works from there.
///
/// - files were copied, none of them a picture → name them, and the command that takes
///   a document;
/// - the session's clipboard is on another machine → say so, rather than asking the person
///   to copy what they already copied (A2) — first, since over SSH the clipboard also
///   fails to open and the distance is the better reason;
/// - the clipboard would not open → say that, and why ([`Unreachable`]), rather than
///   call it empty;
/// - otherwise → the clipboard is simply empty of images.
pub(super) fn nothing_to_paste(
    others: &[PathBuf],
    far: bool,
    unreachable: Option<&Unreachable>,
    loc: &'static Locale,
) -> String {
    if !others.is_empty() {
        return loc.tf(
            "ui.err.image_clipboard_not_image",
            &[("files", &file_names(others))],
        );
    }
    if far {
        return loc.t("ui.err.image_clipboard_remote").to_string();
    }
    match unreachable {
        Some(Unreachable::NoDisplay) => loc.t("ui.err.image_clipboard_no_display").to_string(),
        Some(Unreachable::NoBridge) => loc.t("ui.err.image_clipboard_no_bridge").to_string(),
        Some(Unreachable::Failed(err)) => {
            loc.tf("ui.err.image_clipboard_unreachable", &[("err", err)])
        }
        None => loc.t("ui.err.image_no_clipboard").to_string(),
    }
}

/// How many copied files a note names before it stops — the rest is an ellipsis, since a
/// selection of a hundred files must not become a hundred names in the feed.
const NAMED_FILES: usize = 3;

/// The copied files' names for a note: the last path segment of the first
/// [`NAMED_FILES`], then `…` if there were more.
pub(super) fn file_names(files: &[PathBuf]) -> String {
    let mut names: Vec<String> = files
        .iter()
        .take(NAMED_FILES)
        .map(|p| {
            p.file_name().map_or_else(
                || p.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            )
        })
        .collect();
    if files.len() > NAMED_FILES {
        names.push("…".to_string());
    }
    names.join(", ")
}

/// Writes text into the system clipboard, creating the client lazily and reusing
/// it. Returns error text (instead of panicking) if the clipboard is unavailable — on
/// headless Linux with no X11/Wayland, the `arboard` constructor can fail.
fn write_local(slot: &mut Option<arboard::Clipboard>, text: &str) -> Result<(), String> {
    if slot.is_none() {
        *slot = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
    }
    // `unwrap` is safe: we just guaranteed `Some`.
    slot.as_mut()
        .unwrap()
        .set_text(text.to_string())
        .map_err(|e| e.to_string())
}

/// What became of the terminal's half of a copy (OSC 52).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TerminalCopy {
    /// Not attempted — a local session with a working clipboard, or the mode is
    /// `off`. The overwhelmingly common case, and the silent one.
    NotTried,
    /// The sequence went out. The protocol answers nothing, so this is the most
    /// that can honestly be said.
    Sent,
    /// The text is past what an OSC 52 sequence can carry, so none was written
    /// (fork F3): a silently truncated conversation looks complete, which is the
    /// worse failure.
    TooLarge { bytes: usize },
}

/// The outcome of one copy: what the local clipboard did, and what the terminal
/// was told. Both halves travel together because the note depends on the pair —
/// "copied" is only honest when the local clipboard took it and nothing else was
/// needed (spec §11.7, fork F4).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct CopyReport {
    pub(super) local: Result<(), String>,
    pub(super) terminal: TerminalCopy,
}

impl CopyReport {
    /// The localized note (and whether it is an error), for whichever surface is
    /// showing it. One place, so the chat feed and the chat list's status area
    /// cannot word the same outcome differently.
    pub(super) fn message(&self, loc: &'static Locale) -> (String, bool) {
        match (&self.local, self.terminal) {
            // The ordinary local copy — unchanged wording.
            (Ok(()), TerminalCopy::NotTried) => (loc.t("ui.chat.copied").into(), false),
            // The sequence went out. Whether the local clipboard also took it is
            // not worth a sentence: over SSH that clipboard is on the wrong
            // machine anyway, and the user asked for the text where they type.
            (_, TerminalCopy::Sent) => (loc.t("ui.chat.copied_terminal").into(), false),
            // Too large for the terminal: say so, and say where the text did go
            // — or that it went nowhere, which is a different next step.
            (Ok(()), TerminalCopy::TooLarge { bytes }) => (
                loc.tf(
                    "ui.chat.copied_local_too_large",
                    &[("bytes", &bytes.to_string())],
                ),
                false,
            ),
            (Err(err), TerminalCopy::TooLarge { bytes }) => (
                loc.tf(
                    "ui.err.copy_too_large",
                    &[("bytes", &bytes.to_string()), ("err", err)],
                ),
                true,
            ),
            (Err(err), TerminalCopy::NotTried) => {
                (loc.tf("ui.err.copy_failed", &[("err", err)]), true)
            }
        }
    }
}

/// What the terminal half of a copy should be, decided **before** doing it.
///
/// Split out of [`copy_text`] so the rule is testable: the executing half needs
/// a real clipboard and a real stdout, and a mutation of the ceiling check
/// survived every test while it lived in there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalPlan {
    /// Leave the terminal alone.
    Skip,
    /// Past what a sequence can carry — report rather than send.
    TooLarge,
    /// Write the sequence.
    Send,
}

fn plan_terminal(mode: Osc52Mode, remote: bool, local_failed: bool, len: usize) -> TerminalPlan {
    if !osc52::should_emit(mode, remote, local_failed) {
        TerminalPlan::Skip
    } else if len > osc52::MAX_TEXT_BYTES {
        TerminalPlan::TooLarge
    } else {
        TerminalPlan::Send
    }
}

/// Copies `text`: the local clipboard first, then — per the mode and the session
/// (`osc52::should_emit`) — the terminal's own clipboard over OSC 52.
///
/// The local write comes first on purpose even when the session looks remote:
/// it costs nothing when it works, and its failure is one of the two signals
/// that the terminal's clipboard is the one that matters.
pub(super) fn copy_text(
    slot: &mut Option<arboard::Clipboard>,
    text: &str,
    mode: Osc52Mode,
) -> CopyReport {
    let local = write_local(slot, text);
    let plan = plan_terminal(
        mode,
        osc52::session_looks_remote(),
        local.is_err(),
        text.len(),
    );
    let terminal = match plan {
        TerminalPlan::Skip => TerminalCopy::NotTried,
        TerminalPlan::TooLarge => TerminalCopy::TooLarge { bytes: text.len() },
        // Straight to stdout, between frames: the same place and the same moment
        // the app already writes `SetTitle` and the mouse-capture toggles. A
        // write failure is reported as "not tried" rather than as a third error
        // — there is nothing the user could do about a broken stdout that they
        // are not already finding out about.
        TerminalPlan::Send => match osc52::write_to(&mut io::stdout(), text, osc52::in_tmux()) {
            Ok(true) => TerminalCopy::Sent,
            Ok(false) | Err(_) => TerminalCopy::NotTried,
        },
    };
    CopyReport { local, terminal }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule the executing half obeys, over the whole matrix: nothing is
    /// written locally, the two signals turn it on, and the ceiling turns it
    /// back off — reported rather than sent, so an oversize copy cannot arrive
    /// half-length (fork F3). The ceiling is checked at the boundary, because a
    /// mutation that skipped it entirely once survived every other test here.
    #[test]
    fn the_terminal_half_is_planned_before_it_is_done() {
        use Osc52Mode::*;
        use TerminalPlan::*;
        let max = crate::shared::osc52::MAX_TEXT_BYTES;
        for (mode, remote, failed, len, expected) in [
            // Auto: silent locally, on for either signal.
            (Auto, false, false, 10, Skip),
            (Auto, true, false, 10, Send),
            (Auto, false, true, 10, Send),
            // Always and off override both signals.
            (Always, false, false, 10, Send),
            (Off, true, true, 10, Skip),
            // The ceiling, exactly.
            (Always, false, false, max, Send),
            (Always, false, false, max + 1, TooLarge),
            // …and it never turns a skip into a report: a local session hears
            // nothing about a limit that was never going to apply.
            (Auto, false, false, max + 1, Skip),
            (Off, true, true, max + 1, Skip),
        ] {
            assert_eq!(
                plan_terminal(mode, remote, failed, len),
                expected,
                "{mode:?} remote={remote} local_failed={failed} len={len}"
            );
        }
    }

    /// The wording rule (fork F4): the three outcomes are three different
    /// sentences, and only the plain local copy keeps the old one. A single
    /// "copied" for all of them would claim a delivery OSC 52 cannot confirm.
    #[test]
    fn each_outcome_gets_its_own_wording() {
        let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::En);
        let msg = |local: Result<(), String>, terminal| CopyReport { local, terminal }.message(loc);

        let (plain, failed) = msg(Ok(()), TerminalCopy::NotTried);
        assert!(!failed);
        assert_eq!(
            plain,
            loc.t("ui.chat.copied"),
            "the unchanged local wording"
        );

        let (sent, failed) = msg(Ok(()), TerminalCopy::Sent);
        assert!(!failed);
        assert_ne!(sent, plain, "sending to the terminal is not the same claim");
        // It must not promise arrival — the protocol says nothing back.
        assert!(sent.contains("never confirms"), "{sent}");

        // Too large, with and without a local clipboard to fall back on: two
        // different next steps, so two different sentences.
        let (big_local, failed) = msg(Ok(()), TerminalCopy::TooLarge { bytes: 100_000 });
        assert!(!failed, "the text did reach this machine's clipboard");
        let (big_none, failed) = msg(
            Err("no clipboard".into()),
            TerminalCopy::TooLarge { bytes: 100_000 },
        );
        assert!(failed, "nothing was copied anywhere");
        assert_ne!(big_local, big_none);
        for m in [&big_local, &big_none] {
            assert!(m.contains("100000"), "the size is named: {m}");
        }
        assert!(big_none.contains("no clipboard"), "{big_none}");

        // And the pre-existing failure keeps its own wording.
        let (err, failed) = msg(Err("no clipboard".into()), TerminalCopy::NotTried);
        assert!(failed);
        assert_ne!(err, big_none);
    }

    /// Every outcome where the clipboard may not have delivered names the route
    /// that always works (`/export`) — in every bundled locale.
    ///
    /// This is the project's most-repeated defect, caught live *inside* this
    /// feature: the OSC 52 note told a JupyterLab user their terminal "may not
    /// support the sequence" and stopped there, which is a description of the
    /// situation with no way out of it (docs/lessons.md §4). JupyterLab is named
    /// too — it is the one host we know drops the escape, and being told which
    /// terminal you are fighting is worth a clause.
    #[test]
    fn every_clipboard_dead_end_names_the_export_route() {
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            for (local, terminal) in [
                (Ok(()), TerminalCopy::Sent),
                (Ok(()), TerminalCopy::TooLarge { bytes: 90_000 }),
                (
                    Err("x".to_string()),
                    TerminalCopy::TooLarge { bytes: 90_000 },
                ),
            ] {
                let (msg, _) = CopyReport { local, terminal }.message(loc);
                assert!(
                    msg.contains("/export"),
                    "{lang:?} leaves the user without a route: {msg}"
                );
            }
            // The control: an ordinary local copy worked, so it has nothing to
            // send anyone to — a route there would be noise on every `F5`.
            let (plain, _) = CopyReport {
                local: Ok(()),
                terminal: TerminalCopy::NotTried,
            }
            .message(loc);
            assert!(!plain.contains("/export"), "{lang:?}: {plain}");
        }
    }

    /// Per-locale gate (docs/history/i18n-ui.md §3.5): every outcome renders in
    /// every bundled language with no unsubstituted placeholder, and the `en`
    /// text carries no Cyrillic from the `ru` default. Each of the three new
    /// wordings must also name what to do next (docs/lessons.md §4) — for the
    /// two that report a limit, that is the limit itself.
    #[test]
    fn the_wordings_are_localized_for_all_langs() {
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            for (local, terminal) in [
                (Ok(()), TerminalCopy::NotTried),
                (Ok(()), TerminalCopy::Sent),
                (Ok(()), TerminalCopy::TooLarge { bytes: 90_000 }),
                (
                    Err("x".to_string()),
                    TerminalCopy::TooLarge { bytes: 90_000 },
                ),
                (Err("x".to_string()), TerminalCopy::NotTried),
            ] {
                let (msg, _) = CopyReport { local, terminal }.message(loc);
                assert!(
                    !msg.contains('{') && !msg.contains('}'),
                    "unsubstituted placeholder in {lang:?}: {msg}"
                );
                assert!(!msg.trim().is_empty(), "an empty answer in {lang:?}");
                if lang == crate::shared::i18n::Lang::En {
                    assert!(
                        !msg.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)),
                        "Cyrillic leaked into the en message: {msg}"
                    );
                }
            }
            // The ceiling is quoted in both size messages, so a user can tell
            // how far over they are.
            let (msg, _) = CopyReport {
                local: Ok(()),
                terminal: TerminalCopy::TooLarge { bytes: 90_000 },
            }
            .message(loc);
            assert!(
                msg.contains(&crate::shared::osc52::MAX_TEXT_BYTES.to_string()),
                "the limit is named in {lang:?}: {msg}"
            );
        }
    }

    /// Over SSH the clipboard is the remote machine's — unless a display was forwarded,
    /// which makes it the person's again. A local session is never "far".
    #[test]
    fn the_clipboard_is_far_only_over_ssh_without_a_forwarded_display() {
        assert!(
            far(true, false),
            "ssh, no display: the remote machine's clipboard"
        );
        assert!(!far(true, true), "ssh -X: the person's own display server");
        assert!(!far(false, false), "a local session");
        assert!(!far(false, true), "a local desktop session");
    }

    /// The system clipboard read through the paste's seam, **read-only**: every question
    /// answers without failing, the distance is [`clipboard_is_far`]'s, and a clipboard
    /// that cannot be opened at all — a headless box, CI — reads as empty rather than as
    /// an error, which is what lets a paste fall through to its refusal.
    #[test]
    fn the_system_clipboard_answers_the_paste_and_reads_empty_when_absent() {
        let absent = arboard::Clipboard::new().is_err();
        let mut slot: Option<arboard::Clipboard> = None;
        let image = PasteSource::image(&mut slot);
        let files = PasteSource::files(&mut slot);
        let text = PasteSource::text(&mut slot);
        assert_eq!(PasteSource::far(&slot), clipboard_is_far());
        if absent {
            assert!(image.is_none() && files.is_empty() && text.is_none());
        }
        // A clipboard that would not open says so — and one that opened has no reason.
        let unreachable = PasteSource::unreachable(&mut slot);
        assert_eq!(unreachable.is_some(), absent, "{unreachable:?}");
        if let Some(Unreachable::Failed(err)) = &unreachable {
            assert!(!err.is_empty(), "the platform's words, not a blank");
        }
    }

    /// libwayland's rule for where a client connects: the variable itself when it is a
    /// path, else the name under the runtime directory.
    #[test]
    fn the_wayland_socket_is_found_where_libwayland_looks() {
        use std::ffi::OsString;
        let run = std::env::temp_dir();
        assert_eq!(
            wayland_socket("wayland-1".into(), Some(run.clone().into_os_string())),
            Some(run.join("wayland-1"))
        );
        let absolute = run.join("elsewhere").join("wayland-9");
        assert_eq!(
            wayland_socket(absolute.clone().into_os_string(), None),
            Some(absolute),
            "an absolute name needs no runtime directory"
        );
        assert_eq!(wayland_socket(OsString::from("wayland-1"), None), None);
    }

    /// The session speaks before arboard's error, which on Linux is always the X11
    /// fallback's: no display variable at all is a session with no clipboard; an X
    /// server named, or a Wayland one nobody answers on, leaves arboard's words.
    #[test]
    fn a_closed_clipboard_is_explained_by_the_session_first() {
        let err = || "X11 server connection timed out".to_string();
        let failed = Unreachable::Failed(err());
        let run = Some(std::env::temp_dir().into_os_string());
        assert_eq!(
            closed_because(err(), false, None, None),
            Unreachable::NoDisplay
        );
        assert_eq!(
            closed_because(err(), true, None, None),
            failed,
            "X named: its words"
        );
        let stale = Some("mindfork-no-such-wayland-socket".into());
        assert_eq!(
            closed_because(err(), false, stale.clone(), run.clone()),
            failed,
            "nobody answers: a display that went away, not GNOME"
        );
        assert_eq!(closed_because(err(), true, stale, run), failed);
    }

    /// A compositor is told from a stale variable by whether its socket answers: a
    /// listener does, a path with nobody behind it does not.
    #[cfg(unix)]
    #[test]
    fn a_socket_answers_only_while_something_listens() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("wayland-0");
        assert!(!answers(&socket), "no file at all");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        assert!(answers(&socket), "a compositor is listening");
        // ...and with nothing but Wayland named, its answer makes the refusal GNOME's.
        let run = Some(dir.path().as_os_str().to_owned());
        let named = || Some(std::ffi::OsString::from("wayland-0"));
        assert_eq!(
            closed_because("e".into(), false, named(), run.clone()),
            Unreachable::NoBridge
        );
        assert_eq!(
            closed_because("e".into(), true, named(), run),
            Unreachable::Failed("e".into()),
            "with XWayland named the X11 error is the one that counts"
        );
        drop(listener);
        assert!(!answers(&socket), "the file outlived its compositor");
    }

    /// Pictures are staged, everything else only named — and in the order the file
    /// manager listed them, since that is the order the person sees them land.
    #[test]
    fn copied_files_split_into_pictures_and_the_rest_keeping_their_order() {
        let files: Vec<PathBuf> = ["b.png", "letter.txt", "a.JPG", "dir", "c.webp"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let (images, others) = split_copied_files(files);
        assert_eq!(
            images,
            ["b.png", "a.JPG", "c.webp"].map(PathBuf::from).to_vec()
        );
        assert_eq!(others, ["letter.txt", "dir"].map(PathBuf::from).to_vec());
    }

    /// A note names the files by their last segment, and stops after three: a selection
    /// of a hundred must not become a hundred names in the feed.
    #[test]
    fn file_names_are_the_last_segments_and_stop_after_three() {
        let one = [PathBuf::from("/home/u/Pictures/letter.txt")];
        assert_eq!(file_names(&one), "letter.txt");
        let four: Vec<PathBuf> = ["a.txt", "b.txt", "c.txt", "d.txt"]
            .iter()
            .map(|n| PathBuf::from("dir").join(n))
            .collect();
        assert_eq!(file_names(&four), "a.txt, b.txt, c.txt, …");
        let three = &four[..3];
        assert_eq!(file_names(three), "a.txt, b.txt, c.txt");
    }

    /// Each reason a typed `/image paste` finds nothing gets its own words, in both
    /// locales — and each names the route that works from there (docs/lessons.md §4).
    #[test]
    fn nothing_to_paste_says_why_and_names_the_route_that_works() {
        use crate::shared::i18n::{Lang, locale};
        let docs = [PathBuf::from("report.docx")];
        for lang in [Lang::En, Lang::Ru] {
            let loc = locale(lang);
            // Files, none a picture — named, and pointed at the command for documents.
            let msg = nothing_to_paste(&docs, true, Some(&Unreachable::NoBridge), loc);
            assert_eq!(
                msg,
                loc.tf(
                    "ui.err.image_clipboard_not_image",
                    &[("files", "report.docx")]
                ),
                "copied files win over the distance in {lang:?}"
            );
            assert!(msg.contains("/file attach"), "{lang:?}: {msg}");
            // Nothing at all, over SSH — the distance, and the two routes around it, even
            // though the clipboard there would not open either.
            let failed = Unreachable::Failed("no display".into());
            for unreachable in [None, Some(&failed)] {
                let msg = nothing_to_paste(&[], true, unreachable, loc);
                assert_eq!(msg, loc.t("ui.err.image_clipboard_remote"), "{lang:?}");
                assert!(msg.contains("SSH"), "{lang:?}: {msg}");
                assert!(msg.contains("/image attach <url>"), "{lang:?}: {msg}");
            }
            // A clipboard that would not open is not called empty: a Wayland desktop that
            // shares it with no windowless program is named, any other failure quoted.
            let msg = nothing_to_paste(&[], false, Some(&Unreachable::NoBridge), loc);
            assert_eq!(msg, loc.t("ui.err.image_clipboard_no_bridge"), "{lang:?}");
            assert!(
                msg.contains("XWayland") && msg.contains("GNOME"),
                "{lang:?}: {msg}"
            );
            let msg = nothing_to_paste(&[], false, Some(&failed), loc);
            assert_eq!(
                msg,
                loc.tf(
                    "ui.err.image_clipboard_unreachable",
                    &[("err", "no display")]
                ),
                "{lang:?}"
            );
            assert!(msg.contains("(no display)"), "{lang:?}: {msg}");
            // No display at all — a pod's web terminal: no clipboard, and the routes that
            // bring a picture from elsewhere.
            let msg = nothing_to_paste(&[], false, Some(&Unreachable::NoDisplay), loc);
            assert_eq!(msg, loc.t("ui.err.image_clipboard_no_display"), "{lang:?}");
            assert!(
                msg.contains("DISPLAY") && msg.contains("WAYLAND_DISPLAY"),
                "{msg}"
            );
            assert!(msg.contains("/image attach <url>"), "{lang:?}: {msg}");
            for msg in [
                nothing_to_paste(&[], false, Some(&Unreachable::NoDisplay), loc),
                nothing_to_paste(&[], false, Some(&Unreachable::NoBridge), loc),
                nothing_to_paste(&[], false, Some(&failed), loc),
            ] {
                assert!(
                    msg.contains("/image attach <"),
                    "the route that works: {msg}"
                );
                assert_ne!(msg, loc.t("ui.err.image_no_clipboard"), "{lang:?}");
            }
            // Nothing at all, locally, the clipboard open — today's message, unchanged.
            let msg = nothing_to_paste(&[], false, None, loc);
            assert_eq!(msg, loc.t("ui.err.image_no_clipboard"), "{lang:?}");
        }
    }
}

#[cfg(test)]
#[path = "clipboard/tests/live.rs"]
mod live_tests;

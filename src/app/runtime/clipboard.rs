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
        assert!(sent.contains("does not confirm"), "{sent}");

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

    /// A real round trip through the **operating system's** clipboard: put an image on
    /// it, read it back, and encode it the way a paste would.
    ///
    /// `#[ignore]` because it touches a shared global resource — it would clobber whatever
    /// the developer had copied, and on a headless CI box there is no clipboard at all.
    /// It exists because everything else about this path is a unit test against pixels we
    /// made up: only a real clipboard can show that arboard's `image-data` feature is
    /// actually wired on this platform, that the bytes really come back RGBA in the size
    /// reported, and that the round trip survives the platform's own format conversion
    /// (`CF_DIB` on Windows, `image/png` on X11).
    ///
    /// Run: `cargo test clipboard_image_round_trip -- --ignored --nocapture`.
    #[test]
    #[ignore = "uses the real system clipboard (clobbers what the user copied)"]
    fn clipboard_image_round_trip() {
        let (w, h) = (64u32, 32u32);
        // Distinct per-pixel values, so a stride or channel-order mistake shows up as a
        // mismatch rather than as a plausible-looking picture.
        let source: Vec<u8> = (0..w as usize * h as usize)
            .flat_map(|i| [(i % 251) as u8, (i % 253) as u8, (i % 257 % 256) as u8, 255])
            .collect();

        let mut slot: Option<arboard::Clipboard> = None;
        if slot.is_none() {
            match arboard::Clipboard::new() {
                Ok(c) => slot = Some(c),
                Err(e) => {
                    eprintln!("skip: no system clipboard here ({e})");
                    return;
                }
            }
        }
        slot.as_mut()
            .unwrap()
            .set_image(arboard::ImageData {
                width: w as usize,
                height: h as usize,
                bytes: source.clone().into(),
            })
            .expect("putting an image on the clipboard");

        let (rw, rh, rgba) = clipboard_image(&mut slot).expect("an image back off the clipboard");
        assert_eq!((rw, rh), (w, h), "the size must survive the round trip");
        assert_eq!(rgba.len(), source.len(), "RGBA8, four bytes per pixel");
        // Not asserting byte equality: a platform may composite onto an opaque background
        // or reorder channels internally. What must hold is that the pixels are *ours* —
        // an all-black or all-white buffer would mean the image never made it.
        let distinct = rgba
            .chunks(4)
            .map(|p| (p[0], p[1], p[2]))
            .collect::<std::collections::HashSet<_>>();
        assert!(
            distinct.len() > 100,
            "the round trip flattened the image into {} distinct colours",
            distinct.len()
        );

        // And the encode a paste would do accepts what came back.
        let prepared =
            crate::features::image_prepare::prepare_rgba(rw, rh, &rgba, 1568).expect("encoding");
        assert_eq!(prepared.mime, "image/png");
        assert_eq!((prepared.width, prepared.height), (w, h));
        eprintln!(
            "clipboard round trip: {w}x{h} -> {} bytes png",
            prepared.bytes.len()
        );
    }
}

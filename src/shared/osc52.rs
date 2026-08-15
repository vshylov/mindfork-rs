//! OSC 52 — handing copied text to the **client's** clipboard.
//!
//! `arboard` writes the clipboard of the machine the *process* runs on. Over
//! SSH that is the wrong machine, and on a headless box it is no machine at all
//! (the constructor fails, and the copy reports an error). OSC 52 travels the
//! same pipe the drawing does, so the text reaches the terminal the user is
//! actually sitting in front of. See
//! [docs/research/osc52-clipboard.md](../../docs/research/osc52-clipboard.md).
//!
//! Everything here is pure but for [`session_looks_remote`] and [`in_tmux`],
//! which read the environment: the sequence is built and handed back, and the
//! caller writes it. That is what lets the tests assert the exact bytes.
//!
//! **The protocol gives nothing back.** There is no acknowledgement, and no way
//! to ask whether the terminal supports the sequence at all — the query form of
//! OSC 52 *is* the clipboard-read path that terminals disable as a leak vector.
//! So a caller may say it *sent* the text; it may not say the text arrived.

use std::io::{self, Write};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

/// The largest text an OSC 52 sequence can carry, in bytes.
///
/// The sequence itself is capped at 100 000 bytes in the common
/// implementations; `\e]52;c;` and the terminator take 8 of them, and base64
/// costs four bytes per three. Real terminals are stricter — kitty rejected
/// over 6 138 bytes, tabby fails around 1 KB — but those are their bugs to fix,
/// and truncating everyone's copy to the worst of them would be worse than
/// letting a large copy fall back to the local clipboard.
pub const MAX_TEXT_BYTES: usize = 74_994;

/// Whether OSC 52 is used, and when (`interface.clipboard_osc52`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Osc52Mode {
    /// Emit when the session looks remote, or when the local clipboard failed —
    /// locally nothing changes and no escape is written. The default.
    #[default]
    Auto,
    /// Emit on every copy. For a remote session the environment does not
    /// advertise (a container, a serial console, a terminal multiplexer that
    /// hides the SSH variables).
    Always,
    /// Never emit. The escape hatch for a terminal that renders an unknown OSC
    /// as text instead of ignoring it.
    Off,
}

/// Whether to hand this copy to the terminal as well as to the local clipboard.
///
/// `remote` is the environment's opinion ([`session_looks_remote`]),
/// `local_failed` the local clipboard's. Pure, so the whole matrix is a table
/// test.
pub fn should_emit(mode: Osc52Mode, remote: bool, local_failed: bool) -> bool {
    match mode {
        Osc52Mode::Off => false,
        Osc52Mode::Always => true,
        // A local clipboard that failed is a certainty, where `remote` is a
        // guess — either is reason enough, and neither is reason to skip the
        // local write first.
        Osc52Mode::Auto => remote || local_failed,
    }
}

/// Does this look like a session whose terminal lives on another machine?
///
/// `SSH_TTY`/`SSH_CONNECTION` are set by `sshd` for an interactive session, and
/// VS Code's Remote-SSH inherits them. A container or a web terminal sets
/// neither — hence [`Osc52Mode::Always`].
pub fn session_looks_remote() -> bool {
    std::env::var_os("SSH_TTY").is_some() || std::env::var_os("SSH_CONNECTION").is_some()
}

/// Is this session inside tmux? Its own escape has to be wrapped for it
/// ([`sequence`]).
pub fn in_tmux() -> bool {
    std::env::var_os("TMUX").is_some()
}

/// Builds the OSC 52 sequence for `text`, or `None` when the text is past
/// [`MAX_TEXT_BYTES`] — the caller then says so rather than sending something
/// the terminal will drop or, worse, print.
///
/// With `tmux` set, the sequence is wrapped in DCS passthrough so tmux forwards
/// it to the outer terminal instead of eating it; the inner `\e` is doubled, as
/// that wrapper requires. `screen` needs a different wrapper *and* 768-byte
/// chunking and is deliberately not supported — it degrades to "no OSC 52",
/// which is exactly today's behaviour.
pub fn sequence(text: &str, tmux: bool) -> Option<String> {
    if text.len() > MAX_TEXT_BYTES {
        return None;
    }
    // `c` is the clipboard proper; `p` would be X11's primary selection, which
    // is not what a copy means to most people.
    let inner = format!("\x1b]52;c;{}\x07", BASE64.encode(text));
    Some(if tmux {
        format!("\x1bPtmux;{}\x1b\\", inner.replace('\x1b', "\x1b\x1b"))
    } else {
        inner
    })
}

/// Writes the sequence for `text` to `out`. `Ok(false)` — nothing was written
/// because the text is too large ([`sequence`]).
///
/// Takes a sink rather than reaching for `stdout` so the bytes can be asserted
/// in a test, and so a caller that must not interleave with the drawing can
/// choose when to flush.
pub fn write_to<W: Write>(out: &mut W, text: &str, tmux: bool) -> io::Result<bool> {
    let Some(seq) = sequence(text, tmux) else {
        return Ok(false);
    };
    out.write_all(seq.as_bytes())?;
    out.flush()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact bytes, for the three shapes that matter: ASCII, multi-byte
    /// UTF-8 (base64 must encode the *bytes*, not the characters) and empty.
    #[test]
    fn the_sequence_is_the_documented_bytes() {
        assert_eq!(
            sequence("hi", false).unwrap(),
            "\x1b]52;c;aGk=\x07",
            "ASCII"
        );
        // A two-character Cyrillic word is four UTF-8 bytes; base64 must encode
        // those bytes, not the two characters.
        assert_eq!(
            sequence("да", false).unwrap(),
            format!("\x1b]52;c;{}\x07", BASE64.encode("да".as_bytes()))
        );
        // Empty is a legitimate copy (it clears the clipboard) and must not be
        // special-cased into something malformed.
        assert_eq!(sequence("", false).unwrap(), "\x1b]52;c;\x07");
    }

    /// Inside tmux the sequence is wrapped, and every inner escape is doubled —
    /// a wrapper that forgot the doubling would end the passthrough early and
    /// spray the payload onto the screen.
    #[test]
    fn tmux_wraps_and_doubles_the_escapes() {
        let wrapped = sequence("hi", true).unwrap();
        assert_eq!(wrapped, "\x1bPtmux;\x1b\x1b]52;c;aGk=\x07\x1b\\");
        assert!(wrapped.starts_with("\x1bPtmux;"));
        assert!(wrapped.ends_with("\x1b\\"));
        // The payload survives the wrapping intact.
        assert!(wrapped.contains("aGk="));
        // …and the unwrapped form is genuinely different, so this test cannot
        // pass with the wrapper removed.
        assert_ne!(wrapped, sequence("hi", false).unwrap());
    }

    /// The ceiling is honoured at the boundary, not near it.
    #[test]
    fn the_ceiling_is_exact() {
        let fits = "x".repeat(MAX_TEXT_BYTES);
        assert!(
            sequence(&fits, false).is_some(),
            "the largest text that fits"
        );
        let over = "x".repeat(MAX_TEXT_BYTES + 1);
        assert!(sequence(&over, false).is_none(), "one byte more");
        // Counted in **bytes**, not characters: a two-byte character halves the
        // number of characters that fit.
        let cyrillic = "я".repeat(MAX_TEXT_BYTES / 2 + 1);
        assert!(
            sequence(&cyrillic, false).is_none(),
            "{} characters is {} bytes",
            cyrillic.chars().count(),
            cyrillic.len()
        );
    }

    /// `write_to` reports what it did and writes nothing at all when it refuses
    /// — the assertion behind "an oversize copy leaves the terminal alone".
    #[test]
    fn write_to_reports_and_stays_silent_when_it_refuses() {
        let mut sink = Vec::new();
        assert!(write_to(&mut sink, "hi", false).unwrap());
        assert_eq!(sink, b"\x1b]52;c;aGk=\x07");

        let mut sink = Vec::new();
        let over = "x".repeat(MAX_TEXT_BYTES + 1);
        assert!(!write_to(&mut sink, &over, false).unwrap());
        assert!(sink.is_empty(), "a refused copy must write no bytes");
    }

    /// The whole decision matrix (fork F1a): locally nothing is written, a
    /// remote session or a failed local clipboard turns it on, and the two
    /// explicit modes override both signals.
    #[test]
    fn the_decision_matrix() {
        use Osc52Mode::*;
        for (mode, remote, failed, expected) in [
            (Auto, false, false, false),
            (Auto, true, false, true),
            (Auto, false, true, true),
            (Auto, true, true, true),
            (Always, false, false, true),
            (Always, true, true, true),
            (Off, true, true, false),
            (Off, false, false, false),
        ] {
            assert_eq!(
                should_emit(mode, remote, failed),
                expected,
                "{mode:?} remote={remote} local_failed={failed}"
            );
        }
    }

    /// The default is the conservative one: an ordinary local session writes no
    /// escape until something says otherwise.
    #[test]
    fn the_default_mode_is_auto() {
        assert_eq!(Osc52Mode::default(), Osc52Mode::Auto);
        assert!(!should_emit(Osc52Mode::default(), false, false));
    }
}

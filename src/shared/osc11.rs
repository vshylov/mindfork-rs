//! OSC 11 — asking the terminal what its background colour is.
//!
//! `Theme::Auto` (spec §11.6) is the default and claims to follow the terminal.
//! It cannot do that without asking, and the only portable way to ask is to
//! write `ESC ] 11 ; ? BEL` and read back `ESC ] 11 ; rgb:…`. See
//! [docs/terminal-background-detection.md](../../docs/terminal-background-detection.md)
//! for the plan and the measured host matrix.
//!
//! The split mirrors [`crate::shared::osc52`]: everything above the
//! [`exchange`] section is **pure** — the query is a constant, the reply is
//! parsed from bytes, the verdict is arithmetic — so the whole corpus of
//! replies recorded from real terminals is testable with no terminal at all.
//! The IO half is deliberately small and platform-split.
//!
//! **What the measurements forced** (research doc §5):
//!
//! - The reply's terminator has to be read as **either**. Every host but tmux
//!   answers with ST although the query asks with BEL; tmux answers with BEL.
//!   Accepting one and not the other loses either tmux or everything else.
//! - Local terminals answer in 15–31 ms, but JupyterLab needs **382 ms cold**
//!   because the reply round-trips over a websocket to a browser. Hence the
//!   two-phase unix path: ask early, collect late, and let startup cover it.
//! - Legacy conhost never answers, and no console mode changes that — so the
//!   silent case is normal operation, not an error to report.
//! - The query is sent **unwrapped** even inside tmux — and that is what makes
//!   tmux work, not a limitation tolerated. Multiplexer passthrough is
//!   output-only: it carries the query out, but the reply returns on the
//!   multiplexer's own input and is consumed there, so a wrapped query measures
//!   the wrapper. Asked plainly, tmux answers for itself in 0.1-0.3 ms.

use std::io::IsTerminal;
use std::time::Duration;

/// `ESC ] 11 ; ? BEL` — "report the background colour".
///
/// BEL rather than ST as the *query* terminator: it is the more widely accepted
/// of the two, and the reply's own terminator is read back either way.
pub const QUERY_BG: &str = "\x1b]11;?\x07";

/// Relative-luminance boundary between a dark and a light background.
///
/// The measured backgrounds sit nowhere near it — `#0c0c0c` is 0.0037,
/// `#191a1b` is 0.0102, `#ffffff` is 1.0 — so the midpoint is not a tuned
/// constant, it is simply the obvious place to cut.
pub const DARK_THRESHOLD: f32 = 0.5;

/// How long the reply is worth waiting for, per platform.
///
/// unix covers JupyterLab's 382 ms cold measurement with margin, and pays
/// almost none of it in practice: the wait is overlapped with opening storage
/// ([`Pending::harvest`]). Windows does the exchange in one go before the UI
/// exists, so its budget is the startup cost on a terminal that stays silent —
/// affordable because the slow host is not a Windows host and the ones that
/// are answer in 16–31 ms.
pub const BUDGET: Duration = if cfg!(windows) {
    Duration::from_millis(150)
} else {
    Duration::from_millis(500)
};

/// An 8-bit-per-channel colour parsed out of a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Which way the terminal's background goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Background {
    Dark,
    Light,
}

/// `MINDFORK_TERMINAL_BG` — the escape hatch (design plan §5, F3).
///
/// A settings row was deliberately not added: picking `Dark` or `Light` in the
/// settings screen is already the user-facing opt-out, and this exists for
/// tests, for the container, and for a terminal that answers wrongly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvOverride {
    /// Force the outcome, skipping the query entirely.
    Force(Background),
    /// Do not query at all; fall back as though nothing answered.
    Off,
}

/// Reads `MINDFORK_TERMINAL_BG`. Unrecognised values are ignored (and logged),
/// so a typo degrades to normal detection rather than to a broken theme.
pub fn env_override() -> Option<EnvOverride> {
    let raw = std::env::var("MINDFORK_TERMINAL_BG").ok()?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "dark" => Some(EnvOverride::Force(Background::Dark)),
        "light" => Some(EnvOverride::Force(Background::Light)),
        "off" | "none" => Some(EnvOverride::Off),
        other => {
            tracing::warn!(
                value = %other,
                "MINDFORK_TERMINAL_BG is not one of dark/light/off — ignoring it"
            );
            None
        }
    }
}

/// Extracts the colour from an OSC 10/11 reply body.
///
/// Accepts both shapes terminals answer with: X11's `rgb:R/G/B` with **one to
/// four** hex digits per component, and the `#rrggbb` form. Components are
/// scaled by their own width, so `f`, `ff` and `ffff` are all full-scale — the
/// scaling X11 itself specifies, and the reason `rgb:f/f/f` is white rather
/// than near-black.
pub fn parse_reply(raw: &[u8]) -> Option<Rgb> {
    let text = std::str::from_utf8(raw).ok()?;
    if let Some(at) = text.find("rgb:") {
        let body = &text[at + 4..];
        let mut parts = body.split('/');
        let r = scale(parts.next()?)?;
        let g = scale(parts.next()?)?;
        // The last component runs up to the terminator, which is still attached.
        let b = scale(trim_terminator(parts.next()?))?;
        return Some(Rgb { r, g, b });
    }
    if let Some(at) = text.find('#') {
        let hex = trim_terminator(&text[at + 1..]);
        if hex.len() == 6 && hex.bytes().all(|c| c.is_ascii_hexdigit()) {
            let v = u32::from_str_radix(hex, 16).ok()?;
            return Some(Rgb {
                r: ((v >> 16) & 0xff) as u8,
                g: ((v >> 8) & 0xff) as u8,
                b: (v & 0xff) as u8,
            });
        }
    }
    None
}

/// Cuts the reply's terminator (BEL, or ST as `ESC \`) off a component.
fn trim_terminator(s: &str) -> &str {
    let end = s
        .find(['\x07', '\x1b'])
        .unwrap_or_else(|| s.trim_end().len().min(s.len()));
    &s[..end]
}

/// One `rgb:` component, scaled from its own width to 8 bits.
fn scale(part: &str) -> Option<u8> {
    let part = part.trim();
    if part.is_empty() || part.len() > 4 || !part.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(part, 16).ok()?;
    let max = (1u32 << (part.len() * 4)) - 1;
    Some(((value * 255 + max / 2) / max) as u8)
}

/// WCAG relative luminance (sRGB linearised). 0 is black, 1 is white.
pub fn relative_luminance(rgb: Rgb) -> f32 {
    fn channel(c: u8) -> f32 {
        let c = f32::from(c) / 255.0;
        if c <= 0.040_45 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(rgb.r) + 0.7152 * channel(rgb.g) + 0.0722 * channel(rgb.b)
}

/// Which side of [`DARK_THRESHOLD`] a colour falls on.
pub fn classify(rgb: Rgb) -> Background {
    if relative_luminance(rgb) < DARK_THRESHOLD {
        Background::Dark
    } else {
        Background::Light
    }
}

/// The longest reply worth accumulating. A real one is under forty bytes; this
/// only bounds what a confused terminal can make us hold.
const MAX_REPLY: usize = 128;

/// Reads one OSC reply from `next`, consuming nothing that is not plainly ours.
///
/// The first byte must be ESC and the second `]`; anything else stops the read
/// at once. Type-ahead typed before the UI came up is discarded either way — the
/// position `features/terminal_input.rs::discard_type_ahead` already takes for
/// keystrokes with no consumer — but a terminal that simply stays silent must
/// not cost more than the one byte that proved it.
///
/// Taking the byte source as a closure is what keeps this **one** function
/// rather than one per platform: unix polls a file descriptor and Windows reads
/// console records, but the state machine over those bytes is identical, and a
/// second copy of it is a second place for the terminator handling to drift.
/// It also makes the machine testable without a terminal at all (see the tests).
fn read_reply(mut next: impl FnMut() -> Option<u8>) -> Option<Rgb> {
    if next()? != 0x1b {
        return None;
    }
    if next()? != b']' {
        return None;
    }
    let mut body = vec![0x1b, b']'];
    loop {
        let b = next()?;
        body.push(b);
        match b {
            0x07 => break, // BEL — tmux answers this way
            0x1b => {
                // ST (`ESC \`) — every other measured host answers this way;
                // one more byte belongs to the terminator.
                let _ = next();
                break;
            }
            _ if body.len() > MAX_REPLY => return None,
            _ => {}
        }
    }
    parse_reply(&body)
}

// ---------------------------------------------------------------------------
// exchange — the IO half.
// ---------------------------------------------------------------------------

/// A query in flight (unix), or an answer already in hand (Windows).
///
/// On unix this also owns the raw mode the query needed. If the process errors
/// out between asking and collecting, dropping this restores the terminal —
/// without that, a failure to open storage would hand the user a shell with no
/// echo. [`Pending::harvest`] disarms the restore, because by then `ratatui`
/// owns the terminal and will restore it itself on exit.
pub struct Pending {
    #[cfg(unix)]
    armed: bool,
    #[cfg(unix)]
    started: std::time::Instant,
    #[cfg(windows)]
    answer: Option<Background>,
}

/// Asks the terminal for its background.
///
/// Returns `None` when there is nothing to ask (not a terminal, or the query
/// was suppressed) — never an error: a terminal that will not answer is the
/// normal case on conhost, and the caller's fallback is correct there.
pub fn begin() -> Option<Pending> {
    match env_override() {
        Some(EnvOverride::Off) => {
            tracing::debug!("terminal background: query disabled by MINDFORK_TERMINAL_BG");
            return None;
        }
        Some(EnvOverride::Force(_)) => return None, // resolved by the caller, nothing to ask
        None => {}
    }
    // Both ends, and checked once for both platforms: a query needs somewhere to
    // go, and the reply needs somewhere to come from.
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        tracing::debug!("terminal background: not a terminal on both ends — not asking");
        return None;
    }
    begin_impl()
}

#[cfg(unix)]
fn begin_impl() -> Option<Pending> {
    use std::io::Write;

    // Raw mode is what stops the reply being echoed into the user's scrollback
    // and line-buffered out of reach. crossterm remembers the *original* mode
    // on the first enable, so `ratatui::init` enabling it again is harmless.
    if crossterm::terminal::enable_raw_mode().is_err() {
        return None;
    }
    let mut out = std::io::stdout();
    if out.write_all(QUERY_BG.as_bytes()).is_err() || out.flush().is_err() {
        let _ = crossterm::terminal::disable_raw_mode();
        return None;
    }
    Some(Pending {
        armed: true,
        started: std::time::Instant::now(),
    })
}

#[cfg(windows)]
fn begin_impl() -> Option<Pending> {
    // Windows does the whole exchange here, before `ratatui::init`: the reply
    // only arrives as VT bytes with `ENABLE_VIRTUAL_TERMINAL_INPUT`, a mode
    // crossterm neither sets nor expects to find, so it must not outlive the
    // query. Affordable because the hosts that answer do so in 16–31 ms.
    Some(Pending {
        answer: win::exchange(BUDGET),
    })
}

impl Pending {
    /// Collects the reply, waiting out whatever is left of [`BUDGET`].
    ///
    /// On unix the wait is measured from [`begin`], so the time spent opening
    /// storage counts against it: on every host that answers, the reply is
    /// already waiting and this returns at once.
    #[must_use]
    pub fn harvest(mut self) -> Option<Background> {
        #[cfg(unix)]
        {
            self.armed = false; // ratatui owns the terminal from here
            let deadline = self.started + BUDGET;
            let Some(rgb) = read_reply(|| unix::read_byte(deadline)) else {
                // Logged, not silent: a silent terminal and a detection that
                // never ran look identical from the outside, and telling them
                // apart is exactly what cost this track a false negative during
                // the spike (docs/research/auto-theme-detection.md §4.1).
                tracing::debug!(
                    budget_ms = BUDGET.as_millis(),
                    waited_ms = self.started.elapsed().as_millis(),
                    "terminal background: no reply — falling back to dark"
                );
                return None;
            };
            let bg = classify(rgb);
            tracing::debug!(
                r = rgb.r,
                g = rgb.g,
                b = rgb.b,
                luminance = relative_luminance(rgb),
                ?bg,
                "terminal background detected"
            );
            Some(bg)
        }
        #[cfg(windows)]
        {
            self.answer.take()
        }
    }
}

/// Turns a query started by [`begin`] into the final answer, applying the
/// environment override that [`begin`] deliberately left unresolved.
///
/// Kept here rather than at the call site so the precedence — a forced value
/// beats anything a terminal might have said — lives next to the code that
/// reads the variable.
#[must_use]
pub fn resolve(pending: Option<Pending>) -> Option<Background> {
    let (outcome, source) = match env_override() {
        Some(EnvOverride::Force(bg)) => (Some(bg), "forced by MINDFORK_TERMINAL_BG"),
        Some(EnvOverride::Off) => (None, "not asked — MINDFORK_TERMINAL_BG=off"),
        None => match pending.and_then(Pending::harvest) {
            Some(bg) => (Some(bg), "reported by the terminal"),
            None => (None, "the terminal did not answer"),
        },
    };
    // INFO, and exactly one line: this is a once-per-startup decision that
    // colours the whole UI, taken on a path the user cannot see, whose answer
    // depends on which terminal they happen to be in. That is precisely the
    // thing a support log should already contain rather than ask for — and the
    // fallback is indistinguishable from success without it.
    tracing::info!(
        background = ?outcome.unwrap_or(Background::Dark),
        detected = outcome.is_some(),
        source,
        "terminal background resolved"
    );
    outcome
}

#[cfg(unix)]
impl Drop for Pending {
    fn drop(&mut self) {
        if self.armed {
            // Asked, never collected — an error path between the two phases.
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
}

#[cfg(unix)]
mod unix {
    use std::time::Instant;

    /// One byte, waiting no longer than `deadline`.
    ///
    /// An exhausted deadline still *polls*, with a zero timeout: the budget
    /// bounds how long we are willing to **wait**, not whether we are willing
    /// to look. The two come apart because the budget runs from the moment the
    /// query was written, and the work in between (opening storage, migrations,
    /// a pre-migration backup) can outlast it — at which point the reply is
    /// already sitting in the buffer and returning `None` would throw away an
    /// answer we have.
    pub(super) fn read_byte(deadline: Instant) -> Option<u8> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let mut pfd = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let ms = i32::try_from(remaining.as_millis()).unwrap_or(i32::MAX);
        // SAFETY: a single initialised `pollfd`, a count matching it, and a
        // non-negative timeout — the contract `poll(2)` asks for.
        if unsafe { libc::poll(&mut pfd, 1, ms) } <= 0 {
            return None;
        }
        let mut byte = 0u8;
        // SAFETY: reading one byte into a live local.
        let n = unsafe {
            libc::read(
                libc::STDIN_FILENO,
                std::ptr::from_mut(&mut byte).cast::<libc::c_void>(),
                1,
            )
        };
        (n == 1).then_some(byte)
    }
}

#[cfg(windows)]
mod win {
    use std::io::Write;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Console::{
        CONSOLE_MODE, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
        ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode,
        GetStdHandle, INPUT_RECORD, KEY_EVENT, ReadConsoleInputW, STD_INPUT_HANDLE,
        STD_OUTPUT_HANDLE,
    };
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    use super::{Background, QUERY_BG, classify, relative_luminance};

    /// The whole exchange: set the console up, ask, read, put it back.
    pub(super) fn exchange(budget: Duration) -> Option<Background> {
        // SAFETY: the standard handles; every call below is checked.
        let h_in = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        let h_out = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        let saved_in = get_mode(h_in)?;
        let saved_out = get_mode(h_out)?;

        set_mode(h_out, saved_out | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
        let wanted = (saved_in & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT))
            | ENABLE_VIRTUAL_TERMINAL_INPUT;
        set_mode(h_in, wanted);

        // Legacy conhost drops the VT-input bit rather than failing, and then no
        // reply can reach us as bytes. Reading the mode back tells the two cases
        // apart in the log, which is the difference the spike had to chase down.
        let vt_input = get_mode(h_in).is_some_and(|m| m & ENABLE_VIRTUAL_TERMINAL_INPUT != 0);
        let result = if vt_input {
            ask(budget)
        } else {
            tracing::debug!(
                "terminal background: console refused ENABLE_VIRTUAL_TERMINAL_INPUT — no reply is possible"
            );
            None
        };

        set_mode(h_in, saved_in);
        set_mode(h_out, saved_out);
        result
    }

    fn ask(budget: Duration) -> Option<Background> {
        let mut out = std::io::stdout();
        out.write_all(QUERY_BG.as_bytes()).ok()?;
        out.flush().ok()?;

        let deadline = Instant::now() + budget;
        let Some(rgb) = super::read_reply(|| read_byte(deadline)) else {
            tracing::debug!(
                budget_ms = budget.as_millis(),
                "terminal background: no reply — falling back to dark"
            );
            return None;
        };
        let bg = classify(rgb);
        tracing::debug!(
            r = rgb.r,
            g = rgb.g,
            b = rgb.b,
            luminance = relative_luminance(rgb),
            ?bg,
            "terminal background detected"
        );
        Some(bg)
    }

    /// One byte of the reply, or `None` at the deadline.
    ///
    /// With VT input enabled the reply arrives as ordinary key-down records
    /// carrying one character each; everything else (key-up, window resize,
    /// focus) is skipped without counting against the caller.
    fn read_byte(deadline: Instant) -> Option<u8> {
        // SAFETY: the standard input handle.
        let h_in = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        loop {
            // Zero remaining still waits with a zero timeout, i.e. checks what
            // has already arrived — see the unix `read_byte` for why the budget
            // bounds waiting rather than looking.
            let remaining = deadline.saturating_duration_since(Instant::now());
            let ms = u32::try_from(remaining.as_millis()).unwrap_or(u32::MAX);
            // SAFETY: waiting on a console input handle we own.
            if unsafe { WaitForSingleObject(h_in, ms) } != WAIT_OBJECT_0 {
                return None;
            }
            let mut record: INPUT_RECORD = unsafe { std::mem::zeroed() };
            let mut read = 0u32;
            // SAFETY: one record, one count, both live for the call.
            let ok = unsafe { ReadConsoleInputW(h_in, &mut record, 1, &mut read) };
            if ok == 0 || read != 1 {
                return None;
            }
            if record.EventType != KEY_EVENT as u16 {
                continue;
            }
            // SAFETY: the union is a KEY_EVENT_RECORD, as `EventType` just said.
            let key = unsafe { record.Event.KeyEvent };
            if key.bKeyDown == 0 {
                continue;
            }
            // SAFETY: reading the character out of the same union.
            let ch = unsafe { key.uChar.UnicodeChar };
            if ch == 0 {
                continue;
            }
            if let Ok(byte) = u8::try_from(ch) {
                return Some(byte);
            }
            // A reply is ASCII throughout; anything wider is not ours.
            return None;
        }
    }

    fn get_mode(handle: HANDLE) -> Option<CONSOLE_MODE> {
        let mut mode: CONSOLE_MODE = 0;
        // SAFETY: `mode` outlives the call.
        (unsafe { GetConsoleMode(handle, &mut mode) } != 0).then_some(mode)
    }

    fn set_mode(handle: HANDLE, mode: CONSOLE_MODE) {
        // SAFETY: a handle we obtained and a mode value; failure is ignorable
        // (the restore path must not panic).
        unsafe {
            windows_sys::Win32::System::Console::SetConsoleMode(handle, mode);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Replies recorded from real terminals during the spike (research doc §5),
    /// plus the component widths the specification allows. The Python probe's
    /// `--self-test` carries the same corpus, so the two agree by construction.
    #[test]
    fn parses_the_measured_replies() {
        // Windows Terminal, VS Code and JupyterLab answer ST-terminated; tmux
        // answers BEL-terminated. Both shapes are here because both were
        // measured, and a reader that took only one would lose either tmux or
        // every other host.
        let cases: &[(&[u8], Rgb, Background)] = &[
            (
                b"\x1b]11;rgb:0c0c/0c0c/0c0c\x1b\\",
                Rgb {
                    r: 12,
                    g: 12,
                    b: 12,
                },
                Background::Dark,
            ),
            (
                b"\x1b]11;rgb:1919/1a1a/1b1b\x1b\\",
                Rgb {
                    r: 25,
                    g: 26,
                    b: 27,
                },
                Background::Dark,
            ),
            (
                b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\",
                Rgb {
                    r: 255,
                    g: 255,
                    b: 255,
                },
                Background::Light,
            ),
            // tmux 3.4 over SSH, asked unwrapped — the one host that terminates
            // with BEL, reporting the background of the terminal it runs inside.
            (
                b"\x1b]11;rgb:0c0c/0c0c/0c0c\x07",
                Rgb {
                    r: 12,
                    g: 12,
                    b: 12,
                },
                Background::Dark,
            ),
        ];
        for (raw, expected, verdict) in cases {
            let rgb = parse_reply(raw).expect("a recorded reply must parse");
            assert_eq!(rgb, *expected, "reply {raw:?}");
            assert_eq!(classify(rgb), *verdict, "reply {raw:?}");
        }
    }

    #[test]
    fn accepts_every_component_width_and_the_hash_form() {
        // BEL-terminated, and 1/2/3-digit components: each is scaled by its own
        // width, so `f` is full-scale rather than 15/255.
        assert_eq!(
            parse_reply(b"\x1b]11;rgb:f/f/f\x07"),
            Some(Rgb {
                r: 255,
                g: 255,
                b: 255
            })
        );
        assert_eq!(
            parse_reply(b"\x1b]11;rgb:fd/f6/e3\x1b\\"),
            Some(Rgb {
                r: 253,
                g: 246,
                b: 227
            })
        );
        assert_eq!(
            parse_reply(b"\x1b]11;rgb:000/000/000\x07"),
            Some(Rgb { r: 0, g: 0, b: 0 })
        );
        assert_eq!(
            parse_reply(b"\x1b]11;#282c34\x07"),
            Some(Rgb {
                r: 40,
                g: 44,
                b: 52
            })
        );
    }

    #[test]
    fn rejects_what_is_not_a_colour() {
        for raw in [
            &b"\x1b]11;\x07"[..],
            b"\x1b]11;rgb:zz/zz/zz\x07",
            b"\x1b]11;rgb:1/2\x07",
            b"\x1b]11;rgb:11111/2/3\x07",
            b"",
            b"garbage",
        ] {
            assert_eq!(parse_reply(raw), None, "must not parse: {raw:?}");
        }
    }

    #[test]
    fn luminance_matches_the_measured_verdicts() {
        // The measured backgrounds sit far from the threshold in both
        // directions — the margin is the point, not the exact figures.
        let dark = relative_luminance(Rgb {
            r: 12,
            g: 12,
            b: 12,
        });
        let light = relative_luminance(Rgb {
            r: 255,
            g: 255,
            b: 255,
        });
        assert!(dark < 0.01, "conhost/WT background measured {dark}");
        assert!((light - 1.0).abs() < f32::EPSILON, "white measured {light}");
        assert!(dark < DARK_THRESHOLD && light > DARK_THRESHOLD);
    }

    #[test]
    fn the_query_is_the_bytes_terminals_were_measured_with() {
        assert_eq!(QUERY_BG.as_bytes(), b"\x1b]11;?\x07");
    }

    /// Feeds [`read_reply`] a scripted stream and counts what it consumed.
    fn reader(
        stream: &'static [u8],
    ) -> (
        impl FnMut() -> Option<u8>,
        std::rc::Rc<std::cell::Cell<usize>>,
    ) {
        let taken = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let counter = std::rc::Rc::clone(&taken);
        let next = move || {
            let i = counter.get();
            let b = stream.get(i).copied();
            if b.is_some() {
                counter.set(i + 1);
            }
            b
        };
        (next, taken)
    }

    #[test]
    fn reads_a_reply_under_either_terminator() {
        for stream in [
            &b"\x1b]11;rgb:1e1e/1e1e/1e1e\x07"[..], // BEL, as tmux answers
            b"\x1b]11;rgb:1e1e/1e1e/1e1e\x1b\\",    // ST, as every other host does
        ] {
            let (next, _) = reader(stream);
            assert_eq!(
                read_reply(next),
                Some(Rgb {
                    r: 30,
                    g: 30,
                    b: 30
                }),
                "stream {stream:?}"
            );
        }
    }

    #[test]
    fn a_silent_terminal_costs_at_most_the_byte_that_proved_it() {
        // The safety property. A terminal that does not answer leaves whatever
        // the user typed in the buffer, and a reader that swallowed it would be
        // a worse defect than the one this module exists to fix.
        let (next, taken) = reader(b"");
        assert_eq!(read_reply(next), None);
        assert_eq!(taken.get(), 0, "silence must cost nothing");

        let (next, taken) = reader(b"q");
        assert_eq!(read_reply(next), None);
        assert_eq!(taken.get(), 1, "one keystroke must cost exactly one byte");
    }

    #[test]
    fn a_truncated_or_endless_reply_yields_nothing() {
        for stream in [
            &b"\x1b"[..],      // a bare ESC and then silence
            b"\x1b]11;rgb:11", // never terminated
            b"\x1bA",          // ESC, then something that is not ours
        ] {
            let (next, _) = reader(stream);
            assert_eq!(read_reply(next), None, "stream {stream:?}");
        }
    }

    #[test]
    fn a_terminal_that_never_stops_talking_is_bounded() {
        // `MAX_REPLY` is what stops an unterminated flood being accumulated
        // forever; the stream here is longer than the cap and never terminates.
        const FLOOD: &[u8] = b"\x1b]11;rgb:0000/0000/0000\
            aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\
            aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\
            aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        assert!(FLOOD.len() > MAX_REPLY);
        let (next, taken) = reader(FLOOD);
        assert_eq!(read_reply(next), None);
        assert!(
            taken.get() <= MAX_REPLY + 2,
            "read {} bytes, past the cap",
            taken.get()
        );
    }
}

//! Layout-independent hotkeys.
//!
//! A terminal reports the character of the **active layout**, not the physical
//! key: on the Russian JCUKEN layout, pressing physical key `L` gives
//! `KeyCode::Char('д')`, so matching a shortcut like `Ctrl+L` breaks.
//! [`hotkey_char`] resolves the event back to the Latin letter on the same
//! physical key, so `Ctrl+<letter>` works under any layout. Text input itself is
//! left alone — normalization is only for shortcut matching (under `Ctrl`).
//! See spec §11.7 and docs/research/layout-independent-hotkeys.md.
//!
//! Resolution tiers (first hit wins):
//!
//! 1. **Windows, active layout** — the character we received is the output of
//!    crossterm's `ToUnicodeEx(vk, active layout)` call, so `VkKeyScanExW` +
//!    `MapVirtualKeyExW` against the *same* layout is its exact inverse. Works
//!    for every installed layout, including ones that don't exist yet — no
//!    per-language data.
//! 2. **Static table** — the standard Russian JCUKEN geometry (also covers
//!    Kazakh/Kyrgyz/Tatar/… — their letter zone matches; the extra national
//!    letters sit on the digit row, which hotkeys don't use). On Windows this
//!    only runs when the active layout couldn't be determined (conhost, see
//!    below), where it is the safer answer for the layout we ship for.
//! 3. **Windows, installed layouts** — probe every installed layout for a
//!    character the table doesn't know (Greek, Hebrew, Georgian, …).
//! 4. Pass-through (lowercased).
//!
//! Not covered: unix terminals that report the layout character (kitty keyboard
//! protocol) for scripts outside the static table. The protocol carries the
//! answer — the *base layout key* of the "report alternate keys" enhancement —
//! but crossterm 0.29 parses only the shifted alternate and drops it
//! (crossterm-rs/crossterm#968). Once that lands upstream it becomes tier 0
//! here, and no call site changes. Terminals of the VTE family (GNOME Terminal
//! & co.) need none of this: in the legacy encoding they fall back to the Latin
//! group themselves and send the C0 code of the physical key.
//!
//! The module's second half is the other direction of the same question — not
//! "which key was that" but "which chord can this terminal even deliver":
//! [`newline_chord`] names the one that inserts a line break here (spec §11.5).

use std::sync::OnceLock;

use ratatui::crossterm::event::{KeyCode, KeyEvent};

/// Returns the "physical" Latin character (lowercase) for a hotkey event, or
/// `None` if the event isn't a character key.
///
/// This is the entry point for every `Ctrl+<key>` match in the UI; see the
/// module docs for the resolution tiers.
pub fn hotkey_char(key: &KeyEvent) -> Option<char> {
    match key.code {
        KeyCode::Char(c) => Some(physical_key_char(c)),
        _ => None,
    }
}

/// Resolves a character to the Latin letter on the same physical key.
fn physical_key_char(c: char) -> char {
    let lower = lowercase(c);
    // ASCII is never remapped: on Latin layouts (AZERTY/QWERTZ/Dvorak/Colemak)
    // a shortcut belongs to the key *labeled* with that letter, wherever it
    // sits — that is what the user expects and what we have always done.
    if lower.is_ascii() {
        return lower;
    }
    // Tier 1: exact inverse of the transform the terminal layer applied.
    #[cfg(windows)]
    if let Some(ch) = win_layout::from_active_layout(lower) {
        return ch;
    }
    // Tier 2. On Windows this is reached only when the active layout is
    // undeterminable (conhost — see `win_layout::foreground_layout`): prefer the
    // known-good geometry over guessing among installed layouts, which could
    // pick a different one that happens to contain the same letter.
    if let Some(ch) = jcuken_key(lower) {
        return ch;
    }
    // Tier 3: any other installed layout (Greek, Hebrew, Georgian, …).
    #[cfg(windows)]
    if let Some(ch) = win_layout::from_installed_layouts(lower) {
        return ch;
    }
    lower
}

/// Lowercases a character, keeping it as-is if it lowercases to several
/// characters (no such case among letter keys).
fn lowercase(c: char) -> char {
    let mut it = c.to_lowercase();
    match (it.next(), it.next()) {
        (Some(lower), None) => lower,
        _ => c,
    }
}

/// The "physical" Latin letter for a character of the standard Russian JCUKEN
/// layout (`й`→`q`, `д`→`l`, `с`→`c`, …), or `None` if it isn't one.
fn jcuken_key(lower: char) -> Option<char> {
    Some(match lower {
        // top letter row: `й``ц``у``к``е``н``г``ш``щ``з` → QWERTYUIOP
        'й' => 'q',
        'ц' => 'w',
        'у' => 'e',
        'к' => 'r',
        'е' => 't',
        'н' => 'y',
        'г' => 'u',
        'ш' => 'i',
        'щ' => 'o',
        'з' => 'p',
        // middle row: `ф``ы``в``а``п``р``о``л``д` → ASDFGHJKL
        'ф' => 'a',
        'ы' => 's',
        'в' => 'd',
        'а' => 'f',
        'п' => 'g',
        'р' => 'h',
        'о' => 'j',
        'л' => 'k',
        'д' => 'l',
        // bottom row: `я``ч``с``м``и``т``ь` → ZXCVBNM
        'я' => 'z',
        'ч' => 'x',
        'с' => 'c',
        'м' => 'v',
        'и' => 'b',
        'т' => 'n',
        'ь' => 'm',
        _ => return None,
    })
}

/// Returns the "physical" Latin letter (lowercase) for character `c` using the
/// static table only — the pure core of tiers 2/4, without OS lookups.
#[cfg(test)]
fn physical_char(c: char) -> char {
    let lower = lowercase(c);
    jcuken_key(lower).unwrap_or(lower)
}

/// Returns `true` if the character corresponds to the physical `/` key
/// (opening search).
///
/// On a Latin layout that's `/`. On Russian JCUKEN the physical `/?` key
/// without Shift gives `.`, so we accept that too — otherwise `/` search can't
/// be opened under active Cyrillic input. `.` is ambiguous (on Latin it's on a
/// separate `.>` key), but where this shortcut is used — at the top level of a
/// screen, outside an editor/overlay — `.` does nothing else, so a false
/// trigger on Latin `.` is harmless (search closes via `Esc`). Deliberately not
/// routed through the layout resolver: both characters are ASCII, and ASCII is
/// never remapped by position (see [`physical_key_char`]).
pub fn is_slash_key(c: char) -> bool {
    c == '/' || c == '.'
}

// --------------------------------------------------------------------------
// What the terminal can say about `Enter` (spec §11.5)
// --------------------------------------------------------------------------

/// Whether the terminal reports modifiers on `Enter` — i.e. whether pressing
/// `Shift+Enter` can reach the app as anything other than a bare `Enter`.
///
/// True on Windows (the Console API always reports modifiers) and on a unix
/// terminal that accepted the kitty keyboard protocol; false on every other
/// unix terminal, where the legacy encoding sends the same CR for both — or,
/// in Konsole's case, something we never see at all (see [`newline_chord`]).
///
/// A process-wide `OnceLock` set once from startup (`app/runtime`), for the
/// same reason [`crate::shared::theme::detected_background`] is one: it is a
/// single immutable fact about the terminal this process is attached to, read
/// from render paths that run per frame. Unset — in tests, and in any path
/// that never initialised a terminal — reads as `true`, which keeps the
/// historical wording (and the committed screenshot dumps) unchanged.
static MODIFIED_ENTER_REPORTED: OnceLock<bool> = OnceLock::new();

/// The chord to name when the terminal *does* report a modified `Enter`.
const SHIFT_ENTER: &str = "Shift+Enter";
/// The chord to name when it does not; both are handled everywhere a line
/// break is accepted (`screens/chat/input.rs`, settings, the self-model editor).
const ALT_ENTER: &str = "Alt+Enter";

/// Records what the terminal turned out to be capable of. Called once, from
/// startup; later calls are ignored, so nothing can re-label a running UI.
pub fn set_modified_enter_reported(reported: bool) {
    let _ = MODIFIED_ENTER_REPORTED.set(reported);
}

/// The chord that actually inserts a line break in this terminal — what the
/// input box's footer and the multiline editors advertise.
///
/// Both chords always work; only one of them is always *deliverable*. The
/// terminal that forced this to be a question rather than a constant is
/// **Konsole**: its default keytab maps `Shift+Return` to `\EOM` (SS3 `M`,
/// the keypad Enter), crossterm's unix parser has no arm for that final byte,
/// and its `Err` branch clears the buffer — so the keypress produces no event
/// at all and the footer's promise silently fails. No released Konsole
/// (≤ 26.04) answers the kitty protocol's `CSI ? u` either, so there is
/// nothing to negotiate: on such a terminal the honest thing is to name
/// `Alt+Enter`, which arrives as `ESC` + CR = `Enter`+`ALT`.
pub fn newline_chord() -> &'static str {
    chord_for(MODIFIED_ENTER_REPORTED.get().copied().unwrap_or(true))
}

/// The pure half of [`newline_chord`] — the choice itself, without the global.
const fn chord_for(modified_enter_reported: bool) -> &'static str {
    if modified_enter_reported {
        SHIFT_ENTER
    } else {
        ALT_ENTER
    }
}

/// Resolving a character back to its physical key through the Windows keyboard
/// layout tables (tiers 1 and 3, see the module docs).
///
/// Both APIs are pure lookups in the layout identified by an `HKL`: they don't
/// touch the thread's keyboard state and don't allocate on our behalf.
#[cfg(windows)]
mod win_layout {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyboardLayout, GetKeyboardLayoutList, HKL, MAPVK_VK_TO_VSC, MapVirtualKeyExW,
        VkKeyScanExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId,
    };

    /// Upper bound on the installed layouts we probe (a machine with more is
    /// not a scenario worth an allocation).
    const MAX_LAYOUTS: usize = 16;

    /// `VkKeyScanExW` shift-state bits (high byte of the return value).
    const SHIFT_STATE_CTRL: i16 = 2;
    const SHIFT_STATE_ALT: i16 = 4;

    /// Resolves `c` through the layout that was active when the character was
    /// produced — the exact inverse of the terminal layer's lookup.
    pub(super) fn from_active_layout(c: char) -> Option<char> {
        translate(c, foreground_layout()?)
    }

    /// Resolves `c` through any installed layout that contains it.
    ///
    /// The active-layout ordering is what disambiguates a machine with two
    /// layouts holding the same letter on different keys (e.g. Russian and
    /// Serbian); here we have no such signal and take the first match in
    /// system order.
    pub(super) fn from_installed_layouts(c: char) -> Option<char> {
        installed_layouts()
            .into_iter()
            .find_map(|hkl| translate(c, hkl))
    }

    /// char → virtual key → scan code → the letter at that position on QWERTY.
    fn translate(c: char, hkl: HKL) -> Option<char> {
        // `VkKeyScanExW` takes a single UTF-16 code unit, so a character
        // outside the BMP can't be on a key by definition.
        let code = u16::try_from(c as u32).ok()?;
        // SAFETY: a table lookup in the layout `hkl`; no pointers are passed or
        // dereferenced, and the thread's keyboard state is not modified.
        let scan = unsafe {
            let res = VkKeyScanExW(code, hkl);
            // -1 in both bytes: no key of this layout produces the character.
            if res == -1 {
                return None;
            }
            // Characters reachable only through Ctrl/AltGr are not what the
            // terminal layer reports for a bare key press (it queries the
            // layout with an empty modifier state), so such a match is a
            // different key — skip it.
            if (res >> 8) & (SHIFT_STATE_CTRL | SHIFT_STATE_ALT) != 0 {
                return None;
            }
            MapVirtualKeyExW((res & 0xFF) as u32, MAPVK_VK_TO_VSC, hkl)
        };
        qwerty_at(scan)
    }

    /// The layout of the window that owns the keyboard focus.
    ///
    /// Mirrors what crossterm does to *produce* the character (`GetForegroundWindow`
    /// → `GetWindowThreadProcessId` → `GetKeyboardLayout`), so that our inverse
    /// uses the same layout. Works under Windows Terminal; under conhost the
    /// foreground window can't be queried this way and we get `None` — hence
    /// tier 2 ordering (see the module docs).
    fn foreground_layout() -> Option<HKL> {
        // SAFETY: no pointers are dereferenced; `GetWindowThreadProcessId` is
        // given a null out-param (the process id is not needed) and a window
        // handle we just obtained.
        let hkl = unsafe {
            let window = GetForegroundWindow();
            if window.is_null() {
                return None;
            }
            let thread = GetWindowThreadProcessId(window, std::ptr::null_mut());
            if thread == 0 {
                return None;
            }
            GetKeyboardLayout(thread)
        };
        (!hkl.is_null()).then_some(hkl)
    }

    /// Every input locale installed in the system.
    fn installed_layouts() -> Vec<HKL> {
        let mut list = [std::ptr::null_mut(); MAX_LAYOUTS];
        // SAFETY: the buffer is exactly `MAX_LAYOUTS` entries long and that is
        // the capacity we declare; the OS fills at most that many and returns
        // how many it wrote.
        let written = unsafe { GetKeyboardLayoutList(MAX_LAYOUTS as i32, list.as_mut_ptr()) };
        let written = written.max(0) as usize;
        list.into_iter().take(written.min(MAX_LAYOUTS)).collect()
    }

    /// The character printed at a physical key position on a US QWERTY
    /// keyboard, by IBM PC **Set 1** make code.
    ///
    /// Scan codes are a hardware property, identical under every layout — this
    /// is what makes the table language-independent (one fixed table, forever)
    /// instead of one table per layout. Rows are contiguous, so each is written
    /// as its base code plus the characters along it.
    fn qwerty_at(scan: u32) -> Option<char> {
        const ROWS: [(u32, &str); 4] = [
            (0x02, "1234567890-="),
            (0x10, "qwertyuiop[]"),
            (0x1E, "asdfghjkl;'"),
            (0x2C, "zxcvbnm,./"),
        ];
        match scan {
            // The two keys that break row contiguity.
            0x29 => return Some('`'),
            0x2B => return Some('\\'),
            _ => {}
        }
        ROWS.iter().find_map(|&(base, row)| {
            let offset = usize::try_from(scan.checked_sub(base)?).ok()?;
            row.chars().nth(offset)
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn qwerty_table_covers_the_main_block() {
            // Letter rows (the hotkey-relevant part).
            assert_eq!(qwerty_at(0x10), Some('q'));
            assert_eq!(qwerty_at(0x19), Some('p'));
            assert_eq!(qwerty_at(0x1E), Some('a'));
            assert_eq!(qwerty_at(0x26), Some('l'));
            assert_eq!(qwerty_at(0x2C), Some('z'));
            assert_eq!(qwerty_at(0x32), Some('m'));
            // Digits and punctuation.
            assert_eq!(qwerty_at(0x02), Some('1'));
            assert_eq!(qwerty_at(0x0B), Some('0'));
            assert_eq!(qwerty_at(0x35), Some('/'));
            assert_eq!(qwerty_at(0x29), Some('`'));
            assert_eq!(qwerty_at(0x2B), Some('\\'));
            // Outside the alphanumeric block: Esc, Backspace, Tab, Enter,
            // LShift, Space — no character.
            for scan in [0x01, 0x0E, 0x0F, 0x1C, 0x2A, 0x39, 0x56, 0x100] {
                assert_eq!(qwerty_at(scan), None, "scan {scan:#x}");
            }
        }

        /// The whole OS pipeline (`VkKeyScanExW` → `MapVirtualKeyExW` → table)
        /// against the machine's real layouts. Latin letters are on distinct
        /// physical keys in every Latin layout, which is assertable without
        /// knowing which one is installed (QWERTY gives `q`, AZERTY `a`).
        #[test]
        fn resolves_latin_letters_through_installed_layouts() {
            let q = from_installed_layouts('q').expect("some installed layout has 'q'");
            let a = from_installed_layouts('a').expect("some installed layout has 'a'");
            assert!(q.is_ascii_lowercase(), "unexpected {q:?}");
            assert!(a.is_ascii_lowercase(), "unexpected {a:?}");
            assert_ne!(q, a, "distinct characters must resolve to distinct keys");
        }

        /// Cross-checks the OS pipeline against the static JCUKEN table: on a
        /// machine with the standard Russian layout installed, both must agree
        /// for every character of the table. Skipped when Russian isn't
        /// installed (CI) or is a non-standard variant (Typewriter), where a
        /// disagreement would be the correct answer, not a defect.
        #[test]
        fn agrees_with_the_static_table_when_standard_russian_is_installed() {
            if from_installed_layouts('й') != Some('q') {
                eprintln!(
                    "skipping: no standard Russian layout installed \
                     (nothing to cross-check the OS lookup against)"
                );
                return;
            }
            for c in "йцукенгшщзфывапролдячсмить".chars() {
                assert_eq!(
                    from_installed_layouts(c),
                    super::super::jcuken_key(c),
                    "OS lookup and the static table disagree on {c:?}"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn newline_chord_follows_what_the_terminal_can_report() {
        // The whole point of the flag: on a terminal that cannot deliver a
        // modified `Enter` (Konsole sends `\EOM`, which crossterm drops; a bare
        // xterm sends a plain CR) the footer must name the chord that works.
        assert_eq!(chord_for(true), "Shift+Enter");
        assert_eq!(chord_for(false), "Alt+Enter");
    }

    #[test]
    fn newline_chord_defaults_to_shift_enter_until_startup_says_otherwise() {
        // Unset — tests, and any path with no terminal behind it (the demo
        // frame dumps among them). The default must be the historical wording,
        // and this test must not `set` the global: it is process-wide, and one
        // test flipping it would re-label every other test's UI.
        assert_eq!(newline_chord(), "Shift+Enter");
    }

    #[test]
    fn latin_letters_pass_through_lowercased() {
        assert_eq!(physical_char('l'), 'l');
        assert_eq!(physical_char('L'), 'l');
        assert_eq!(physical_char('1'), '1');
    }

    #[test]
    fn cyrillic_maps_to_physical_qwerty_key() {
        // Shortcuts key to the app: L, N, C, G, T, P.
        assert_eq!(physical_char('д'), 'l');
        assert_eq!(physical_char('т'), 'n');
        assert_eq!(physical_char('с'), 'c');
        assert_eq!(physical_char('п'), 'g');
        assert_eq!(physical_char('е'), 't');
        assert_eq!(physical_char('з'), 'p');
        // Cyrillic case is normalized too
        assert_eq!(physical_char('Д'), 'l');
    }

    #[test]
    fn unknown_characters_pass_through() {
        // The pure core (tiers 2/4): a character outside the table is returned
        // lowercased, unchanged.
        for c in ['界', '😀', '\u{a0}'] {
            assert_eq!(physical_char(c), lowercase(c));
        }
    }

    #[test]
    fn hotkey_char_reads_character_keys_only() {
        assert_eq!(hotkey_char(&ctrl('l')), Some('l'));
        assert_eq!(hotkey_char(&ctrl('L')), Some('l'));
        assert_eq!(
            hotkey_char(&KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)),
            None
        );
        assert_eq!(
            hotkey_char(&KeyEvent::new(KeyCode::F(10), KeyModifiers::NONE)),
            None
        );
    }

    #[test]
    fn hotkey_char_resolves_a_non_latin_key_to_a_latin_letter() {
        // Exact target ('д' is on the L key) is asserted on the pure table
        // above; here the invariant that holds on every machine, whichever
        // tier answers: a Cyrillic letter resolves to *some* Latin key.
        let resolved = hotkey_char(&ctrl('д')).expect("a character key");
        assert!(resolved.is_ascii_lowercase(), "unexpected {resolved:?}");
    }

    #[test]
    fn slash_key_accepts_latin_and_cyrillic_layout() {
        // Latin layout gives `/`; Russian JCUKEN on the same key gives `.`.
        assert!(is_slash_key('/'));
        assert!(is_slash_key('.'));
        assert!(!is_slash_key(','));
        assert!(!is_slash_key('a'));
    }
}

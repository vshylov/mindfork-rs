//! Layout-independent hotkeys.
//!
//! Crossterm in normal mode reports the character of the **active layout**, not
//! the physical key: on the Russian JCUKEN layout, pressing physical key `L`
//! gives `KeyCode::Char('д')`, not `'l'`, so matching a shortcut like `Ctrl+L`
//! breaks. [`physical_char`] translates a Cyrillic character to the Latin letter
//! on the same physical key, so `Ctrl+<letter>` works under any layout. Text
//! input itself is left alone — normalization is only for shortcut matching
//! (under `Ctrl`). See spec §11.7.

/// Returns the "physical" Latin letter (lowercase) for character `c`.
///
/// For Latin — just `c` lowercased. For Cyrillic — the letter on the same key
/// of the standard Russian JCUKEN layout (`й`→`q`, `д`→`l`, `с`→`c`, …). Other
/// characters pass through as-is (lowercased).
pub fn physical_char(c: char) -> char {
    let lower = c.to_lowercase().next().unwrap_or(c);
    match lower {
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
        other => other,
    }
}

/// Returns `true` if the character corresponds to the physical `/` key
/// (opening search).
///
/// On a Latin layout that's `/`. On Russian JCUKEN the physical `/?` key
/// without Shift gives `.`, so we accept that too — otherwise `/` search can't
/// be opened under active Cyrillic input. `.` is ambiguous (on Latin it's on a
/// separate `.>` key), but where this shortcut is used — at the top level of a
/// screen, outside an editor/overlay — `.` does nothing else, so a false
/// trigger on Latin `.` is harmless (search closes via `Esc`).
pub fn is_slash_key(c: char) -> bool {
    c == '/' || c == '.'
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn slash_key_accepts_latin_and_cyrillic_layout() {
        // Latin layout gives `/`; Russian JCUKEN on the same key gives `.`.
        assert!(is_slash_key('/'));
        assert!(is_slash_key('.'));
        assert!(!is_slash_key(','));
        assert!(!is_slash_key('a'));
    }
}

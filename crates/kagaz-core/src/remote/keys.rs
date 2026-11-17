//! Keys, from how the window's page names them to what a session sends.
//!
//! The page gives each key press twice over, the way browsers do: `key`,
//! what the press means with the layout and Shift applied ("a", "A", "é",
//! "Enter"), and `code`, which key of the board it was ("KeyA",
//! "ShiftRight"). VNC wants the meaning, as an X11 key symbol; Remote
//! Desktop wants the key, as the number a PC keyboard gives it, and lets
//! the device's own layout decide what it means.

/// The X11 key symbol for a key press, or none for one that has no symbol
/// (a dead key, a key the page could not name).
pub fn keysym(key: &str, code: &str) -> Option<u32> {
    let mut chars = key.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Some(char_keysym(c));
    }
    let right = code.ends_with("Right");
    Some(match key {
        "Backspace" => 0xff08,
        "Tab" => 0xff09,
        "Enter" => {
            if code == "NumpadEnter" {
                0xff8d
            } else {
                0xff0d
            }
        }
        "Escape" => 0xff1b,
        "Delete" => 0xffff,
        "Home" => 0xff50,
        "ArrowLeft" => 0xff51,
        "ArrowUp" => 0xff52,
        "ArrowRight" => 0xff53,
        "ArrowDown" => 0xff54,
        "PageUp" => 0xff55,
        "PageDown" => 0xff56,
        "End" => 0xff57,
        "Insert" => 0xff63,
        "Shift" => 0xffe1 + right as u32,
        "Control" => 0xffe3 + right as u32,
        "CapsLock" => 0xffe5,
        "Alt" => 0xffe9 + right as u32,
        "AltGraph" => 0xfe03,
        // The Windows key, Command, Super: one key under three names.
        "Meta" | "OS" | "Super" => 0xffeb + right as u32,
        "ContextMenu" => 0xff67,
        "NumLock" => 0xff7f,
        "ScrollLock" => 0xff14,
        "Pause" => 0xff13,
        "PrintScreen" => 0xff61,
        _ => {
            let n: u32 = key.strip_prefix('F')?.parse().ok()?;
            if !(1..=24).contains(&n) {
                return None;
            }
            0xffbe + n - 1
        }
    })
}

/// A character's key symbol: Latin-1 is itself, the rest is Unicode's
/// number in the range X11 keeps for it.
pub fn char_keysym(c: char) -> u32 {
    match c as u32 {
        cp @ (0x20..=0x7e | 0xa0..=0xff) => cp,
        // Enter and Tab typed as text (a pasted line, a phone's keyboard).
        0x0a | 0x0d => 0xff0d,
        0x09 => 0xff09,
        cp => 0x0100_0000 + cp,
    }
}

/// The number a PC keyboard gives a key (scancode set 1), and whether it
/// is one of the "extended" keys that share a number with another.
pub fn scancode(code: &str) -> Option<(u8, bool)> {
    const LETTERS: &[u8; 26] = &[
        0x1e, 0x30, 0x2e, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18, 0x19, 0x10, 0x13, 0x1f,
        0x14, 0x16, 0x2f, 0x11, 0x2d, 0x15, 0x2c,
    ];
    if let Some(letter) = code.strip_prefix("Key") {
        let c = letter.bytes().next().filter(|c| letter.len() == 1 && c.is_ascii_uppercase())?;
        return Some((LETTERS[(c - b'A') as usize], false));
    }
    if let Some(digit) = code.strip_prefix("Digit") {
        let d = digit.bytes().next().filter(|d| digit.len() == 1 && d.is_ascii_digit())? - b'0';
        return Some((if d == 0 { 0x0b } else { 0x01 + d }, false));
    }
    if let Some(n) = code.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
        return match n {
            1..=10 => Some((0x3a + n, false)),
            11 => Some((0x57, false)),
            12 => Some((0x58, false)),
            _ => None,
        };
    }
    let plain = |n: u8| Some((n, false));
    let extended = |n: u8| Some((n, true));
    match code {
        "Escape" => plain(0x01),
        "Minus" => plain(0x0c),
        "Equal" => plain(0x0d),
        "Backspace" => plain(0x0e),
        "Tab" => plain(0x0f),
        "BracketLeft" => plain(0x1a),
        "BracketRight" => plain(0x1b),
        "Enter" => plain(0x1c),
        "ControlLeft" => plain(0x1d),
        "Semicolon" => plain(0x27),
        "Quote" => plain(0x28),
        "Backquote" => plain(0x29),
        "ShiftLeft" => plain(0x2a),
        "Backslash" => plain(0x2b),
        "Comma" => plain(0x33),
        "Period" => plain(0x34),
        "Slash" => plain(0x35),
        "ShiftRight" => plain(0x36),
        "NumpadMultiply" => plain(0x37),
        "AltLeft" => plain(0x38),
        "Space" => plain(0x39),
        "CapsLock" => plain(0x3a),
        "NumLock" => plain(0x45),
        "ScrollLock" => plain(0x46),
        "Numpad7" => plain(0x47),
        "Numpad8" => plain(0x48),
        "Numpad9" => plain(0x49),
        "NumpadSubtract" => plain(0x4a),
        "Numpad4" => plain(0x4b),
        "Numpad5" => plain(0x4c),
        "Numpad6" => plain(0x4d),
        "NumpadAdd" => plain(0x4e),
        "Numpad1" => plain(0x4f),
        "Numpad2" => plain(0x50),
        "Numpad3" => plain(0x51),
        "Numpad0" => plain(0x52),
        "NumpadDecimal" => plain(0x53),
        "IntlBackslash" => plain(0x56),
        "NumpadEnter" => extended(0x1c),
        "ControlRight" => extended(0x1d),
        "NumpadDivide" => extended(0x35),
        "PrintScreen" => extended(0x37),
        "AltRight" => extended(0x38),
        "Home" => extended(0x47),
        "ArrowUp" => extended(0x48),
        "PageUp" => extended(0x49),
        "ArrowLeft" => extended(0x4b),
        "ArrowRight" => extended(0x4d),
        "End" => extended(0x4f),
        "ArrowDown" => extended(0x50),
        "PageDown" => extended(0x51),
        "Insert" => extended(0x52),
        "Delete" => extended(0x53),
        "MetaLeft" | "OSLeft" => extended(0x5b),
        "MetaRight" | "OSRight" => extended(0x5c),
        "ContextMenu" => extended(0x5d),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_signs_are_themselves() {
        assert_eq!(keysym("a", "KeyA"), Some(0x61));
        assert_eq!(keysym("A", "KeyA"), Some(0x41));
        assert_eq!(keysym(" ", "Space"), Some(0x20));
        assert_eq!(keysym("é", "Digit2"), Some(0xe9));
        assert_eq!(keysym("€", "KeyE"), Some(0x0100_20ac));
        assert_eq!(keysym("अ", "KeyD"), Some(0x0100_0905));
    }

    #[test]
    fn named_keys() {
        assert_eq!(keysym("Enter", "Enter"), Some(0xff0d));
        assert_eq!(keysym("Enter", "NumpadEnter"), Some(0xff8d));
        assert_eq!(keysym("Shift", "ShiftLeft"), Some(0xffe1));
        assert_eq!(keysym("Shift", "ShiftRight"), Some(0xffe2));
        assert_eq!(keysym("Control", "ControlRight"), Some(0xffe4));
        assert_eq!(keysym("Meta", "MetaLeft"), Some(0xffeb));
        assert_eq!(keysym("F1", "F1"), Some(0xffbe));
        assert_eq!(keysym("F12", "F12"), Some(0xffc9));
        assert_eq!(keysym("ArrowRight", "ArrowRight"), Some(0xff53));
    }

    #[test]
    fn keys_as_a_pc_keyboard_numbers_them() {
        assert_eq!(scancode("KeyA"), Some((0x1e, false)));
        assert_eq!(scancode("KeyQ"), Some((0x10, false)));
        assert_eq!(scancode("KeyZ"), Some((0x2c, false)));
        assert_eq!(scancode("Digit1"), Some((0x02, false)));
        assert_eq!(scancode("Digit0"), Some((0x0b, false)));
        assert_eq!(scancode("F1"), Some((0x3b, false)));
        assert_eq!(scancode("F10"), Some((0x44, false)));
        assert_eq!(scancode("F12"), Some((0x58, false)));
        assert_eq!(scancode("Enter"), Some((0x1c, false)));
        // The same number as another key, told apart as extended.
        assert_eq!(scancode("NumpadEnter"), Some((0x1c, true)));
        assert_eq!(scancode("ControlRight"), Some((0x1d, true)));
        assert_eq!(scancode("Delete"), Some((0x53, true)));
        assert_eq!(scancode("NumpadDecimal"), Some((0x53, false)));
        assert_eq!(scancode("MetaLeft"), Some((0x5b, true)));
        for none in ["", "KeyÄ", "Keyab", "Digit12", "F13", "Fn", "Pause"] {
            assert_eq!(scancode(none), None, "{none}");
        }
        // Every letter has a number of its own.
        let mut seen = std::collections::HashSet::new();
        for c in b'A'..=b'Z' {
            assert!(seen.insert(scancode(&format!("Key{}", c as char)).unwrap()));
        }
    }

    #[test]
    fn keys_without_a_symbol() {
        assert_eq!(keysym("Dead", "Quote"), None);
        assert_eq!(keysym("Unidentified", ""), None);
        assert_eq!(keysym("F99", "F99"), None);
        assert_eq!(keysym("Fn", "Fn"), None);
    }
}

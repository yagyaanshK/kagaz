//! Keys, from how the window's page names them to what a session sends.
//!
//! The page gives each key press twice over, the way browsers do: `key`,
//! what the press means with the layout and Shift applied ("a", "A", "é",
//! "Enter"), and `code`, which key of the board it was ("KeyA",
//! "ShiftRight"). VNC wants the meaning, as an X11 key symbol.

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
    fn keys_without_a_symbol() {
        assert_eq!(keysym("Dead", "Quote"), None);
        assert_eq!(keysym("Unidentified", ""), None);
        assert_eq!(keysym("F99", "F99"), None);
        assert_eq!(keysym("Fn", "Fn"), None);
    }
}

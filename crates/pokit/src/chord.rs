//! Key chords written with physical key names (`Ctrl+Equal`), resolved to the
//! fields CDP's `Input.dispatchKeyEvent` takes, on a US layout.

pub const ALT: u32 = 1;
pub const CTRL: u32 = 2;
pub const META: u32 = 4;
pub const SHIFT: u32 = 8;

/// One key press as CDP describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyPress {
    pub code: &'static str,
    pub key: String,
    pub vk: u32,
    pub modifiers: u32,
    /// The text the press produces; `None` for a chord with Ctrl, Alt or Meta, or a key that types nothing.
    pub text: Option<String>,
}

impl KeyPress {
    /// The key-down event CDP's `Input.dispatchKeyEvent` takes; `repeat` marks an auto-repeat.
    pub fn down_event(&self, repeat: bool) -> serde_json::Value {
        let mut down = serde_json::json!({
            "type": if self.text.is_some() { "keyDown" } else { "rawKeyDown" },
            "key": self.key,
            "code": self.code,
            "windowsVirtualKeyCode": self.vk,
            "modifiers": self.modifiers,
            "autoRepeat": repeat,
        });
        if let Some(t) = &self.text {
            down["text"] = serde_json::json!(t);
            down["unmodifiedText"] = serde_json::json!(t);
        }
        down
    }

    /// The key-up event CDP's `Input.dispatchKeyEvent` takes.
    pub fn up_event(&self) -> serde_json::Value {
        serde_json::json!({ "type": "keyUp", "key": self.key, "code": self.code,
                            "windowsVirtualKeyCode": self.vk, "modifiers": self.modifiers })
    }
}

/// A physical key: its `code`, the key value unshifted and shifted, and its Windows virtual key code.
struct Key {
    code: &'static str,
    key: &'static str,
    shifted: &'static str,
    vk: u32,
}

const fn k(code: &'static str, key: &'static str, shifted: &'static str, vk: u32) -> Key {
    Key {
        code,
        key,
        shifted,
        vk,
    }
}

/// Keys that type a character, on a US layout.
const PRINTABLE: &[Key] = &[
    k("KeyA", "a", "A", 65),
    k("KeyB", "b", "B", 66),
    k("KeyC", "c", "C", 67),
    k("KeyD", "d", "D", 68),
    k("KeyE", "e", "E", 69),
    k("KeyF", "f", "F", 70),
    k("KeyG", "g", "G", 71),
    k("KeyH", "h", "H", 72),
    k("KeyI", "i", "I", 73),
    k("KeyJ", "j", "J", 74),
    k("KeyK", "k", "K", 75),
    k("KeyL", "l", "L", 76),
    k("KeyM", "m", "M", 77),
    k("KeyN", "n", "N", 78),
    k("KeyO", "o", "O", 79),
    k("KeyP", "p", "P", 80),
    k("KeyQ", "q", "Q", 81),
    k("KeyR", "r", "R", 82),
    k("KeyS", "s", "S", 83),
    k("KeyT", "t", "T", 84),
    k("KeyU", "u", "U", 85),
    k("KeyV", "v", "V", 86),
    k("KeyW", "w", "W", 87),
    k("KeyX", "x", "X", 88),
    k("KeyY", "y", "Y", 89),
    k("KeyZ", "z", "Z", 90),
    k("Digit0", "0", ")", 48),
    k("Digit1", "1", "!", 49),
    k("Digit2", "2", "@", 50),
    k("Digit3", "3", "#", 51),
    k("Digit4", "4", "$", 52),
    k("Digit5", "5", "%", 53),
    k("Digit6", "6", "^", 54),
    k("Digit7", "7", "&", 55),
    k("Digit8", "8", "*", 56),
    k("Digit9", "9", "(", 57),
    k("Space", " ", " ", 32),
    k("Minus", "-", "_", 189),
    k("Equal", "=", "+", 187),
    k("BracketLeft", "[", "{", 219),
    k("BracketRight", "]", "}", 221),
    k("Backslash", "\\", "|", 220),
    k("Semicolon", ";", ":", 186),
    k("Quote", "'", "\"", 222),
    k("Backquote", "`", "~", 192),
    k("Comma", ",", "<", 188),
    k("Period", ".", ">", 190),
    k("Slash", "/", "?", 191),
];

/// Keys that type nothing, except Enter's carriage return.
const NAMED: &[Key] = &[
    k("Enter", "Enter", "Enter", 13),
    k("Escape", "Escape", "Escape", 27),
    k("Tab", "Tab", "Tab", 9),
    k("Backspace", "Backspace", "Backspace", 8),
    k("Delete", "Delete", "Delete", 46),
    k("Insert", "Insert", "Insert", 45),
    k("Home", "Home", "Home", 36),
    k("End", "End", "End", 35),
    k("PageUp", "PageUp", "PageUp", 33),
    k("PageDown", "PageDown", "PageDown", 34),
    k("ArrowUp", "ArrowUp", "ArrowUp", 38),
    k("ArrowDown", "ArrowDown", "ArrowDown", 40),
    k("ArrowLeft", "ArrowLeft", "ArrowLeft", 37),
    k("ArrowRight", "ArrowRight", "ArrowRight", 39),
    k("F1", "F1", "F1", 112),
    k("F2", "F2", "F2", 113),
    k("F3", "F3", "F3", 114),
    k("F4", "F4", "F4", 115),
    k("F5", "F5", "F5", 116),
    k("F6", "F6", "F6", 117),
    k("F7", "F7", "F7", 118),
    k("F8", "F8", "F8", 119),
    k("F9", "F9", "F9", 120),
    k("F10", "F10", "F10", 121),
    k("F11", "F11", "F11", 122),
    k("F12", "F12", "F12", 123),
];

fn modifier(name: &str) -> Option<u32> {
    match name {
        "Ctrl" | "Control" => Some(CTRL),
        "Alt" | "Option" => Some(ALT),
        "Shift" => Some(SHIFT),
        "Meta" | "Cmd" | "Command" | "Win" => Some(META),
        _ => None,
    }
}

fn press(key: &Key, modifiers: u32, printable: bool) -> KeyPress {
    let value = if modifiers & SHIFT != 0 {
        key.shifted
    } else {
        key.key
    };
    let text = if modifiers & (CTRL | ALT | META) != 0 {
        None
    } else if printable {
        Some(value.to_string())
    } else if key.code == "Enter" {
        Some("\r".to_string())
    } else {
        None
    };
    KeyPress {
        code: key.code,
        key: value.to_string(),
        vk: key.vk,
        modifiers,
        text,
    }
}

/// Parses `Mod+Mod+Code`, e.g. `Ctrl+Shift+KeyT`.
pub fn parse_chord(chord: &str) -> Result<KeyPress, String> {
    let parts: Vec<&str> = chord.split('+').collect();
    let (main, mods) = match parts.split_last() {
        Some((main, mods)) if !main.is_empty() => (*main, mods),
        _ => return Err(format!("empty key in chord `{chord}`")),
    };
    let mut modifiers = 0;
    for m in mods {
        modifiers |=
            modifier(m).ok_or_else(|| format!("unknown modifier `{m}` in chord `{chord}`"))?;
    }
    if let Some(key) = PRINTABLE.iter().find(|k| k.code == main) {
        return Ok(press(key, modifiers, true));
    }
    if let Some(key) = NAMED.iter().find(|k| k.code == main) {
        return Ok(press(key, modifiers, false));
    }
    match PRINTABLE
        .iter()
        .find(|k| k.key == main || k.shifted == main)
    {
        Some(key) => Err(format!(
            "`{main}` is a character; write the physical key `{}`",
            key.code
        )),
        None => Err(format!("unknown key `{main}` in chord `{chord}`")),
    }
}

/// The press that types `c` on a US layout, with Shift where the character needs it.
pub fn key_for_char(c: char) -> Option<KeyPress> {
    let s = c.to_string();
    if let Some(key) = PRINTABLE.iter().find(|k| k.key == s) {
        return Some(press(key, 0, true));
    }
    PRINTABLE
        .iter()
        .find(|k| k.shifted == s)
        .map(|key| press(key, SHIFT, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_equal_matches_the_values_penterm_sends() {
        let k = parse_chord("Ctrl+Equal").unwrap();
        assert_eq!(
            (k.key.as_str(), k.code, k.vk, k.modifiers),
            ("=", "Equal", 187, 2)
        );
        assert_eq!(k.text, None);
    }

    #[test]
    fn ctrl_alt_combine_into_one_modifier_mask() {
        let k = parse_chord("Ctrl+Alt+Equal").unwrap();
        assert_eq!(k.modifiers, 3);
    }

    #[test]
    fn shift_selects_the_shifted_key_and_types_it() {
        let k = parse_chord("Shift+KeyT").unwrap();
        assert_eq!(
            (k.key.as_str(), k.code, k.vk, k.modifiers),
            ("T", "KeyT", 84, 8)
        );
        assert_eq!(k.text.as_deref(), Some("T"));
    }

    #[test]
    fn enter_types_a_carriage_return() {
        let k = parse_chord("Enter").unwrap();
        assert_eq!((k.key.as_str(), k.vk, k.modifiers), ("Enter", 13, 0));
        assert_eq!(k.text.as_deref(), Some("\r"));
    }

    #[test]
    fn meta_is_bit_four() {
        assert_eq!(parse_chord("Meta+KeyK").unwrap().modifiers, 4);
    }

    #[test]
    fn a_character_instead_of_a_key_name_is_refused_with_the_name_to_use() {
        let err = parse_chord("Ctrl+=").unwrap_err();
        assert!(err.contains("Equal"), "{err}");
    }

    #[test]
    fn an_unknown_key_is_refused_by_name() {
        let err = parse_chord("Ctrl+Foo").unwrap_err();
        assert!(err.contains("Foo"), "{err}");
    }

    #[test]
    fn typing_a_capital_letter_presses_shift() {
        let k = key_for_char('A').unwrap();
        assert_eq!(
            (k.code, k.modifiers, k.text.as_deref()),
            ("KeyA", 8, Some("A"))
        );
    }

    #[test]
    fn a_character_off_the_us_layout_has_no_key() {
        assert!(key_for_char('한').is_none());
    }
}

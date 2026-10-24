//! Key chords written with physical key names (`Ctrl+Equal`), resolved to the
//! fields CDP's `Input.dispatchKeyEvent` takes, and to the key events the macOS plugin
//! delivers, on a US layout.

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
    /// The macOS virtual key code.
    pub mac: u16,
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

    /// The `code` of each key-down a press sends: its modifiers in the order `modifier_events`
    /// presses them, then the key.
    pub fn down_codes(&self) -> Vec<String> {
        let mut codes: Vec<String> = self
            .modifier_events()
            .0
            .iter()
            .map(|e| e["code"].as_str().unwrap_or("").to_string())
            .collect();
        codes.push(self.code.to_string());
        codes
    }

    /// The key-up event CDP's `Input.dispatchKeyEvent` takes.
    pub fn up_event(&self) -> serde_json::Value {
        serde_json::json!({ "type": "keyUp", "key": self.key, "code": self.code,
                            "windowsVirtualKeyCode": self.vk, "modifiers": self.modifiers })
    }

    /// The chord's modifiers as keys of their own, the way a keyboard sends them: the key-downs
    /// in the order Control, Alt, Shift, Meta, each carrying the modifiers held so far, and the
    /// key-ups in reverse, each without its own.
    pub fn modifier_events(&self) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
        const MODS: [(u32, &str, &str, u32); 4] = [
            (CTRL, "Control", "ControlLeft", 17),
            (ALT, "Alt", "AltLeft", 18),
            (SHIFT, "Shift", "ShiftLeft", 16),
            (META, "Meta", "MetaLeft", 91),
        ];
        let mut held = 0;
        let mut downs = Vec::new();
        let mut ups = Vec::new();
        for (bit, key, code, vk) in MODS.iter().filter(|(bit, ..)| self.modifiers & bit != 0) {
            held |= bit;
            downs.push(
                serde_json::json!({ "type": "rawKeyDown", "key": key, "code": code,
                                           "windowsVirtualKeyCode": vk, "modifiers": held }),
            );
            ups.push((*bit, *key, *code, *vk));
        }
        let ups = ups
            .into_iter()
            .rev()
            .map(|(bit, key, code, vk)| {
                held &= !bit;
                serde_json::json!({ "type": "keyUp", "key": key, "code": code,
                                    "windowsVirtualKeyCode": vk, "modifiers": held })
            })
            .collect();
        (downs, ups)
    }
}

/// The modifiers in the order a press holds them: bit, `code`, macOS key code, and the macOS
/// event flag (`NSEventModifierFlags`).
const MAC_MODS: [(u32, &str, u16, u64); 4] = [
    (CTRL, "ControlLeft", 59, 1 << 18),
    (ALT, "AltLeft", 58, 1 << 19),
    (SHIFT, "ShiftLeft", 56, 1 << 17),
    (META, "MetaLeft", 55, 1 << 20),
];

impl KeyPress {
    /// The events the macOS plugin delivers for this press, as a keyboard sends them: a modifier
    /// change per modifier, in the order Control, Alt, Shift, Meta, the key-down and key-up with
    /// every modifier's flag, then the modifiers released in reverse.
    pub fn mac_events(&self) -> Vec<serde_json::Value> {
        let mods: Vec<_> = MAC_MODS
            .iter()
            .filter(|(bit, ..)| self.modifiers & bit != 0)
            .collect();
        let mut flags = 0;
        let mut events = Vec::new();
        for (_, _, key_code, flag) in &mods {
            flags |= flag;
            events
                .push(serde_json::json!({ "type": "flags", "key_code": key_code, "flags": flags }));
        }
        let (characters, unmodified) = self.mac_characters();
        let key = |kind: &str| {
            serde_json::json!({ "type": kind, "key_code": self.mac, "characters": characters,
                                "unmodified": unmodified, "flags": flags })
        };
        events.push(key("keydown"));
        events.push(key("keyup"));
        for (_, _, key_code, flag) in mods.iter().rev() {
            flags &= !flag;
            events
                .push(serde_json::json!({ "type": "flags", "key_code": key_code, "flags": flags }));
        }
        events
    }

    /// The `code` of each keydown the page sees for `mac_events`: its modifiers, then the key.
    /// The page names key code 114 `Help`, since a Mac keyboard has Help where Insert would be.
    pub fn mac_down_codes(&self) -> Vec<String> {
        let mut codes: Vec<String> = MAC_MODS
            .iter()
            .filter(|(bit, ..)| self.modifiers & bit != 0)
            .map(|(_, code, ..)| code.to_string())
            .collect();
        codes.push(
            if self.code == "Insert" {
                "Help"
            } else {
                self.code
            }
            .to_string(),
        );
        codes
    }

    /// The event's `characters` and `charactersIgnoringModifiers`, as a keyboard gives them: a
    /// letter with Control is its control character, a key that types nothing is AppKit's
    /// function-key character, and the rest is the key's value.
    fn mac_characters(&self) -> (String, String) {
        let unmodified = match self.code {
            "Enter" => "\r".to_string(),
            "Escape" => "\u{1b}".to_string(),
            "Tab" => "\t".to_string(),
            "Backspace" => "\u{7f}".to_string(),
            _ => match function_key(self.code) {
                Some(c) => c.to_string(),
                None => self.key.clone(),
            },
        };
        let mut chars = unmodified.chars();
        let characters = match (chars.next(), chars.next()) {
            (Some(c), None) if self.modifiers & CTRL != 0 && c.is_ascii_alphabetic() => {
                char::from(c.to_ascii_uppercase() as u8 - b'@').to_string()
            }
            _ => unmodified.clone(),
        };
        (characters, unmodified)
    }
}

/// AppKit's character for a key that types nothing (`NSUpArrowFunctionKey` and the rest).
fn function_key(code: &str) -> Option<char> {
    let n = match code {
        "ArrowUp" => 0xF700,
        "ArrowDown" => 0xF701,
        "ArrowLeft" => 0xF702,
        "ArrowRight" => 0xF703,
        "Insert" => 0xF727,
        "Delete" => 0xF728,
        "Home" => 0xF729,
        "End" => 0xF72B,
        "PageUp" => 0xF72C,
        "PageDown" => 0xF72D,
        _ => match code.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
            Some(f @ 1..=12) => 0xF703 + f,
            _ => return None,
        },
    };
    char::from_u32(n)
}

/// A physical key: its `code`, the key value unshifted and shifted, and its Windows and macOS
/// virtual key codes.
struct Key {
    code: &'static str,
    key: &'static str,
    shifted: &'static str,
    vk: u32,
    mac: u16,
}

const fn k(code: &'static str, key: &'static str, shifted: &'static str, vk: u32, mac: u16) -> Key {
    Key {
        code,
        key,
        shifted,
        vk,
        mac,
    }
}

/// Keys that type a character, on a US layout.
const PRINTABLE: &[Key] = &[
    k("KeyA", "a", "A", 65, 0),
    k("KeyB", "b", "B", 66, 11),
    k("KeyC", "c", "C", 67, 8),
    k("KeyD", "d", "D", 68, 2),
    k("KeyE", "e", "E", 69, 14),
    k("KeyF", "f", "F", 70, 3),
    k("KeyG", "g", "G", 71, 5),
    k("KeyH", "h", "H", 72, 4),
    k("KeyI", "i", "I", 73, 34),
    k("KeyJ", "j", "J", 74, 38),
    k("KeyK", "k", "K", 75, 40),
    k("KeyL", "l", "L", 76, 37),
    k("KeyM", "m", "M", 77, 46),
    k("KeyN", "n", "N", 78, 45),
    k("KeyO", "o", "O", 79, 31),
    k("KeyP", "p", "P", 80, 35),
    k("KeyQ", "q", "Q", 81, 12),
    k("KeyR", "r", "R", 82, 15),
    k("KeyS", "s", "S", 83, 1),
    k("KeyT", "t", "T", 84, 17),
    k("KeyU", "u", "U", 85, 32),
    k("KeyV", "v", "V", 86, 9),
    k("KeyW", "w", "W", 87, 13),
    k("KeyX", "x", "X", 88, 7),
    k("KeyY", "y", "Y", 89, 16),
    k("KeyZ", "z", "Z", 90, 6),
    k("Digit0", "0", ")", 48, 29),
    k("Digit1", "1", "!", 49, 18),
    k("Digit2", "2", "@", 50, 19),
    k("Digit3", "3", "#", 51, 20),
    k("Digit4", "4", "$", 52, 21),
    k("Digit5", "5", "%", 53, 23),
    k("Digit6", "6", "^", 54, 22),
    k("Digit7", "7", "&", 55, 26),
    k("Digit8", "8", "*", 56, 28),
    k("Digit9", "9", "(", 57, 25),
    k("Space", " ", " ", 32, 49),
    k("Minus", "-", "_", 189, 27),
    k("Equal", "=", "+", 187, 24),
    k("BracketLeft", "[", "{", 219, 33),
    k("BracketRight", "]", "}", 221, 30),
    k("Backslash", "\\", "|", 220, 42),
    k("Semicolon", ";", ":", 186, 41),
    k("Quote", "'", "\"", 222, 39),
    k("Backquote", "`", "~", 192, 50),
    k("Comma", ",", "<", 188, 43),
    k("Period", ".", ">", 190, 47),
    k("Slash", "/", "?", 191, 44),
];

/// Keys that type nothing, except Enter's carriage return.
const NAMED: &[Key] = &[
    k("Enter", "Enter", "Enter", 13, 36),
    k("Escape", "Escape", "Escape", 27, 53),
    k("Tab", "Tab", "Tab", 9, 48),
    k("Backspace", "Backspace", "Backspace", 8, 51),
    k("Delete", "Delete", "Delete", 46, 117),
    k("Insert", "Insert", "Insert", 45, 114),
    k("Home", "Home", "Home", 36, 115),
    k("End", "End", "End", 35, 119),
    k("PageUp", "PageUp", "PageUp", 33, 116),
    k("PageDown", "PageDown", "PageDown", 34, 121),
    k("ArrowUp", "ArrowUp", "ArrowUp", 38, 126),
    k("ArrowDown", "ArrowDown", "ArrowDown", 40, 125),
    k("ArrowLeft", "ArrowLeft", "ArrowLeft", 37, 123),
    k("ArrowRight", "ArrowRight", "ArrowRight", 39, 124),
    k("F1", "F1", "F1", 112, 122),
    k("F2", "F2", "F2", 113, 120),
    k("F3", "F3", "F3", 114, 99),
    k("F4", "F4", "F4", 115, 118),
    k("F5", "F5", "F5", 116, 96),
    k("F6", "F6", "F6", 117, 97),
    k("F7", "F7", "F7", 118, 98),
    k("F8", "F8", "F8", 119, 100),
    k("F9", "F9", "F9", 120, 101),
    k("F10", "F10", "F10", 121, 109),
    k("F11", "F11", "F11", 122, 103),
    k("F12", "F12", "F12", 123, 111),
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
        mac: key.mac,
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
    fn a_press_sends_a_key_down_for_each_modifier_then_the_key() {
        assert_eq!(
            parse_chord("Ctrl+Shift+KeyK").unwrap().down_codes(),
            ["ControlLeft", "ShiftLeft", "KeyK"]
        );
        assert_eq!(parse_chord("Enter").unwrap().down_codes(), ["Enter"]);
    }

    #[test]
    fn modifiers_are_pressed_as_keys_in_order_and_released_in_reverse() {
        let p = parse_chord("Ctrl+Shift+KeyK").unwrap();
        let (downs, ups) = p.modifier_events();
        let keys = |v: &[serde_json::Value]| -> Vec<(String, u64)> {
            v.iter()
                .map(|e| {
                    (
                        e["key"].as_str().unwrap().to_string(),
                        e["modifiers"].as_u64().unwrap(),
                    )
                })
                .collect()
        };
        assert_eq!(
            keys(&downs),
            vec![("Control".into(), 2), ("Shift".into(), 10)]
        );
        assert_eq!(keys(&ups), vec![("Shift".into(), 2), ("Control".into(), 0)]);
        assert_eq!(downs[0]["code"], "ControlLeft");
        assert_eq!(ups[1]["type"], "keyUp");
        let (none, _) = parse_chord("KeyK").unwrap().modifier_events();
        assert!(none.is_empty());
    }

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
    fn a_mac_chord_changes_each_modifier_around_the_key_with_its_control_character() {
        use serde_json::json;
        let events = parse_chord("Ctrl+Shift+KeyT").unwrap().mac_events();
        let (ctrl, both) = (1u64 << 18, (1u64 << 18) | (1 << 17));
        let key = |kind: &str| {
            json!({ "type": kind, "key_code": 17, "characters": "\u{14}", "unmodified": "T",
                    "flags": both })
        };
        assert_eq!(
            events,
            vec![
                json!({ "type": "flags", "key_code": 59, "flags": ctrl }),
                json!({ "type": "flags", "key_code": 56, "flags": both }),
                key("keydown"),
                key("keyup"),
                json!({ "type": "flags", "key_code": 56, "flags": ctrl }),
                json!({ "type": "flags", "key_code": 59, "flags": 0 }),
            ]
        );
    }

    #[test]
    fn the_page_sees_the_same_codes_on_a_mac_as_on_windows_but_help_for_insert() {
        for key in PRINTABLE.iter().chain(NAMED) {
            for modifiers in [0, CTRL | ALT | SHIFT | META] {
                let p = press(key, modifiers, false);
                let mut want = p.down_codes();
                if key.code == "Insert" {
                    *want.last_mut().unwrap() = "Help".into();
                }
                assert_eq!(p.mac_down_codes(), want, "{}", key.code);
            }
        }
    }

    #[test]
    fn a_key_that_types_nothing_carries_appkits_character() {
        let chars = |c: &str| parse_chord(c).unwrap().mac_characters();
        assert_eq!(chars("ArrowUp"), ("\u{F700}".into(), "\u{F700}".into()));
        assert_eq!(chars("F12"), ("\u{F70F}".into(), "\u{F70F}".into()));
        assert_eq!(chars("Enter"), ("\r".into(), "\r".into()));
        assert_eq!(chars("Shift+KeyA"), ("A".into(), "A".into()));
    }

    #[test]
    fn a_character_off_the_us_layout_has_no_key() {
        assert!(key_for_char('한').is_none());
    }
}

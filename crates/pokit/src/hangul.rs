//! Korean typed the way a 2-Set (Dubeolsik) IME composes it: text split into the jamo keys a
//! person presses, and the composition and committed text after each key.

/// Initial consonants in syllable order.
const L: [char; 19] = [
    'ㄱ', 'ㄲ', 'ㄴ', 'ㄷ', 'ㄸ', 'ㄹ', 'ㅁ', 'ㅂ', 'ㅃ', 'ㅅ', 'ㅆ', 'ㅇ', 'ㅈ', 'ㅉ', 'ㅊ', 'ㅋ',
    'ㅌ', 'ㅍ', 'ㅎ',
];
/// Vowels in syllable order.
const V: [char; 21] = [
    'ㅏ', 'ㅐ', 'ㅑ', 'ㅒ', 'ㅓ', 'ㅔ', 'ㅕ', 'ㅖ', 'ㅗ', 'ㅘ', 'ㅙ', 'ㅚ', 'ㅛ', 'ㅜ', 'ㅝ', 'ㅞ',
    'ㅟ', 'ㅠ', 'ㅡ', 'ㅢ', 'ㅣ',
];
/// Final consonants in syllable order; index 0 is no final.
const T: [char; 28] = [
    '\0', 'ㄱ', 'ㄲ', 'ㄳ', 'ㄴ', 'ㄵ', 'ㄶ', 'ㄷ', 'ㄹ', 'ㄺ', 'ㄻ', 'ㄼ', 'ㄽ', 'ㄾ', 'ㄿ', 'ㅀ',
    'ㅁ', 'ㅂ', 'ㅄ', 'ㅅ', 'ㅆ', 'ㅇ', 'ㅈ', 'ㅊ', 'ㅋ', 'ㅌ', 'ㅍ', 'ㅎ',
];
/// Vowels typed as two keys.
const COMPOUND_V: [(char, char, char); 7] = [
    ('ㅗ', 'ㅏ', 'ㅘ'),
    ('ㅗ', 'ㅐ', 'ㅙ'),
    ('ㅗ', 'ㅣ', 'ㅚ'),
    ('ㅜ', 'ㅓ', 'ㅝ'),
    ('ㅜ', 'ㅔ', 'ㅞ'),
    ('ㅜ', 'ㅣ', 'ㅟ'),
    ('ㅡ', 'ㅣ', 'ㅢ'),
];
/// Finals typed as two keys.
const COMPOUND_T: [(char, char, char); 11] = [
    ('ㄱ', 'ㅅ', 'ㄳ'),
    ('ㄴ', 'ㅈ', 'ㄵ'),
    ('ㄴ', 'ㅎ', 'ㄶ'),
    ('ㄹ', 'ㄱ', 'ㄺ'),
    ('ㄹ', 'ㅁ', 'ㄻ'),
    ('ㄹ', 'ㅂ', 'ㄼ'),
    ('ㄹ', 'ㅅ', 'ㄽ'),
    ('ㄹ', 'ㅌ', 'ㄾ'),
    ('ㄹ', 'ㅍ', 'ㄿ'),
    ('ㄹ', 'ㅎ', 'ㅀ'),
    ('ㅂ', 'ㅅ', 'ㅄ'),
];
/// Each jamo key on a 2-Set layout: the physical key and whether Shift is held.
const KEYS: [(char, &str, bool); 33] = [
    ('ㅂ', "KeyQ", false),
    ('ㅈ', "KeyW", false),
    ('ㄷ', "KeyE", false),
    ('ㄱ', "KeyR", false),
    ('ㅅ', "KeyT", false),
    ('ㅛ', "KeyY", false),
    ('ㅕ', "KeyU", false),
    ('ㅑ', "KeyI", false),
    ('ㅐ', "KeyO", false),
    ('ㅔ', "KeyP", false),
    ('ㅁ', "KeyA", false),
    ('ㄴ', "KeyS", false),
    ('ㅇ', "KeyD", false),
    ('ㄹ', "KeyF", false),
    ('ㅎ', "KeyG", false),
    ('ㅗ', "KeyH", false),
    ('ㅓ', "KeyJ", false),
    ('ㅏ', "KeyK", false),
    ('ㅣ', "KeyL", false),
    ('ㅋ', "KeyZ", false),
    ('ㅌ', "KeyX", false),
    ('ㅊ', "KeyC", false),
    ('ㅍ', "KeyV", false),
    ('ㅠ', "KeyB", false),
    ('ㅜ', "KeyN", false),
    ('ㅡ', "KeyM", false),
    ('ㅃ', "KeyQ", true),
    ('ㅉ', "KeyW", true),
    ('ㄸ', "KeyE", true),
    ('ㄲ', "KeyR", true),
    ('ㅆ', "KeyT", true),
    ('ㅒ', "KeyO", true),
    ('ㅖ', "KeyP", true),
];

/// The physical key and Shift state that type `jamo` on a 2-Set layout.
pub fn key_for(jamo: char) -> Option<(&'static str, bool)> {
    KEYS.iter()
        .find(|(j, _, _)| *j == jamo)
        .map(|(_, code, shift)| (*code, *shift))
}

/// Whether `c` is typed through the IME: a precomposed syllable, or a lone jamo the IME types
/// as itself. A compound final such as ㄳ is not: its two keys type ㄱ and ㅅ.
pub fn is_composed(c: char) -> bool {
    if is_syllable(c) {
        return true;
    }
    let keys = keys_for(c);
    keys.iter().all(|k| key_for(*k).is_some()) && (keys.len() == 1 || is_vowel(c))
}

/// Whether `c` is a precomposed Hangul syllable.
pub fn is_syllable(c: char) -> bool {
    ('가'..='힣').contains(&c)
}

fn is_vowel(j: char) -> bool {
    V.contains(&j)
}

/// The keys a person presses to type `c`, in order.
pub fn keys_for(c: char) -> Vec<char> {
    if !('가'..='힣').contains(&c) {
        return match split(c, &COMPOUND_V)[..] {
            [_, _] => split(c, &COMPOUND_V),
            _ => split(c, &COMPOUND_T),
        };
    }
    let s = c as u32 - '가' as u32;
    let (l, v, t) = (
        L[(s / 588) as usize],
        V[(s % 588 / 28) as usize],
        T[(s % 28) as usize],
    );
    let mut keys = vec![l];
    keys.extend(split(v, &COMPOUND_V));
    if t != '\0' {
        keys.extend(split(t, &COMPOUND_T));
    }
    keys
}

fn split(j: char, compounds: &[(char, char, char)]) -> Vec<char> {
    compounds
        .iter()
        .find(|(_, _, c)| *c == j)
        .map(|(a, b, _)| vec![*a, *b])
        .unwrap_or_else(|| vec![j])
}

fn join(a: char, b: char, compounds: &[(char, char, char)]) -> Option<char> {
    compounds
        .iter()
        .find(|(x, y, _)| *x == a && *y == b)
        .map(|(_, _, c)| *c)
}

/// The syllable or lone jamo being composed.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Syllable {
    l: Option<char>,
    v: Option<char>,
    t: Option<char>,
}

impl Syllable {
    fn text(self) -> String {
        match (self.l, self.v, self.t) {
            (Some(l), Some(v), t) => {
                let li = L.iter().position(|x| *x == l).unwrap() as u32;
                let vi = V.iter().position(|x| *x == v).unwrap() as u32;
                let ti = t.map_or(0, |t| T.iter().position(|x| *x == t).unwrap() as u32);
                char::from_u32('가' as u32 + li * 588 + vi * 28 + ti)
                    .unwrap()
                    .to_string()
            }
            (Some(j), None, _) | (None, Some(j), _) => j.to_string(),
            _ => String::new(),
        }
    }

    fn is_empty(self) -> bool {
        self.l.is_none() && self.v.is_none()
    }
}

/// What one key did: text the IME committed, and the composition left after it.
#[derive(Debug, PartialEq)]
pub struct Step {
    pub committed: Option<String>,
    pub composing: String,
}

/// A 2-Set IME's composition state.
#[derive(Debug, Default)]
pub struct Composer {
    now: Syllable,
}

impl Composer {
    /// Presses the key for `jamo`.
    pub fn press(&mut self, jamo: char) -> Step {
        let mut committed = None;
        let commit = |s: Syllable, committed: &mut Option<String>| {
            if !s.is_empty() {
                *committed = Some(s.text());
            }
        };
        let now = self.now;
        self.now = if is_vowel(jamo) {
            match now {
                Syllable {
                    l: Some(_),
                    v: None,
                    ..
                } => Syllable {
                    v: Some(jamo),
                    ..now
                },
                Syllable {
                    v: Some(v),
                    t: None,
                    ..
                } => match join(v, jamo, &COMPOUND_V) {
                    Some(c) => Syllable { v: Some(c), ..now },
                    None => {
                        commit(now, &mut committed);
                        Syllable {
                            v: Some(jamo),
                            ..Syllable::default()
                        }
                    }
                },
                Syllable {
                    l: Some(l),
                    v: Some(v),
                    t: Some(t),
                } => {
                    let (kept, moved) = match COMPOUND_T.iter().find(|(_, _, c)| *c == t) {
                        Some((a, b, _)) => (Some(*a), *b),
                        None => (None, t),
                    };
                    commit(
                        Syllable {
                            l: Some(l),
                            v: Some(v),
                            t: kept,
                        },
                        &mut committed,
                    );
                    Syllable {
                        l: Some(moved),
                        v: Some(jamo),
                        t: None,
                    }
                }
                _ => Syllable {
                    v: Some(jamo),
                    ..Syllable::default()
                },
            }
        } else {
            match now {
                Syllable {
                    l: Some(_),
                    v: Some(_),
                    t: None,
                } if T.contains(&jamo) && jamo != '\0' => Syllable {
                    t: Some(jamo),
                    ..now
                },
                Syllable {
                    v: Some(_),
                    t: Some(t),
                    ..
                } if join(t, jamo, &COMPOUND_T).is_some() => Syllable {
                    t: join(t, jamo, &COMPOUND_T),
                    ..now
                },
                _ => {
                    commit(now, &mut committed);
                    Syllable {
                        l: Some(jamo),
                        ..Syllable::default()
                    }
                }
            }
        };
        Step {
            committed,
            composing: self.now.text(),
        }
    }

    /// Ends the composition; the text it commits, if any.
    pub fn finish(&mut self) -> Option<String> {
        let now = std::mem::take(&mut self.now);
        (!now.is_empty()).then(|| now.text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Types `text` key by key and returns every step, then what finishing commits.
    fn typed(text: &str) -> (Vec<(Option<String>, String)>, Option<String>) {
        let mut c = Composer::default();
        let steps = text
            .chars()
            .flat_map(keys_for)
            .map(|k| {
                let s = c.press(k);
                (s.committed, s.composing)
            })
            .collect();
        (steps, c.finish())
    }

    fn s(x: &str) -> String {
        x.to_string()
    }

    #[test]
    fn syllables_split_into_the_keys_a_person_presses() {
        assert_eq!(keys_for('한'), vec!['ㅎ', 'ㅏ', 'ㄴ']);
        assert_eq!(keys_for('괜'), vec!['ㄱ', 'ㅗ', 'ㅐ', 'ㄴ']);
        assert_eq!(keys_for('닭'), vec!['ㄷ', 'ㅏ', 'ㄹ', 'ㄱ']);
        assert_eq!(keys_for('ㅘ'), vec!['ㅗ', 'ㅏ']);
        assert_eq!(keys_for('a'), vec!['a']);
        assert!(is_composed('한') && is_composed('ㅘ') && is_composed('ㅋ'));
        assert!(!is_composed('a') && !is_composed('漢') && !is_composed('ㄳ'));
    }

    #[test]
    fn hangul_is_composed_one_syllable_after_another() {
        let (steps, last) = typed("한글");
        assert_eq!(
            steps,
            vec![
                (None, s("ㅎ")),
                (None, s("하")),
                (None, s("한")),
                (Some(s("한")), s("ㄱ")),
                (None, s("그")),
                (None, s("글")),
            ]
        );
        assert_eq!(last, Some(s("글")));
    }

    #[test]
    fn a_final_consonant_moves_to_the_next_syllable_when_a_vowel_follows() {
        let (steps, last) = typed("가나");
        assert_eq!(
            steps,
            vec![
                (None, s("ㄱ")),
                (None, s("가")),
                (None, s("간")),
                (Some(s("가")), s("나")),
            ]
        );
        assert_eq!(last, Some(s("나")));
    }

    #[test]
    fn a_compound_final_splits_when_a_vowel_follows() {
        let (steps, last) = typed("달가");
        assert_eq!(
            steps,
            vec![
                (None, s("ㄷ")),
                (None, s("다")),
                (None, s("달")),
                (None, s("닭")),
                (Some(s("달")), s("가")),
            ]
        );
        assert_eq!(last, Some(s("가")));
    }

    #[test]
    fn compound_vowels_build_up_over_two_keys() {
        let (steps, last) = typed("괜");
        assert_eq!(
            steps,
            vec![
                (None, s("ㄱ")),
                (None, s("고")),
                (None, s("괘")),
                (None, s("괜")),
            ]
        );
        assert_eq!(last, Some(s("괜")));
    }

    #[test]
    fn a_consonant_that_cannot_end_a_syllable_starts_the_next() {
        let (steps, _) = typed("가따");
        assert_eq!(steps[2], (Some(s("가")), s("ㄸ")));
    }

    #[test]
    fn every_jamo_key_is_on_the_layout() {
        for c in L.iter().chain(V.iter()) {
            for k in keys_for(*c) {
                assert!(key_for(k).is_some(), "{k} has no key");
            }
        }
        assert_eq!(key_for('ㅎ'), Some(("KeyG", false)));
        assert_eq!(key_for('ㄲ'), Some(("KeyR", true)));
    }
}

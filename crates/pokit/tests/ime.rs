//! Korean typed through IME composition over CDP, against the fixture app's IME field.

#![cfg(windows)]

mod common;

use common::*;

/// Types `text` into the IME field and returns its value and the field's events as
/// `type:data` strings.
fn typed(p: &Pokit, text: &str) -> (String, Vec<String>) {
    let r = p.run(&[
        "eval",
        "document.querySelector('#ime').value = ''; window.__events.length = 0; true",
    ]);
    assert_eq!(r.code, 0, "{}", r.out);
    let r = p.run(&["type", text, "--into", "#ime"]);
    assert_eq!(r.code, 0, "{}", r.out);
    let v = p
        .run(&[
            "eval",
            "({ value: document.querySelector('#ime').value, \
           events: window.__events.filter(e => e[2] === 'ime').map(e => e[1] + ':' + e[3]) })",
        ])
        .out["value"]
        .clone();
    let events = v["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e.as_str().unwrap().to_string())
        .collect();
    (v["value"].as_str().unwrap().to_string(), events)
}

fn of_kind(events: &[String], prefix: &str) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| e.strip_prefix(prefix).map(str::to_string))
        .collect()
}

/// Whether `want` appears in `seen` in this order, not necessarily next to each other.
fn in_order(seen: &[String], want: &[&str]) -> bool {
    let mut it = seen.iter();
    want.iter().all(|w| it.any(|s| s == w))
}

#[test]
fn hangul_is_composed_key_by_key_and_lands_as_typed() {
    let p = shared_fixture();
    let (value, events) = typed(&p, "한글");
    assert_eq!(value, "한글");
    assert_eq!(of_kind(&events, "compositionstart:").len(), 2, "{events:?}");
    assert_eq!(
        of_kind(&events, "compositionend:"),
        vec!["한", "글"],
        "{events:?}"
    );
    assert!(
        in_order(
            &of_kind(&events, "compositionupdate:"),
            &["ㅎ", "하", "한", "ㄱ", "그", "글"]
        ),
        "the composition did not go ㅎ 하 한 ㄱ 그 글: {events:?}"
    );
    assert_eq!(
        of_kind(&events, "keydown:"),
        vec!["Process"; 6],
        "each jamo key is a Process key-down: {events:?}"
    );
}

#[test]
fn a_final_consonant_moves_on_as_a_real_ime_moves_it() {
    let p = shared_fixture();
    let (value, events) = typed(&p, "가나");
    assert_eq!(value, "가나");
    assert!(
        in_order(
            &events,
            &[
                "compositionupdate:ㄱ",
                "compositionupdate:가",
                "compositionupdate:간",
                "compositionend:가",
                "compositionupdate:나",
                "compositionend:나",
            ]
        ),
        "간 was not split back into 가 + 나 when ㅏ came: {events:?}"
    );
}

#[test]
fn text_mixing_hangul_and_ascii_lands_in_order() {
    let p = shared_fixture();
    let (value, _) = typed(&p, "a한 b글.");
    assert_eq!(value, "a한 b글.");
}

#[test]
fn lone_jamo_land_as_typed_and_do_not_join_their_neighbours() {
    let p = shared_fixture();
    for text in ["네ㅋㅋ", "가ㄴ", "한ㅏ", "ㄱㅏ", "ㅗㅏ", "ㄳ", "ㅘ요"] {
        let (value, _) = typed(&p, text);
        assert_eq!(value, text);
    }
}

#[test]
fn composing_needs_no_focus_from_the_user() {
    let before = common::window::foreground();
    let p = Pokit::launch_fixture("ime-focus");
    let (value, events) = typed(&p, "한글");
    assert_eq!(value, "한글", "{events:?}");
    let after = common::window::foreground();
    assert_eq!(
        after,
        before,
        "the foreground moved from pid {} to pid {} (fixture {})",
        common::window::window_pid(before),
        common::window::window_pid(after),
        p.app_pid()
    );
}

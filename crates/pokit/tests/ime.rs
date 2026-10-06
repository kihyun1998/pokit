//! Korean typed through IME composition over CDP, against the fixture app's IME field.

mod common;

use common::*;
use serde_json::Value;

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

#[test]
fn hangul_is_composed_key_by_key_and_lands_as_typed() {
    let p = Pokit::launch_fixture("ime-compose");
    let (value, events) = typed(&p, "한글");
    assert_eq!(value, "한글");
    assert_eq!(of_kind(&events, "compositionstart:").len(), 2, "{events:?}");
    assert_eq!(
        of_kind(&events, "compositionend:"),
        vec!["한", "글"],
        "{events:?}"
    );
    let updates = of_kind(&events, "compositionupdate:");
    for (i, step) in ["ㅎ", "하", "한", "ㄱ", "그", "글"].iter().enumerate() {
        assert!(
            updates.iter().any(|u| u == step),
            "composition never showed {step} (step {i}): {events:?}"
        );
    }
    assert_eq!(
        of_kind(&events, "keydown:"),
        vec!["Process"; 6],
        "each jamo key is a Process key-down: {events:?}"
    );
}

#[test]
fn a_final_consonant_moves_on_as_a_real_ime_moves_it() {
    let p = Pokit::launch_fixture("ime-move");
    let (value, events) = typed(&p, "가나");
    assert_eq!(value, "가나");
    let updates = of_kind(&events, "compositionupdate:");
    let at = updates
        .iter()
        .position(|u| u == "간")
        .unwrap_or_else(|| panic!("the composition never held 간: {events:?}"));
    assert_eq!(
        of_kind(&events, "compositionend:")[0],
        "가",
        "간 was not split back into 가 when ㅏ came: {events:?}"
    );
    assert!(at < updates.len() - 1, "{events:?}");
}

#[test]
fn text_mixing_hangul_and_ascii_lands_in_order() {
    let p = Pokit::launch_fixture("ime-mixed");
    let (value, _) = typed(&p, "a한 b글.");
    assert_eq!(value, "a한 b글.");
}

#[test]
fn composing_needs_no_focus_from_the_user() {
    let before = common::window::foreground();
    assert_ne!(
        before, 0,
        "no window is in the foreground, so there is nothing to keep"
    );
    if !common::window::can_take_foreground("composing_needs_no_focus_from_the_user") {
        return;
    }
    let p = Pokit::launch_fixture("ime-focus");
    let (value, _) = typed(&p, "한글");
    assert_eq!(value, "한글");
    let after = common::window::foreground();
    assert_eq!(
        after,
        before,
        "the foreground moved from pid {} to pid {} (fixture {})",
        common::window::window_pid(before),
        common::window::window_pid(after),
        p.app_pid()
    );
    let caps: Value = p.run(&["capabilities"]).out;
    assert_eq!(caps["commands"]["type"]["supported"], true, "{caps}");
}

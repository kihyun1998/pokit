//! `--route os` and `window activate`, against the fixture app. These take the user's focus:
//! OS input needs the app in front.

#![cfg(windows)]

mod common;

use common::*;
use serde_json::Value;

fn events(p: &Pokit, prefix: &str) -> Vec<String> {
    let v = p
        .run(&[
            "eval",
            "JSON.stringify(window.__events.map(e => e[1] + ':' + e[3]))",
        ])
        .out["value"]
        .clone();
    serde_json::from_str::<Vec<String>>(v.as_str().unwrap())
        .unwrap()
        .into_iter()
        .filter(|e| e.starts_with(prefix))
        .collect()
}

fn clear_events(p: &Pokit) {
    assert_eq!(p.run(&["eval", "window.__events.length = 0"]).code, 0);
}

#[test]
fn os_input_to_an_app_not_in_front_is_refused_and_sends_nothing() {
    let p = Pokit::launch_fixture("os-guard");
    clear_events(&p);
    for args in [
        &["key", "KeyA", "--into", "#name", "--route", "os"][..],
        &["type", "abc", "--into", "#name", "--route", "os"][..],
        &["click", "#target", "--route", "os"][..],
    ] {
        let r = p.run(args);
        assert_eq!(r.code, 6, "{args:?}: {}", r.out);
        assert!(
            r.out["error"]["message"]
                .as_str()
                .unwrap()
                .contains("window activate"),
            "{}",
            r.out
        );
    }
    assert!(
        events(&p, "key").is_empty() && events(&p, "input").is_empty(),
        "refused OS input reached the page"
    );
    assert_eq!(p.read_text("#last-mouse"), "");
}

/// One test, because each case brings its own fixture to the front.
#[test]
fn os_input_reaches_the_page_as_a_hand_would_send_it() {
    let p = Pokit::launch_fixture("os-input");
    let r = p.run(&["window", "activate"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(r.out["frontmost"], true, "{}", r.out);

    clear_events(&p);
    let r = p.run(&["key", "Ctrl+KeyK", "--into", "#name", "--route", "os"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(
        eventually(2000, || p.read_text("#last-key") == "Ctrl+KeyK"),
        "the page did not see Ctrl+KeyK with its code: {}",
        p.read_text("#last-key")
    );
    assert_eq!(
        events(&p, "key"),
        vec!["keydown:Control", "keydown:k", "keyup:k", "keyup:Control"],
        "OS input presses the modifier as a key of its own"
    );

    let log: Value = p.run(&["logs"]).out["entries"].clone();
    let accelerators: Vec<&str> = log
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["text"].as_str())
        .filter(|t| t.starts_with("fixture-accelerator:"))
        .collect();
    assert!(
        accelerators.iter().any(|t| t.contains("vk=75")),
        "OS input did not raise WebView2's AcceleratorKeyPressed: {accelerators:?}"
    );
    let seen = accelerators.len();
    assert_eq!(p.run(&["key", "Ctrl+KeyK", "--into", "#name"]).code, 0);
    std::thread::sleep(std::time::Duration::from_millis(500));
    let after = p.run(&["logs"]).out["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["text"]
                .as_str()
                .is_some_and(|t| t.starts_with("fixture-accelerator:"))
        })
        .count();
    assert_eq!(
        after, seen,
        "a CDP key raised AcceleratorKeyPressed, which only OS input should"
    );

    let r = p.run(&["type", "ab 한c", "--into", "#other-input", "--route", "os"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(
        eventually(2000, || p.read_value("#other-input") == "ab 한c"),
        "OS typing gave {:?}",
        p.read_value("#other-input")
    );

    let r = p.run(&["click", "#target", "--route", "os"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(
        eventually(2000, || p.read_text("#last-mouse") == "click"),
        "an OS click at device pixel ratio {} missed the target",
        r.out["device_pixel_ratio"]
    );
    // Clicks closer together than the system's double-click time count as one sequence, as a
    // hand's would; the single click above would make this a triple.
    std::thread::sleep(std::time::Duration::from_millis(700));
    let r = p.run(&["click", "#target", "--route", "os", "--double"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(eventually(2000, || p.read_text("#last-mouse") == "dblclick"));
    let r = p.run(&["click", "#target", "--route", "os", "--right"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(eventually(2000, || p.read_text("#last-mouse") == "contextmenu"));
}

#[test]
fn capabilities_say_which_routes_take_focus() {
    let p = Pokit::new("os-capabilities");
    let c = p.run(&["capabilities"]).out["commands"].clone();
    for command in ["click", "key", "type"] {
        assert_eq!(
            c[command]["routes"]["cdp"]["takes_focus"], false,
            "{command}: {c}"
        );
        assert_eq!(
            c[command]["routes"]["cdp"]["default"], true,
            "{command}: {c}"
        );
        assert_eq!(
            c[command]["routes"]["os"]["takes_focus"], true,
            "{command}: {c}"
        );
    }
    assert_eq!(c["type"]["routes"]["cdp"]["ime_composition"], true, "{c}");
    assert_eq!(c["type"]["routes"]["os"]["ime_composition"], false, "{c}");
    assert_eq!(c["window"]["takes_focus"], true, "{c}");
}

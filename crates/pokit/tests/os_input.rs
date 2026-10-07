//! `--route os` and `window activate`, against the fixture app. These take the user's focus:
//! OS input needs the app in front.

#![cfg(windows)]

mod common;

use common::*;
use serde_json::Value;
use std::sync::Mutex;

/// Held by every test that brings its fixture to the front, so that two never fight for it.
static FRONT: Mutex<()> = Mutex::new(());

fn front() -> std::sync::MutexGuard<'static, ()> {
    FRONT.lock().unwrap_or_else(|e| e.into_inner())
}

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
    let _front = front();
    let p = Pokit::launch_fixture("os-guard");
    clear_events(&p);
    assert_eq!(p.run(&["eval", "window.scrollTo(0, 0); true"]).code, 0);
    for args in [
        &["key", "KeyA", "--into", "#name", "--route", "os"][..],
        &["type", "abc", "--into", "#name", "--route", "os"][..],
        &["click", "#show-later", "--route", "os"][..],
        &[
            "hold", "KeyA", "--count", "3", "--into", "#name", "--route", "os",
        ][..],
        &["wheel", "--notches", "1", "--route", "os"][..],
        &[
            "drag", "#drag-me", "--to-x", "5", "--to-y", "5", "--route", "os",
        ][..],
        &[
            "hold",
            "KeyA",
            "--count",
            "3",
            "--into",
            "#name",
            "--compare",
        ][..],
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
    assert_eq!(
        p.run(&["eval", "window.scrollY"]).out["value"],
        0,
        "a refused OS click scrolled the page"
    );
}

/// One test, because each case brings its own fixture to the front.
#[test]
fn os_input_reaches_the_page_as_a_hand_would_send_it() {
    let _front = front();
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
        accelerators.iter().any(|t| t.contains("vk=75"))
            && accelerators.iter().any(|t| t.contains("vk=17")),
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
    std::thread::sleep(window::double_click_time() + std::time::Duration::from_millis(200));
    let r = p.run(&["click", "#target", "--route", "os", "--double"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(eventually(2000, || p.read_text("#last-mouse") == "dblclick"));
    let r = p.run(&["click", "#target", "--route", "os", "--right"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(eventually(2000, || p.read_text("#last-mouse") == "contextmenu"));

    // With another of the app's windows in front, the main page would not get the input.
    assert_eq!(p.run(&["click", "#open-second"]).code, 0);
    assert!(eventually(5000, || p
        .run(&["targets", "--select", "second"])
        .code
        == 0));
    assert_eq!(p.run(&["targets", "--select", "0"]).code, 0);
    assert_eq!(p.run(&["window", "activate"]).code, 0);
    let r = p.run(&["targets", "--select", "second"]);
    assert_eq!(r.code, 0, "{}", r.out);
    let in_second = p.run(&["eval", "document.hasFocus()"]).out["value"] == true;
    assert_eq!(p.run(&["targets", "--select", "0"]).code, 0);
    if in_second {
        clear_events(&p);
        let r = p.run(&["key", "KeyA", "--route", "os"]);
        assert_eq!(
            r.code, 6,
            "OS input went ahead with another window in front: {}",
            r.out
        );
        assert!(
            events(&p, "key").is_empty(),
            "the main page got keys meant for no one"
        );
    } else {
        eprintln!("the second window did not come to the front; that case was not tried");
    }

    // With the app's own dialog in front, the input would go to the dialog.
    assert_eq!(p.run(&["native", "choose", "Help > Ask"]).code, 0);
    assert!(eventually(5000, || {
        p.run(&["native", "list"]).out["dialogs"]
            .as_array()
            .is_some_and(|d| !d.is_empty())
    }));
    let r = p.run(&["click", "#target", "--route", "os"]);
    assert_eq!(
        r.code, 6,
        "an OS click went ahead with a dialog in front: {}",
        r.out
    );
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
    assert_eq!(
        c["window"]["actions"]["activate"]["takes_focus"], true,
        "{c}"
    );
    assert_eq!(c["window"]["actions"]["move"]["takes_focus"], false, "{c}");
    assert_eq!(
        c["window"]["actions"]["resize"]["takes_focus"], false,
        "{c}"
    );
}

/// The scroller's position after `wheel` on `route`, from the top.
fn wheeled(p: &Pokit, notches: &str, route: &str) -> (Value, Vec<String>) {
    assert_eq!(
        p.run(&[
            "eval",
            "document.querySelector('#scroller').scrollTop = 0; window.__events.length = 0; true"
        ])
        .code,
        0
    );
    let r = p.run(&["wheel", "#scroller", "--notches", notches, "--route", route]);
    assert_eq!(r.code, 0, "{route}: {}", r.out);
    std::thread::sleep(std::time::Duration::from_millis(800));
    let top = p
        .run(&["eval", "document.querySelector('#scroller').scrollTop"])
        .out["value"]
        .clone();
    (top, events(p, "wheel"))
}

/// Holds `FRONT`, because each case brings its fixture to the front.
#[test]
fn hold_wheel_and_the_comparison_run_on_both_routes() {
    let _front = front();
    let p = Pokit::launch_fixture("os-b");
    assert_eq!(p.run(&["window", "activate"]).code, 0);

    clear_events(&p);
    assert_eq!(
        p.run(&[
            "eval",
            "window.__repeats = []; document.querySelector('#stall').addEventListener('keydown', \
             e => { if (e.code === 'KeyB') window.__repeats.push(e.repeat); }); true"
        ])
        .code,
        0
    );
    let r = p.run(&[
        "hold",
        "Ctrl+KeyB",
        "--count",
        "4",
        "--into",
        "#stall",
        "--route",
        "os",
    ]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(eventually(2000, || events(&p, "keyup").len() == 2));
    assert_eq!(
        events(&p, "key"),
        vec![
            "keydown:Control",
            "keydown:b",
            "keydown:b",
            "keydown:b",
            "keydown:b",
            "keyup:b",
            "keyup:Control"
        ],
        "an OS hold is the modifier down, repeated key-downs, then both up"
    );
    assert_eq!(
        p.run(&["eval", "window.__repeats"]).out["value"],
        serde_json::json!([false, true, true, true]),
        "Windows marks every key-down after the first as a repeat"
    );

    assert_eq!(
        p.run(&[
            "eval",
            "document.querySelector('#scroller').scrollIntoView({ block: 'center' }); true"
        ])
        .code,
        0
    );
    let (cdp_top, cdp_wheels) = wheeled(&p, "1", "cdp");
    let (os_top, os_wheels) = wheeled(&p, "1", "os");
    assert!(
        os_top.as_f64().unwrap() > 0.0,
        "one OS notch scrolled nothing"
    );
    assert_eq!(cdp_top, os_top, "one notch scrolls as far on either route");
    assert_eq!(cdp_wheels.len(), 1, "{cdp_wheels:?}");
    assert_eq!(
        cdp_wheels, os_wheels,
        "one notch is one wheel event of the same delta"
    );

    let r = p.run(&[
        "eval",
        "document.body.style.paddingBottom = '3000px'; \
         window.scrollTo(0, document.querySelector('#scroller').getBoundingClientRect().bottom \
         + scrollY + 100); document.querySelector('#scroller').getBoundingClientRect().bottom < 0",
    ]);
    assert_eq!(
        r.out["value"], true,
        "the scroller did not leave the view: {}",
        r.out
    );
    for route in ["cdp", "os"] {
        let r = p.run(&["wheel", "#scroller", "--notches", "1", "--route", route]);
        assert_eq!(
            r.code, 5,
            "{route}: a wheel over an element out of view: {}",
            r.out
        );
        let r = p.run(&[
            "wheel",
            "--x",
            "5",
            "--y",
            "100000",
            "--notches",
            "1",
            "--route",
            route,
        ]);
        assert_eq!(r.code, 5, "{route}: a wheel below the viewport: {}", r.out);
    }
    assert_eq!(
        p.run(&[
            "eval",
            "document.body.style.paddingBottom = ''; window.scrollTo(0, 0); true"
        ])
        .code,
        0
    );

    let r = p.run(&[
        "hold",
        "KeyA",
        "--count",
        "10",
        "--into",
        "#stall",
        "--compare",
    ]);
    assert_eq!(r.code, 0, "{}", r.out);
    for route in ["cdp", "os"] {
        assert_eq!(r.out[route]["latency"]["keys"], 10, "{route}: {}", r.out);
        assert!(
            r.out[route]["latency"]["p50_ms"].is_number(),
            "{route}: {}",
            r.out
        );
    }
    assert!(r.out["os_minus_cdp"]["p50_ms"].is_number(), "{}", r.out);
    assert!(r.out["clock"]["uncertainty_ms"].is_number(), "{}", r.out);

    assert_eq!(p.run(&["measure", "start"]).code, 0);
    let r = p.run(&[
        "hold",
        "KeyA",
        "--count",
        "2",
        "--into",
        "#stall",
        "--compare",
    ]);
    assert_eq!(
        r.code, 6,
        "--compare took over a running measurement: {}",
        r.out
    );
    let m = p.run(&["measure", "stop", "--quiet", "300"]);
    assert_eq!(m.code, 0, "the running measurement was lost: {}", m.out);
}

/// Holds `FRONT`, because the drag brings its fixture to the front.
#[test]
fn a_drag_out_of_the_window_through_the_os_opens_a_window_where_it_was_dropped() {
    let _front = front();
    let p = Pokit::launch_fixture("os-drag");
    assert_eq!(p.run(&["window", "move", "--x", "40", "--y", "40"]).code, 0);
    assert_eq!(p.run(&["window", "activate"]).code, 0);
    let width = p.run(&["eval", "innerWidth"]).out["value"]
        .as_f64()
        .unwrap();
    let beyond = format!("{}", width + 150.0);
    let r = p.run(&[
        "drag", "#drag-me", "--to-x", &beyond, "--to-y", "100", "--route", "os",
    ]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(
        eventually(5000, || p.read_text("#drag-result").starts_with("out at")),
        "the page did not see the drop outside its window: {:?}",
        p.read_text("#drag-result")
    );
    assert!(
        eventually(5000, || p.run(&["targets"]).out["targets"]
            .as_array()
            .is_some_and(|t| t.len() == 2)),
        "the dropped window is not among the targets: {}",
        p.run(&["targets"]).out
    );
    let pid = p.app_pid();
    assert!(
        common::window::windows_of(pid)
            .into_iter()
            .any(|h| common::window::title(h) == "pokit fixture - dropped"),
        "no window opened for the drop"
    );
}

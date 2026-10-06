//! The fixture app's window at launch, behind another window, minimized, and with its webview
//! hidden.

mod common;

use common::window::{self, Cover};
use common::*;
use serde_json::Value;
use std::time::Duration;

fn fixture_window(p: &Pokit) -> isize {
    let pid = p.app_pid();
    let mut found = Vec::new();
    assert!(
        eventually(5000, || {
            found = window::windows_of(pid);
            !found.is_empty()
        }),
        "the fixture has no visible window"
    );
    found[0]
}

/// Measures about 1.5 s with 10 keys held into the stall field.
fn measure_with_keys(p: &Pokit) -> Value {
    assert_eq!(p.run(&["measure", "start"]).code, 0);
    let hold = p.run(&["hold", "KeyA", "--count", "10", "--into", "#stall"]);
    assert_eq!(hold.code, 0, "{}", hold.out);
    std::thread::sleep(Duration::from_millis(1500));
    let m = p.run(&["measure", "stop", "--quiet", "300"]);
    assert_eq!(m.code, 0, "{}", m.out);
    m.out
}

/// The page stayed visible, kept painting and handled every key, as measured with Tauri 2.12.1
/// on WebView2 154 (docs/map/measuring-the-page.md).
fn assert_still_rendering(what: &str, m: &Value) {
    assert_eq!(
        m["hidden"]["was_hidden"], false,
        "{what}: the page was hidden; WebView2 or wry now hides it, so update          docs/map/measuring-the-page.md: {m}"
    );
    assert!(
        m["frames"]["count"].as_u64().unwrap() >= 30,
        "{what}: reported visible but painted few frames: {m}"
    );
    assert_eq!(m["keys"]["handled"], 10, "{what}: {m}");
}

/// Whether this test descends from the foreground app, so that a launch could take the
/// foreground at all; says why not when it does not.
fn can_take_foreground(test: &str) -> bool {
    let owner = window::window_pid(window::foreground());
    let can = window::ancestors(std::process::id()).contains(&owner);
    if !can {
        eprintln!(
            "{test}: the foreground app (pid {owner}) did not start this test, so launch could              not take the foreground either way; run it from the terminal in front to test anything"
        );
    }
    can
}

/// Windows passes the right to take the foreground down from the foreground process to the
/// processes it starts, so `launch` can only take it when this test descends from the
/// foreground app, as it does when run from the terminal the user is looking at. Both cases run
/// in this one test, because one launch taking the foreground would fail the other's check.
#[test]
fn launch_leaves_the_foreground_where_it_was() {
    let before = window::foreground();
    assert_ne!(
        before, 0,
        "no window is in the foreground, so there is nothing to keep"
    );
    if !can_take_foreground("launch_leaves_the_foreground_where_it_was") {
        return;
    }

    let p = Pokit::launch_fixture("window-focus");
    std::thread::sleep(Duration::from_millis(1500));
    let after = window::foreground();
    assert_eq!(
        after,
        before,
        "launch changed the foreground window to one of pid {}",
        window::window_pid(after)
    );
    assert_ne!(window::window_pid(after), p.app_pid());
    drop(p);

    let mut p = Pokit::new("window-take-focus");
    let exe = fixture_exe().to_str().unwrap().to_string();
    let r = p.run(&["launch", &exe, "--", "--take-focus"]);
    assert_eq!(r.code, 0, "{}", r.out);
    p.launched = Some(r.out.clone());
    std::thread::sleep(Duration::from_millis(1500));
    let given = r.out["foreground_given_back"].as_u64().unwrap_or(0);
    let refused = r.out["foreground_give_back_refused"].as_u64().unwrap_or(0);
    assert!(
        given + refused >= 1,
        "the app never took the foreground, so this proved nothing: {}",
        r.out
    );
    let after = window::foreground();
    if refused > 0 && after != before {
        eprintln!(
            "launch_leaves_the_foreground_where_it_was: Windows refused to give the foreground              back {refused} times (the user's input withdraws the right); best effort, not a failure"
        );
        return;
    }
    assert_eq!(
        after,
        before,
        "an app that took the foreground kept it (now pid {}); launch: {}",
        window::window_pid(after),
        r.out
    );
}

#[test]
fn a_covered_or_minimized_window_keeps_rendering_and_taking_keys() {
    let p = Pokit::launch_fixture("window-covered");
    let hwnd = fixture_window(&p);
    let r = window::rect(hwnd);
    {
        let _cover = Cover::over(r, 20);
        std::thread::sleep(Duration::from_millis(1000));
        for i in 1..5 {
            for j in 1..5 {
                let (x, y) = (
                    r.left + (r.right - r.left) * i / 5,
                    r.top + (r.bottom - r.top) * j / 5,
                );
                assert_ne!(
                    window::top_level_at(x, y),
                    hwnd,
                    "the fixture is not covered at ({x}, {y})"
                );
            }
        }
        let m = measure_with_keys(&p);
        assert_still_rendering("covered", &m);
    }

    window::minimize(hwnd);
    assert!(
        eventually(2000, || window::is_minimized(hwnd)),
        "the fixture did not minimize"
    );
    let m = measure_with_keys(&p);
    assert!(
        window::is_minimized(hwnd),
        "the fixture restored itself during the measurement"
    );
    assert_still_rendering("minimized", &m);
    window::restore(hwnd);
}

#[test]
fn a_hidden_webview_is_reported_and_its_gap_is_not_a_frame() {
    let p = Pokit::launch_fixture("window-hidden");
    assert_eq!(p.run(&["measure", "start"]).code, 0);
    std::thread::sleep(Duration::from_millis(300));
    let r = p.run(&[
        "eval",
        "window.__TAURI__.core.invoke('hide_webview_for', { ms: 800 }); true",
    ]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(
        eventually(2000, || {
            p.run(&["eval", "document.visibilityState"]).out["value"] == "hidden"
        }),
        "the page never became hidden"
    );
    assert!(
        eventually(3000, || {
            p.run(&["eval", "document.visibilityState"]).out["value"] == "visible"
        }),
        "the page never became visible again"
    );
    std::thread::sleep(Duration::from_millis(300));
    let m = p.run(&["measure", "stop", "--quiet", "300"]);
    assert_eq!(m.code, 0, "{}", m.out);
    let m = m.out;

    assert_eq!(m["hidden"]["was_hidden"], true, "{m}");
    let hidden = m["hidden"]["hidden_ms"].as_f64().unwrap();
    assert!(
        (600.0..1500.0).contains(&hidden),
        "hidden for {hidden} ms, not about 800: {m}"
    );
    let longest = m["frames"]["max_ms"].as_f64().unwrap();
    assert!(
        longest < hidden / 2.0,
        "a {longest} ms frame: the gap across the hidden period was read as a frame: {m}"
    );
}

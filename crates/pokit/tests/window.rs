//! The fixture app's window at launch, behind another window, minimized, and with its webview
//! hidden.

#![cfg(windows)]

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
        "{what}: the page was hidden; WebView2 or wry now hides it, so update docs/map/measuring-the-page.md: {m}"
    );
    assert!(
        m["frames"]["count"].as_u64().unwrap() >= 30,
        "{what}: reported visible but painted few frames: {m}"
    );
    assert_eq!(m["keys"]["handled"], 10, "{what}: {m}");
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
    if !window::can_take_foreground("launch_leaves_the_foreground_where_it_was") {
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
            "launch_leaves_the_foreground_where_it_was: Windows refused to give the foreground back {refused} times (the user's input withdraws the right); best effort, not a failure"
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

/// Where the current page is on the screen and how big, in CSS pixels, as it reports itself.
fn page_box(p: &Pokit) -> (f64, f64, f64, f64) {
    let r = p.run(&["eval", "[screenX, screenY, innerWidth, innerHeight]"]);
    assert_eq!(r.code, 0, "{}", r.out);
    let v: Vec<f64> = (0..4)
        .map(|i| r.out["value"][i].as_f64().unwrap())
        .collect();
    (v[0], v[1], v[2], v[3])
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1.0
}

#[test]
fn window_move_and_resize_reshape_the_page_without_taking_the_foreground() {
    let p = Pokit::launch_fixture("window-shape");
    let main = fixture_window(&p);
    let before = window::foreground();

    let a = p.run(&["window", "resize", "--width", "700", "--height", "500"]);
    assert_eq!(a.code, 0, "{}", a.out);
    let (_, _, w1, h1) = page_box(&p);
    assert_eq!(
        a.out["viewport"],
        serde_json::json!({ "width": w1, "height": h1 })
    );
    let b = p.run(&["window", "resize", "--width", "600", "--height", "450"]);
    assert_eq!(b.code, 0, "{}", b.out);
    let (_, _, w2, h2) = page_box(&p);
    assert!(
        near(w1 - w2, 100.0) && near(h1 - h2, 50.0),
        "the page did not shrink by what the window did: {w1}x{h1} -> {w2}x{h2}"
    );
    assert!(
        near(b.out["rect"]["width"].as_f64().unwrap(), 600.0),
        "{}",
        b.out
    );
    assert_eq!(
        b.out["viewport"],
        serde_json::json!({ "width": w2, "height": h2 })
    );

    let c = p.run(&["window", "move", "--x", "200", "--y", "150"]);
    assert_eq!(c.code, 0, "{}", c.out);
    let (x1, y1, _, _) = page_box(&p);
    let d = p.run(&["window", "move", "--x", "260", "--y", "190"]);
    assert_eq!(d.code, 0, "{}", d.out);
    let (x2, y2, w3, h3) = page_box(&p);
    assert!(
        near(x2 - x1, 60.0) && near(y2 - y1, 40.0),
        "the page did not move by what the window did: ({x1}, {y1}) -> ({x2}, {y2})"
    );
    assert_eq!((w3, h3), (w2, h2), "moving the window resized the page");
    assert!(
        near(d.out["rect"]["x"].as_f64().unwrap(), 260.0),
        "{}",
        d.out
    );

    let v = p.run(&[
        "window",
        "resize",
        "--width",
        "640",
        "--height",
        "400",
        "--viewport",
    ]);
    assert_eq!(v.code, 0, "{}", v.out);
    let (_, _, vw, vh) = page_box(&p);
    assert!(
        near(vw, 640.0) && near(vh, 400.0),
        "--viewport did not size the page: {vw}x{vh}"
    );
    let w2 = vw;
    for bad in [["--width", "NaN"], ["--width", "0"]] {
        let r = p.run(&["window", "resize", bad[0], bad[1], "--height", "300"]);
        assert_eq!(r.code, 2, "{bad:?}: {}", r.out);
    }
    let r = p.run(&["window", "move", "--x", "inf", "--y", "0"]);
    assert_eq!(r.code, 2, "{}", r.out);

    if before != main {
        assert_eq!(
            window::foreground(),
            before,
            "moving or resizing took the foreground"
        );
    }

    assert_eq!(p.run(&["click", "#open-second"]).code, 0);
    assert!(eventually(5000, || p.run(&["targets"]).out["targets"]
        .as_array()
        .is_some_and(|t| t.len() == 2)));
    assert_eq!(p.run(&["targets", "--select", "1"]).code, 0);
    let (_, _, sw, _) = page_box(&p);
    let e = p.run(&["window", "resize", "--width", "500", "--height", "400"]);
    assert_eq!(e.code, 0, "{}", e.out);
    let (_, _, sw2, _) = page_box(&p);
    assert!(
        !near(sw, sw2),
        "the second window was not resized: {sw} -> {sw2}"
    );
    assert_eq!(p.run(&["targets", "--select", "0"]).code, 0);
    assert_eq!(
        page_box(&p).2,
        w2,
        "resizing the second window resized the main one"
    );
    let second = window::windows_of(p.app_pid())
        .into_iter()
        .find(|&h| window::title(h) == "pokit fixture - second")
        .expect("the second window");
    let above = |upper: isize, lower: isize| {
        let order = window::windows_of(p.app_pid());
        let at = |h| order.iter().position(|&o| o == h).unwrap();
        at(upper) < at(lower)
    };
    assert!(above(second, main), "the second window did not open on top");
    let g = p.run(&[
        "window",
        "resize",
        "--width",
        "700",
        "--height",
        "500",
        "--viewport",
    ]);
    assert_eq!(g.code, 0, "{}", g.out);
    assert!(
        near(page_box(&p).2 - w2, 60.0),
        "with the second window on top, the main page's window was not the one resized: {}",
        g.out
    );
    assert!(
        above(second, main),
        "resizing the main window brought it above the second"
    );

    window::minimize(main);
    let f = p.run(&["window", "move", "--x", "100", "--y", "100"]);
    window::restore(main);
    assert_eq!(f.code, 1, "a minimized window was moved: {}", f.out);
    assert!(
        f.out["error"]["message"]
            .as_str()
            .unwrap()
            .contains("minimized"),
        "{}",
        f.out
    );
}

/// The PNG at `path`: its width, height and RGBA pixels.
fn pixels(path: &str) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(std::fs::File::open(path).unwrap());
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .chunks(3)
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect(),
        other => panic!("unexpected PNG colour type {other:?}"),
    };
    (info.width, info.height, rgba)
}

/// How many pixels two captures of the same size differ in.
fn differing(a: &(u32, u32, Vec<u8>), b: &(u32, u32, Vec<u8>)) -> usize {
    assert_eq!((a.0, a.1), (b.0, b.1), "the captures differ in size");
    a.2.chunks(4)
        .zip(b.2.chunks(4))
        .filter(|(x, y)| x != y)
        .count()
}

#[test]
fn a_window_capture_holds_the_apps_native_ui_and_nothing_of_other_apps() {
    let p = Pokit::launch_fixture("capture-window");
    let main = fixture_window(&p);
    let shot = |name: &str| {
        let out = p.home.join(name);
        let r = p.run(&["capture", "--window", "--out", out.to_str().unwrap()]);
        assert_eq!(r.code, 0, "{}", r.out);
        (pixels(out.to_str().unwrap()), r.out)
    };

    let (plain, out) = shot("plain.png");
    let white = plain
        .2
        .chunks(4)
        .filter(|c| c[..3] == [255, 255, 255])
        .count();
    assert!(
        white > (plain.0 * plain.1 / 3) as usize,
        "the page is not drawn in the capture ({white} white pixels of {}x{})",
        plain.0,
        plain.1
    );
    assert_eq!(out["windows"].as_array().map(Vec::len), Some(1), "{out}");

    let cover = Cover::over(window::rect(main), 0);
    let (covered, _) = shot("covered.png");
    drop(cover);
    let changed = differing(&plain, &covered);
    assert!(
        changed < (plain.0 * plain.1 / 100) as usize,
        "another app's window over the fixture got into its capture ({changed} pixels changed)"
    );

    assert_eq!(p.run(&["native", "choose", "Help > Ask"]).code, 0);
    assert!(eventually(5000, || p.run(&["native", "list"]).out
        ["dialogs"]
        .as_array()
        .is_some_and(|d| !d.is_empty())));
    let (with_dialog, out) = shot("dialog.png");
    let drawn = out["windows"].as_array().unwrap();
    let dialog = drawn
        .iter()
        .find(|w| w["title"] == "pokit fixture question")
        .unwrap_or_else(|| panic!("the dialog is not among the windows drawn: {out}"));
    let n = |k: &str| dialog["rect"][k].as_u64().unwrap() as u32;
    let (x, y, w, h) = (n("x"), n("y"), n("width"), n("height"));
    assert!(w > 50 && h > 50, "{out}");
    let changed_in_dialog = (y..y + h)
        .flat_map(|row| (x..x + w).map(move |col| ((row * plain.0 + col) * 4) as usize))
        .filter(|&i| plain.2[i..i + 4] != with_dialog.2[i..i + 4])
        .count();
    assert!(
        changed_in_dialog > (w * h / 5) as usize,
        "the dialog is listed but not drawn where it says ({changed_in_dialog} of {} pixels changed): {out}",
        w * h
    );
    let r = p.run(&["capture", "#submit", "--window"]);
    assert_eq!(r.code, 2, "an element and --window together: {}", r.out);
}

//! `native`, against the fixture app's menu bar and its yes-or-no dialog.

#![cfg(windows)]

mod common;

use common::*;
use serde_json::Value;
use std::sync::Mutex;
use std::time::Duration;

/// Held by the tests that look at the foreground, so that one's fixture does not move it under
/// another.
static FOCUS: Mutex<()> = Mutex::new(());

fn native_result(p: &Pokit) -> String {
    p.run(&["read", "#native-result"]).out["text"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

/// Waits for the fixture to have `n` dialogs open; returns them.
fn dialogs(p: &Pokit, n: usize) -> Vec<Value> {
    let mut found = Vec::new();
    assert!(
        eventually(5000, || {
            found = p.run(&["native", "list"]).out["dialogs"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            found.len() == n
        }),
        "the fixture did not come to have {n} dialogs open: {found:?}"
    );
    found
}

#[test]
fn the_menu_bar_is_read_and_its_entries_chosen() {
    let p = Pokit::launch_fixture("native-menu");
    let r = p.run(&["native", "list"]);
    assert_eq!(r.code, 0, "{}", r.out);
    let text = r.out["text"].as_str().unwrap();
    for line in [
        "- File",
        "- Say hello (Ctrl+H)",
        "- Not now [disabled]",
        "- Help",
        "- Ask",
    ] {
        assert!(text.contains(line), "`{line}` is not in the menu:\n{text}");
    }

    let r = p.run(&["native", "choose", "File > Say hello"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(
        eventually(3000, || native_result(&p) == "hello from the menu"),
        "choosing the entry did not run it"
    );

    let r = p.run(&["native", "choose", "File > Not now"]);
    assert_eq!(r.code, 1, "a disabled entry was chosen: {}", r.out);
    assert!(
        r.out["error"]["message"]
            .as_str()
            .unwrap()
            .contains("is disabled"),
        "{}",
        r.out
    );
    let r = p.run(&["native", "choose", "File > Nothing like this"]);
    assert_eq!(r.code, 5, "{}", r.out);
    let r = p.run(&["native", "choose", "File"]);
    assert_eq!(r.code, 1, "a submenu was chosen as an entry: {}", r.out);
    assert!(
        r.out["error"]["message"]
            .as_str()
            .unwrap()
            .contains("opens a submenu"),
        "{}",
        r.out
    );
}

#[test]
fn a_dialog_is_read_answered_and_closes_with_the_answer() {
    let p = Pokit::launch_fixture("native-dialog");
    assert_eq!(p.run(&["native", "choose", "Help > Ask"]).code, 0);
    let open = dialogs(&p, 1);
    assert_eq!(open[0]["title"], "pokit fixture question", "{open:?}");
    assert_eq!(open[0]["text"][0], "Proceed?", "{open:?}");
    let buttons: Vec<String> = open[0]["buttons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        buttons.len(),
        2,
        "a yes-or-no dialog lists two buttons, no title-bar ones: {buttons:?}"
    );

    // The dialog's button names follow the system's language ("Yes", "예(Y)"); the name before
    // its access key is enough.
    let yes = buttons[0].split('(').next().unwrap().trim().to_string();
    let r = p.run(&["native", "answer", &yes]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(
        r.out["closed"], true,
        "the press did not close the dialog: {}",
        r.out
    );
    assert!(
        eventually(3000, || native_result(&p) == "answered yes"),
        "the dialog did not close with the answer: {}",
        native_result(&p)
    );
    dialogs(&p, 0);

    assert_eq!(p.run(&["native", "choose", "Help > Ask"]).code, 0);
    dialogs(&p, 1);
    let r = p.run(&["native", "choose", "Help > Ask"]);
    assert_eq!(
        r.code, 1,
        "a menu behind a modal dialog was chosen: {}",
        r.out
    );
    assert!(
        r.out["error"]["message"]
            .as_str()
            .unwrap()
            .contains("modal dialog"),
        "{}",
        r.out
    );
    dialogs(&p, 1);
    let r = p.run(&["native", "answer", "Nothing like this"]);
    assert_eq!(r.code, 5, "{}", r.out);
    let no = buttons[1].split('(').next().unwrap().trim().to_string();
    assert_eq!(
        p.run(&[
            "native",
            "answer",
            &no,
            "--dialog",
            "pokit fixture question"
        ])
        .code,
        0
    );
    assert!(eventually(3000, || native_result(&p) == "answered no"));
}

#[test]
fn a_dialog_the_app_opens_does_not_keep_the_foreground() {
    let _focus = FOCUS.lock().unwrap_or_else(|e| e.into_inner());
    let before = window::foreground();
    let p = Pokit::launch_fixture("native-focus");
    if !window::can_take_foreground("a_dialog_the_app_opens_does_not_keep_the_foreground") {
        return;
    }
    assert_eq!(p.run(&["native", "choose", "Help > Ask"]).code, 0);
    dialogs(&p, 1);
    std::thread::sleep(Duration::from_millis(1000));
    let after = window::foreground();
    let log = p.run(&["logs"]).out["entries"].to_string();
    if after != before && log.contains("could not give it back") {
        eprintln!(
            "a_dialog_the_app_opens_does_not_keep_the_foreground: Windows refused to give the \
             foreground back (the user's input withdraws the right); best effort, not a failure"
        );
        return;
    }
    assert_eq!(
        after,
        before,
        "the dialog kept the foreground (pid {}); session log: {log}",
        window::window_pid(after)
    );
    assert!(
        log.contains("after a menu choice"),
        "the dialog never took the foreground, so this proved nothing: {log}"
    );
}

#[test]
fn an_attached_session_cannot_reach_the_native_ui() {
    let owner = Pokit::launch_fixture("native-owner");
    let port = owner.launched.as_ref().unwrap()["cdp_port"]
        .as_u64()
        .unwrap()
        .to_string();
    // The attached session is marked launched, so that dropping it closes it.
    let mut guest = Pokit::new("native-guest");
    let r = guest.run(&["attach", "--port", &port]);
    assert_eq!(r.code, 0, "{}", r.out);
    guest.launched = Some(r.out);
    let r = guest.run(&["native", "list"]);
    assert_eq!(r.code, 7, "{}", r.out);
}

/// Right-clicks the fixture's context target and waits for its context menu.
fn open_context_menu(p: &Pokit) -> Value {
    let r = p.run(&["click", "#context-target", "--right"]);
    assert_eq!(r.code, 0, "{}", r.out);
    let mut menu = Value::Null;
    assert!(
        eventually(5000, || {
            menu = p.run(&["native", "list"]).out["context_menu"].clone();
            !menu.is_null()
        }),
        "no context menu opened"
    );
    menu
}

fn context_menu_closed(p: &Pokit) -> bool {
    eventually(3000, || {
        p.run(&["native", "list"]).out["context_menu"].is_null()
    })
}

#[test]
fn a_context_menu_is_read_chosen_in_and_dismissed() {
    let _focus = FOCUS.lock().unwrap_or_else(|e| e.into_inner());
    let p = Pokit::launch_fixture("native-context");
    let before = window::foreground();
    // Whether the app took the foreground when its menu opened: only then is there anything to
    // give back.
    let took = || window::window_pid(window::foreground()) == p.app_pid();
    let came_back = |took: bool| !took || eventually(2500, || window::foreground() == before);
    assert!(p.run(&["native", "list"]).out["context_menu"].is_null());

    let menu = open_context_menu(&p);
    let took_first = took();
    let labels: Vec<_> = menu
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["label"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(labels, ["Mark here", "Cannot", "More"], "{menu}");
    assert_eq!(menu[1]["enabled"], false, "{menu}");
    assert_eq!(menu[2]["items"][0]["label"], "Deeper", "{menu}");

    let r = p.run(&["native", "choose", "More > Deeper"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(r.out["window"], "context menu", "{}", r.out);
    assert!(
        context_menu_closed(&p),
        "choosing left the context menu open"
    );
    assert!(
        came_back(took_first),
        "choosing in the context menu kept the foreground"
    );
    assert!(eventually(3000, || native_result(&p) == "deeper from the context menu"));

    open_context_menu(&p);
    let took_second = took();
    let r = p.run(&["native", "choose", "Cannot"]);
    assert_eq!(r.code, 1, "a disabled entry was chosen: {}", r.out);
    let r = p.run(&["native", "choose", "File > Say hello"]);
    assert_eq!(
        r.code, 5,
        "with a context menu open, a menu bar path was chosen: {}",
        r.out
    );
    let r = p.run(&["native", "dismiss"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(
        context_menu_closed(&p),
        "dismiss left the context menu open"
    );
    assert!(
        came_back(took_second),
        "dismissing the context menu kept the foreground"
    );
    eprintln!(
        "the app took the foreground at the first menu: {took_first}, the second: {took_second}"
    );
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(native_result(&p), "deeper from the context menu");

    open_context_menu(&p);
    assert_eq!(p.run(&["native", "choose", "Mark here"]).code, 0);
    assert!(eventually(3000, || native_result(&p) == "marked from the context menu"));
    let r = p.run(&["native", "dismiss"]);
    assert_eq!(r.code, 5, "dismiss with no context menu open: {}", r.out);
}

#[test]
fn the_tray_icon_is_clicked_and_its_menu_chosen_in() {
    let _focus = FOCUS.lock().unwrap_or_else(|e| e.into_inner());
    let mut p = Pokit::new("native-tray");
    let exe = fixture_exe().to_str().unwrap().to_string();
    let r = p.run(&["launch", &exe, "--", "--tray"]);
    assert_eq!(r.code, 0, "{}", r.out);
    p.launched = Some(r.out);
    let before = window::foreground();
    assert_eq!(p.run(&["native", "list"]).out["tray_icons"], 1);

    let r = p.run(&["native", "tray"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(
        eventually(3000, || native_result(&p) == "the tray icon was clicked"),
        "{:?}",
        native_result(&p)
    );
    assert_eq!(
        window::foreground(),
        before,
        "a left click on the tray icon moved the foreground"
    );

    let r = p.run(&["native", "tray", "--right"]);
    assert_eq!(r.code, 0, "{}", r.out);
    let mut menu = Value::Null;
    assert!(
        eventually(5000, || {
            menu = p.run(&["native", "list"]).out["context_menu"].clone();
            !menu.is_null()
        }),
        "a right click on the tray icon opened no menu"
    );
    let took = window::window_pid(window::foreground()) == p.app_pid();
    assert_eq!(menu[0]["label"], "Note from the tray", "{menu}");
    assert_eq!(menu[1]["enabled"], false, "{menu}");
    let r = p.run(&["native", "choose", "Note from the tray"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(eventually(3000, || native_result(&p) == "noted from the tray"));
    assert!(
        !took || eventually(2500, || window::foreground() == before),
        "the tray menu kept the foreground"
    );
    eprintln!("the app took the foreground at the tray menu: {took}");
}

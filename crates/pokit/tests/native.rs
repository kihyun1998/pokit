//! `native`, against the fixture app's menu bar and its yes-or-no dialog.

#![cfg(windows)]

mod common;

use common::*;
use serde_json::Value;
use std::time::Duration;

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
    let r = p.run(&["native", "choose", "File > Nothing like this"]);
    assert_eq!(r.code, 5, "{}", r.out);
    let r = p.run(&["native", "choose", "File"]);
    assert_eq!(r.code, 1, "a submenu was chosen as an entry: {}", r.out);
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

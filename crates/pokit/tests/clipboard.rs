//! `clipboard`, against the fixture app's clipboard field. The clipboard is one per desktop, so
//! every case runs in this one test, in order.

#![cfg(windows)]

mod common;

use common::*;
use std::process::Command;

fn powershell(script: &str) -> String {
    let out = Command::new("powershell")
        .args(["-NoProfile", "-STA", "-Command"])
        .arg(format!(
            "[Console]::OutputEncoding = [Text.Encoding]::UTF8; {script}"
        ))
        .output()
        .expect("powershell did not start");
    String::from_utf8_lossy(&out.stdout)
        .trim_end_matches(['\r', '\n'])
        .to_string()
}

/// The clipboard's text, as another program sees it.
fn clipboard_text() -> String {
    powershell("Get-Clipboard -Raw")
}

/// Puts text on the clipboard the way another program would.
fn set_clipboard_text(text: &str) {
    powershell(&format!(
        "Set-Clipboard -Value '{}'",
        text.replace('\'', "''")
    ));
    assert_eq!(clipboard_text(), text, "could not set the clipboard");
}

/// Gives the developer's clipboard text back when the test ends, however it ends.
struct KeepClipboard(String);

impl Drop for KeepClipboard {
    fn drop(&mut self) {
        if !self.0.is_empty() {
            powershell(&format!(
                "Set-Clipboard -Value '{}'",
                self.0.replace('\'', "''")
            ));
        }
    }
}

/// Puts text on the clipboard marked as not for monitoring, as a password manager does.
fn set_clipboard_text_not_for_monitoring(text: &str) {
    powershell(&format!(
        "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object Windows.Forms.DataObject; $d.SetText('{}'); $d.SetData('ExcludeClipboardContentFromMonitorProcessing', (New-Object IO.MemoryStream(,[byte[]](0,0,0,0)))); [Windows.Forms.Clipboard]::SetDataObject($d, $true)",
        text.replace('\'', "''")
    ));
    assert_eq!(clipboard_text(), text, "could not set the clipboard");
}

#[test]
fn the_clipboard_is_written_pasted_kept_out_of_history_and_given_back() {
    let _keep = KeepClipboard(clipboard_text());
    let before = format!("pokit-before-{}", std::process::id());
    set_clipboard_text(&before);

    let p = Pokit::launch_fixture("clipboard");
    set_clipboard_text_not_for_monitoring("hunter2-from-a-password-manager");
    let r = p.run(&["clipboard", "read"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(r.out["text"], serde_json::Value::Null, "{}", r.out);
    assert!(r.out["withheld"].is_string(), "{}", r.out);
    assert!(
        !p.all_written_text().contains("hunter2"),
        "a clipboard marked not for monitoring reached the run record"
    );
    set_clipboard_text(&before);
    let text = "pokit 한글 ✓";
    let r = p.run(&["clipboard", "write", text]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(r.out["saved_user_clipboard"], true, "{}", r.out);
    assert_eq!(
        clipboard_text(),
        text,
        "another program does not see the write"
    );
    assert_eq!(p.run(&["clipboard", "read"]).out["text"], text);
    let excluded = powershell(
        "Add-Type -AssemblyName System.Windows.Forms; \
         [Windows.Forms.Clipboard]::ContainsData('ExcludeClipboardContentFromMonitorProcessing')",
    );
    assert_eq!(
        excluded, "True",
        "the write is not kept out of clipboard history"
    );

    let r = p.run(&["key", "Ctrl+KeyV", "--into", "#clip"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(
        p.run(&["eval", "document.querySelector('#clip').value"])
            .out["value"],
        text,
        "pasting into the fixture did not give the written text"
    );

    let r = p.run(&["clipboard", "write", "second"]);
    assert_eq!(r.out["saved_user_clipboard"], false, "{}", r.out);
    assert_eq!(p.run(&["close"]).code, 0);
    let mut p = p;
    p.launched = None;
    assert_eq!(
        clipboard_text(),
        before,
        "close did not give back what the clipboard held before the first write"
    );

    let p = Pokit::launch_fixture("clipboard-taken");
    std::env::set_var("POKIT_TEST_CLIP_SECRET", "s3cr3t-clip");
    let r = p.run(&[
        "clipboard",
        "write",
        "--secret-env",
        "POKIT_TEST_CLIP_SECRET",
    ]);
    assert_eq!(r.code, 0, "{}", r.out);
    let read = p.run(&["clipboard", "read"]);
    assert_eq!(read.code, 0, "{}", read.out);
    assert_eq!(read.out["text"], "***", "{}", read.out);
    assert!(
        !read.out.to_string().contains("s3cr3t-clip"),
        "a secret written to the clipboard is printed: {}",
        read.out
    );
    set_clipboard_text("written by someone else");
    assert_eq!(p.run(&["close"]).code, 0);
    assert!(
        !p.all_written_text().contains("s3cr3t-clip"),
        "a secret written to the clipboard is in the run record"
    );
    let mut p = p;
    p.launched = None;
    assert_eq!(
        clipboard_text(),
        "written by someone else",
        "close overwrote what another program put on the clipboard"
    );
}

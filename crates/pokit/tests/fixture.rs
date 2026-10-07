//! pokit's command-line contract, against the fixture app.

#![cfg(windows)]

mod common;

use common::*;

#[test]
fn launch_lists_the_main_page_and_close_removes_the_instance() {
    let mut p = Pokit::launch_fixture("lifecycle");
    let app = p.app_pid();
    assert!(process_alive(app));

    let r = p.run(&["targets"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(r.out["engine"], "cdp");
    let targets = r.out["targets"].as_array().unwrap();
    assert!(
        targets
            .iter()
            .any(|t| t["title"] == "pokit fixture" && t["current"] == true),
        "{}",
        r.out
    );

    let r = p.run(&["close"]);
    assert_eq!(r.code, 0, "{}", r.out);
    p.launched = None;
    assert!(
        eventually(5000, || !process_alive(app)),
        "the launched instance outlived close"
    );
}

/// The ref written beside `line_start` in a snapshot, e.g. `textbox "Name"` → `e1`.
fn ref_of(snapshot: &str, line_start: &str) -> String {
    let line = snapshot
        .lines()
        .find(|l| l.trim_start().starts_with(&format!("- {line_start}")))
        .unwrap_or_else(|| panic!("no `{line_start}` in snapshot:\n{snapshot}"));
    let start = line
        .find("[ref=")
        .unwrap_or_else(|| panic!("no ref on `{line}`"))
        + 5;
    line[start..].split(']').next().unwrap().to_string()
}

#[test]
fn a_form_is_filled_through_refs_submitted_and_its_result_read_back() {
    let p = Pokit::launch_fixture("form");
    let snap = p.run(&["snapshot"]);
    assert_eq!(snap.code, 0, "{}", snap.out);
    let text = snap.out["snapshot"].as_str().unwrap();
    let (name, submit) = (
        ref_of(text, "textbox \"Name\""),
        ref_of(text, "button \"Submit\""),
    );

    let r = p.run(&["type", "kim", "--into", &name]);
    assert_eq!(r.code, 0, "{}", r.out);
    let r = p.run(&["click", &submit]);
    assert_eq!(r.code, 0, "{}", r.out);
    let r = p.run(&["wait", "--text", "Hello, kim"]);
    assert_eq!(r.code, 0, "{}", r.out);

    let r = p.run(&["read", "#result"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(r.out["text"], "Hello, kim");
    let r = p.run(&["read", "#name"]);
    assert_eq!(r.out["value"], "kim");
}

#[test]
fn a_ref_from_an_earlier_snapshot_is_refused_by_name() {
    let p = Pokit::launch_fixture("stale");
    let first = p.run(&["snapshot"]);
    let old = ref_of(first.out["snapshot"].as_str().unwrap(), "textbox \"Name\"");
    p.run(&["click", "#show-later"]);
    assert_eq!(p.run(&["wait", "--selector", "#late"]).code, 0);
    assert_eq!(p.run(&["snapshot"]).code, 0);

    let r = p.run(&["click", &old]);
    assert_eq!(r.code, 5, "{}", r.out);
    assert_eq!(r.out["error"]["kind"], "stale_ref");
    assert!(
        r.out["error"]["message"].as_str().unwrap().contains(&old),
        "{}",
        r.out
    );
}

#[test]
fn wait_holds_until_an_element_appears_and_times_out_on_one_that_never_comes() {
    let p = Pokit::launch_fixture("wait");
    let home = p.home.clone();
    let waiter = std::thread::spawn(move || {
        let w = Pokit {
            home,
            launched: None,
        };
        w.run(&["wait", "--selector", "#on-demand", "--for", "30000"])
    });
    std::thread::sleep(std::time::Duration::from_millis(1000));
    assert!(
        !waiter.is_finished(),
        "wait returned before #on-demand existed"
    );
    assert_eq!(p.run(&["click", "#show-now"]).code, 0);
    let r = waiter.join().unwrap();
    assert_eq!(r.code, 0, "{}", r.out);

    let start = std::time::Instant::now();
    let r = p.run(&["wait", "--selector", "#never", "--for", "1000"]);
    assert_eq!(r.code, 4, "{}", r.out);
    assert_eq!(r.out["error"]["kind"], "timeout");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(20),
        "wait ignored its own timeout and ran into the command's"
    );
}

#[test]
fn input_is_refused_when_focus_is_outside_the_required_target() {
    let p = Pokit::launch_fixture("guard");
    let r = p.run(&[
        "type",
        "x",
        "--into",
        "#other-input",
        "--require-focus",
        "#pane-form",
    ]);
    assert_eq!(r.code, 6, "{}", r.out);
    assert_eq!(r.out["error"]["kind"], "guard_refused");
    assert_eq!(p.run(&["read", "#other-input"]).out["value"], "");

    let r = p.run(&[
        "type",
        "y",
        "--into",
        "#name",
        "--require-focus",
        "#pane-form",
    ]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(p.run(&["read", "#name"]).out["value"], "y");
}

#[test]
fn a_secret_is_masked_in_output_logs_and_the_run_record() {
    let secret = "hunter2-secret";
    let p = Pokit::launch_fixture("secret");
    let r = p.run_with_stdin(&["type", "--secret", "--into", "#password"], Some(secret));
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(!r.out.to_string().contains(secret));
    p.run(&["type", "kim", "--into", "#name"]);
    p.run(&["click", "#submit"]);
    assert_eq!(p.run(&["wait", "--text", "Hello, kim"]).code, 0);

    let read = p.run(&["read", "#password"]);
    assert_eq!(read.out["value"], "***", "{}", read.out);
    let logs = p.run(&["logs"]).out.to_string();
    assert!(logs.contains("password=***"), "{logs}");
    assert!(!logs.contains(secret), "{logs}");
    let written = p.all_written_text();
    assert!(
        written.contains("\"type\""),
        "the record holds the type command"
    );
    assert!(
        !written.contains(secret),
        "the run record leaked the secret"
    );
}

#[test]
fn page_errors_and_backend_output_reach_logs() {
    let p = Pokit::launch_fixture("logs");
    p.run(&["click", "#throw"]);
    p.run(&["click", "#backend-log"]);
    let found = eventually(5000, || {
        let logs = p.run(&["logs"]).out;
        let entries = logs["entries"].as_array().cloned().unwrap_or_default();
        let has = |source: &str, text: &str| {
            entries
                .iter()
                .any(|e| e["source"] == source && e["text"].as_str().unwrap_or("").contains(text))
        };
        has("exception", "fixture boom") && has("backend-stdout", "fixture-backend: hello")
    });
    assert!(found, "{}", p.run(&["logs"]).out);

    let all = p.run(&["logs"]).out;
    let next = all["next"].as_u64().unwrap();
    assert_eq!(
        p.run(&["logs", "--since", &next.to_string()]).out["entries"],
        serde_json::json!([])
    );
}

#[test]
fn a_second_window_is_listed_and_can_be_switched_to() {
    let p = Pokit::launch_fixture("windows");
    p.run(&["click", "#open-second"]);
    // Waits for the second window by its URL.
    let listed = eventually(5000, || {
        let targets = p.run(&["targets"]).out["targets"].clone();
        let targets = targets.as_array().cloned().unwrap_or_default();
        targets.len() == 2
            && targets
                .iter()
                .any(|t| t["url"].as_str().unwrap_or("").contains("second.html"))
    });
    assert!(listed, "{}", p.run(&["targets"]).out);

    let r = p.run(&["targets", "--select", "second"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(
        p.run(&["eval", "document.title"]).out["value"],
        "pokit fixture - second"
    );
    let r = p.run(&["targets", "--select", "nothing-like-this"]);
    assert_eq!(r.code, 5, "{}", r.out);
}

#[test]
fn closing_an_attached_session_leaves_the_app_running() {
    let owner = Pokit::launch_fixture("attach-owner");
    let port = owner.launched.as_ref().unwrap()["cdp_port"]
        .as_u64()
        .unwrap()
        .to_string();
    let guest = Pokit::new("attach-guest");
    let r = guest.run(&["attach", "--port", &port]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(
        guest.run(&["eval", "document.title"]).out["value"],
        "pokit fixture"
    );
    let r = guest.run(&["close"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(r.out["closed"], false);
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(
        process_alive(owner.app_pid()),
        "closing an attached session closed the app"
    );
}

#[test]
fn an_idle_session_ends_and_closes_the_instance_it_launched() {
    let mut p = Pokit::new("idle");
    let exe = fixture_exe().to_str().unwrap().to_string();
    let r = p.run(&["launch", &exe, "--idle-timeout", "2"]);
    assert_eq!(r.code, 0, "{}", r.out);
    let app = r.out["app_pid"].as_u64().unwrap() as u32;
    assert!(
        eventually(10_000, || !process_alive(app)),
        "the idle session left its instance running"
    );
    assert_eq!(
        p.run(&["targets"]).code,
        5,
        "commands after the idle end find no session"
    );
    p.launched = None;
}

#[test]
fn chords_and_mouse_variants_reach_the_page_as_the_page_reports_them() {
    let p = Pokit::launch_fixture("input");
    p.run(&["click", "#target"]);
    assert_eq!(p.run(&["key", "Ctrl+KeyK"]).code, 0);
    assert_eq!(p.run(&["read", "#last-key"]).out["text"], "Ctrl+KeyK");
    assert_eq!(
        p.run(&["key", "Ctrl+="]).code,
        1,
        "a character instead of a key name is refused"
    );

    for (flag, event) in [
        ("--double", "dblclick"),
        ("--right", "contextmenu"),
        ("--hover", "mouseover"),
    ] {
        p.run(&["click", "#submit", "--hover"]);
        let r = p.run(&["click", "#target", flag]);
        assert_eq!(r.code, 0, "{}", r.out);
        assert_eq!(p.run(&["read", "#last-mouse"]).out["text"], event, "{flag}");
    }
}

#[test]
fn capture_writes_a_png_of_the_page_or_one_element() {
    let p = Pokit::launch_fixture("capture");
    let page = p.run(&["capture"]);
    assert_eq!(page.code, 0, "{}", page.out);
    let element = p.run(&["capture", "#submit"]);
    assert_eq!(element.code, 0, "{}", element.out);
    for r in [&page, &element] {
        let bytes = std::fs::read(r.out["path"].as_str().unwrap()).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    }
    assert!(
        element.out["bytes"].as_u64() < page.out["bytes"].as_u64(),
        "an element is smaller than the page"
    );
}

#[test]
fn a_failed_check_is_recorded_with_a_screenshot() {
    let p = Pokit::launch_fixture("record");
    let r = p.run(&["wait", "--selector", "#never", "--for", "300"]);
    assert_eq!(r.code, 4);
    let shot = r.out["error"]["screenshot"]
        .as_str()
        .expect("a failure screenshot path");
    assert!(std::path::Path::new(shot).exists());
    let record = p.all_written_text();
    assert!(
        record.contains("\"exit\":4") && record.contains("#never"),
        "{record}"
    );
}

#[test]
fn doctor_and_capabilities_describe_the_platform() {
    let p = Pokit::new("doctor");
    let r = p.run(&["doctor"]);
    assert_eq!(
        r.code, 3,
        "doctor without a session fails its check: {}",
        r.out
    );
    let r = p.run(&["capabilities"]);
    assert_eq!(r.code, 0);
    assert_eq!(r.out["commands"]["launch"]["supported"], true);

    let p = Pokit::launch_fixture("doctor-live");
    let r = p.run(&["doctor"]);
    assert_eq!(r.code, 0, "{}", r.out);
    let checks = r.out["checks"].as_array().unwrap();
    assert!(
        checks
            .iter()
            .any(|c| c["check"] == "webview2" && c["ok"] == true),
        "{}",
        r.out
    );
}

#[test]
fn a_usage_error_is_still_one_json_object_with_the_usage_code() {
    let p = Pokit::new("usage");
    let r = p.run(&["click", "--no-such-flag"]);
    assert_eq!(r.code, 2, "{}", r.out);
    assert_eq!(r.out["ok"], false);
    assert_eq!(r.out["error"]["kind"], "usage");
}

#[test]
fn refused_input_leaves_focus_where_it_was() {
    let p = Pokit::launch_fixture("guard-focus");
    p.run(&["click", "#other-input"]);
    let r = p.run(&[
        "type",
        "x",
        "--into",
        "#name",
        "--require-focus",
        "#pane-other",
    ]);
    assert_eq!(r.code, 6, "{}", r.out);
    assert_eq!(
        p.run(&["eval", "document.activeElement.id"]).out["value"],
        "other-input"
    );
    assert_eq!(p.run(&["read", "#name"]).out["value"], "");
}

#[test]
fn wait_on_a_broken_expression_fails_at_once_instead_of_waiting_out_its_timeout() {
    let p = Pokit::launch_fixture("wait-broken");
    let r = p.run(&[
        "wait",
        "--expr",
        "this is not ( javascript",
        "--for",
        "8000",
    ]);
    // Waiting out the timeout would end as `timeout` (exit 4), not as the page's error.
    assert_eq!(r.code, 1, "{}", r.out);
    assert_eq!(r.out["error"]["kind"], "js_error");
}

#[test]
fn the_run_record_starts_with_the_launch_and_holds_no_copy_of_the_logs() {
    let p = Pokit::launch_fixture("record-launch");
    p.run(&["logs"]);
    let record = p.all_written_text();
    let first = record.lines().next().expect("a record line");
    assert!(first.contains("\"command\":\"launch\""), "{first}");
    assert!(
        !record.contains("fixture-backend: started"),
        "logs output was copied into the record:\n{record}"
    );
}

#[test]
fn a_failed_secret_type_leaves_no_screenshot() {
    let p = Pokit::launch_fixture("secret-fail");
    let r = p.run_with_stdin(
        &["type", "--secret", "--into", "#nope"],
        Some("hunter2-secret"),
    );
    assert_eq!(r.code, 5, "{}", r.out);
    assert!(r.out["error"].get("screenshot").is_none(), "{}", r.out);
}

#[test]
fn a_stale_session_file_never_gets_a_process_pokit_did_not_start_killed() {
    let profile = std::env::temp_dir().join(format!("pokit-test-bystander-{}", std::process::id()));
    let mut bystander = std::process::Command::new(fixture_exe())
        .env("WEBVIEW2_USER_DATA_FOLDER", &profile)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let p = Pokit::new("stale-bystander");
    p.write_session_file(&serde_json::json!({
        "pid": 4_000_000_000u32, "status": "ready", "error": null, "port": 1, "token": "t",
        "mode": "launch", "app_pid": bystander.id(), "app_exe": fixture_exe(), "cdp_port": 1, "app_started": 1,
    }));
    let exe = fixture_exe().to_str().unwrap().to_string();
    let r = p.run(&["launch", &exe]);
    let alive = process_alive(bystander.id());
    let _ = bystander.kill();
    let _ = bystander.wait();
    p.run(&["close"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(
        alive,
        "a stale session file got a process pokit did not start killed"
    );
}

#[test]
fn a_session_that_dies_takes_the_instance_it_launched_with_it() {
    let mut p = Pokit::launch_fixture("orphan");
    let app = p.app_pid();
    let session = p.launched.as_ref().unwrap()["session_pid"]
        .as_u64()
        .unwrap();
    std::process::Command::new("taskkill")
        .args(["/PID", &session.to_string(), "/F"])
        .output()
        .unwrap();
    assert!(eventually(3000, || !process_alive(session as u32)));
    assert!(
        eventually(5000, || !process_alive(app)),
        "the instance outlived the session that launched it"
    );

    let exe = fixture_exe().to_str().unwrap().to_string();
    let r = p.run(&["launch", &exe]);
    assert_eq!(
        r.code, 0,
        "the dead session's file blocks no new launch: {}",
        r.out
    );
    p.launched = Some(r.out);
}

#[test]
fn a_process_squatting_on_the_session_port_never_receives_the_token_or_a_secret() {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let received = std::thread::spawn(move || {
        let mut got = Vec::new();
        if let Ok((mut s, _)) = listener.accept() {
            use std::io::Write;
            s.set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .ok();
            let mut buf = [0u8; 4096];
            // It answers like a session would, with a proof it cannot know.
            if let Ok(n) = s.read(&mut buf) {
                got.extend_from_slice(&buf[..n]);
                let _ = s.write_all(b"{\"proof\":\"a-guess\"}\n");
            }
            while let Ok(n) = s.read(&mut buf) {
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
            }
        }
        String::from_utf8_lossy(&got).to_string()
    });
    let p = Pokit::new("squat");
    p.write_session_file(&serde_json::json!({
        "pid": 1, "status": "ready", "error": null, "port": port, "token": "session-token-123",
        "mode": "attach", "app_pid": null, "app_exe": null, "cdp_port": 1, "proof": "the-real-proof",
    }));
    let r = p.run_with_stdin(
        &["type", "--secret", "--into", "#password"],
        Some("hunter2-secret"),
    );
    let got = received.join().unwrap();
    assert_eq!(r.code, 5, "{}", r.out);
    assert!(
        !got.contains("hunter2-secret") && !got.contains("session-token-123"),
        "the squatter received: {got}"
    );
}

#[test]
fn a_relative_capture_path_is_relative_to_where_the_command_ran() {
    let p = Pokit::launch_fixture("capture-rel");
    let dir = p.home.join("caller");
    std::fs::create_dir_all(&dir).unwrap();
    let r = p.run_in(&dir, &["capture", "--out", "shot.png"], None);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(dir.join("shot.png").exists(), "{}", r.out);
}

#[test]
fn launch_removes_profiles_left_by_ended_sessions_and_keeps_its_own() {
    let other = Pokit::launch_fixture("profile-other");
    let other_session = other.launched.as_ref().unwrap()["session_pid"]
        .as_u64()
        .unwrap();
    let mut p = Pokit::new("profile");
    let orphan = p.home.join("instances").join("19990101-000000-4000000000");
    let live = p
        .home
        .join("instances")
        .join(format!("19990101-000000-{other_session}"));
    for dir in [&orphan, &live] {
        std::fs::create_dir_all(dir.join("EBWebView")).unwrap();
        std::fs::write(dir.join("EBWebView").join("Local State"), "{}").unwrap();
    }

    let exe = fixture_exe().to_str().unwrap().to_string();
    let r = p.run(&["launch", &exe]);
    assert_eq!(r.code, 0, "{}", r.out);
    let own = std::path::PathBuf::from(r.out["data_dir"].as_str().unwrap());
    p.launched = Some(r.out);
    assert!(
        eventually(30_000, || !orphan.exists()),
        "a profile left by an ended session survived the next launch"
    );
    assert!(
        own.exists(),
        "launch removed the profile of its own instance"
    );
    assert!(
        live.exists(),
        "launch removed the profile of a session still running"
    );
}

#[test]
fn a_chord_can_be_sent_into_an_element() {
    let p = Pokit::launch_fixture("key-into");
    let r = p.run(&["key", "Shift+KeyA", "--into", "#name"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(p.run(&["read", "#name"]).out["value"], "A");
}

#[test]
fn a_chord_on_cdp_presses_its_modifiers_as_keys_like_a_keyboard() {
    let p = Pokit::launch_fixture("cdp-modifiers");
    assert_eq!(p.run(&["eval", "window.__events.length = 0"]).code, 0);
    let r = p.run(&["key", "Ctrl+KeyK", "--into", "#name"]);
    assert_eq!(r.code, 0, "{}", r.out);
    let keys = p
        .run(&[
            "eval",
            "window.__events.filter(e => e[1].startsWith('key')).map(e => e[1] + ':' + e[3])",
        ])
        .out["value"]
        .clone();
    assert_eq!(
        keys,
        serde_json::json!(["keydown:Control", "keydown:k", "keyup:k", "keyup:Control"]),
        "measured from a real keyboard on 2026-10-07"
    );
    assert_eq!(p.run(&["read", "#last-key"]).out["text"], "Ctrl+KeyK");
}

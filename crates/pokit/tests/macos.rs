//! The macOS backend: a session on the fixture app's test build, through pokit's plugin.

#![cfg(target_os = "macos")]

mod common;

use common::*;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Held by the tests that look at, or may change, which app is frontmost.
static FRONTMOST: Mutex<()> = Mutex::new(());

const UNSUPPORTED: i32 = 7;

#[test]
fn eval_runs_in_the_page_and_close_ends_the_app() {
    let mut p = Pokit::launch_fixture("mac-eval");
    let app = p.app_pid();
    assert!(process_alive(app));

    let r = p.run(&["eval", "1 + 1"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(r.out["value"], 2);
    assert_eq!(r.out["engine"], "plugin", "{}", r.out);

    let r = p.run(&["eval", "document.title"]);
    assert_eq!(r.out["value"], "pokit fixture", "{}", r.out);

    let r = p.run(&[
        "eval",
        "new Promise(r => setTimeout(() => r({ late: [1, 'x'] }), 50))",
    ]);
    assert_eq!(r.out["value"], json!({ "late": [1, "x"] }), "{}", r.out);

    let r = p.run(&["eval", "undefined"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(r.out["value"], Value::Null);

    let r = p.run(&["eval", "(() => { throw new Error('fixture boom') })()"]);
    assert_eq!(r.out["error"]["kind"], "js_error", "{}", r.out);
    assert!(
        r.out["error"]["message"]
            .as_str()
            .unwrap()
            .contains("fixture boom"),
        "{}",
        r.out
    );

    let r = p.run(&["eval", "var a = 20; a + 1"]);
    assert_eq!(r.out["value"], 21, "{}", r.out);

    let r = p.run(&[
        "eval",
        "(window.__pokitRuns = (window.__pokitRuns || 0) + 1, JSON.parse('{'))",
    ]);
    assert_eq!(r.out["error"]["kind"], "js_error", "{}", r.out);
    let r = p.run(&["eval", "window.__pokitRuns"]);
    assert_eq!(
        r.out["value"], 1,
        "an expression that threw ran more than once"
    );

    let r = p.run(&[
        "--timeout",
        "1",
        "eval",
        "new Promise(r => setTimeout(r, 5000))",
    ]);
    assert_eq!(r.out["error"]["kind"], "timeout", "{}", r.out);
    let r = p.run(&["--timeout", "1", "eval", "new Promise(() => {})"]);
    assert_eq!(r.out["error"]["kind"], "timeout", "{}", r.out);

    let r = p.run(&["close"]);
    assert_eq!(r.code, 0, "{}", r.out);
    p.launched = None;
    assert!(
        eventually(5000, || !process_alive(app)),
        "the launched app outlived close"
    );
}

#[test]
fn type_and_key_reach_the_page_behind_another_app_and_leave_the_front_and_the_cursor_alone() {
    let _front = FRONTMOST.lock().unwrap_or_else(|e| e.into_inner());
    let p = Pokit::launch_fixture("mac-type");
    let watching = Arc::new(AtomicBool::new(true));
    let fronts = {
        let watching = watching.clone();
        std::thread::spawn(move || {
            let mut seen = Vec::new();
            while watching.load(Ordering::SeqCst) {
                if let Some(pid) = frontmost_pid() {
                    if seen.last() != Some(&pid) {
                        seen.push(pid);
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            seen
        })
    };
    // The cursor is read just before and after each input command.
    let moved = std::cell::RefCell::new(Vec::new());
    let input = |args: &[&str]| {
        let (before, start) = (cursor_location(), std::time::Instant::now());
        let r = p.run(args);
        let after = cursor_location();
        if after != before {
            moved.borrow_mut().push((
                format!("{args:?}: {before:?} -> {after:?}"),
                start.elapsed(),
            ));
        }
        r
    };

    let r = input(&["type", "kim", "--into", "#name"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(r.out["engine"], "plugin", "{}", r.out);
    assert_eq!(r.out["key_events"], 3, "{}", r.out);
    assert_eq!(r.out["keydowns_sent"], 3, "{}", r.out);
    assert_eq!(r.out["keydowns_received"], 3, "{}", r.out);
    assert_eq!(r.out["delivery"], "confirmed", "{}", r.out);
    assert_eq!(p.run(&["read", "#name"]).out["value"], "kim");

    let r = input(&["type", " Kim é한", "--into", "#other-input"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(r.out["inserted_chars"], 2, "{}", r.out);
    assert_eq!(r.out["keydowns_sent"], 6, "{}", r.out);
    assert_eq!(p.run(&["read", "#other-input"]).out["value"], " Kim é한");

    let r = input(&["key", "Ctrl+Shift+KeyT", "--into", "#name"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(r.out["code"], "KeyT", "{}", r.out);
    assert_eq!(r.out["keydowns_received"], 3, "{}", r.out);
    assert_eq!(p.run(&["read", "#last-key"]).out["text"], "Ctrl+Shift+KeyT");

    let r = input(&["key", "Backspace"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(p.run(&["read", "#name"]).out["value"], "ki");

    watching.store(false, Ordering::SeqCst);
    let fronts = fronts.join().unwrap();
    assert!(
        !fronts.contains(&p.app_pid()),
        "input brought the app (pid {}) to the front; frontmost pids in order: {fronts:?}",
        p.app_pid()
    );
    let moved = moved.into_inner();
    if let Some((_, took)) = moved.iter().max_by_key(|(_, took)| *took) {
        // A cursor that also moves with no input sent is a hand on the mouse.
        let before = cursor_location();
        std::thread::sleep(*took);
        if cursor_location() != before {
            eprintln!("the mouse is in use; the cursor was not checked: {moved:?}");
            return;
        }
        panic!("the cursor moved during input: {moved:?}");
    }
}

#[test]
fn inserted_text_keeps_its_place_behind_keys_a_busy_page_has_not_handled_yet() {
    let p = Pokit::launch_fixture("mac-type-order");
    for _ in 0..3 {
        let r = p.run(&[
            "eval",
            "window.__stallMs = 30; document.querySelector('#stall').value = ''",
        ]);
        assert_eq!(r.code, 0, "{}", r.out);
        let r = p.run(&["type", "abcé", "--into", "#stall"]);
        assert_eq!(r.code, 0, "{}", r.out);
        assert_eq!(p.run(&["read", "#stall"]).out["value"], "abcé");
    }
}

#[test]
fn typed_text_arrives_as_given_under_a_korean_input_source() {
    let p = Pokit::launch_fixture("mac-input-source");
    let Some(_korean) = InputSource::select("com.apple.inputmethod.Korean.2SetKorean") else {
        eprintln!("2-Set Korean is not an enabled input source; nothing to check");
        return;
    };
    let r = p.run(&["type", "abc", "--into", "#name"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(p.run(&["read", "#name"]).out["value"], "abc");
}

#[test]
fn input_refuses_what_macos_cannot_take_and_masks_a_secret() {
    let secret = "hunter2-secret";
    let p = Pokit::launch_fixture("mac-type-secret");
    let r = p.run_with_stdin(&["type", "--secret", "--into", "#password"], Some(secret));
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(!r.out.to_string().contains(secret), "{}", r.out);
    let read = p.run(&["read", "#password"]);
    assert_eq!(read.out["value"], "***", "{}", read.out);
    assert!(
        p.all_written_text().contains("\"type\""),
        "the record holds the type command"
    );
    assert!(
        !p.all_written_text().contains(secret),
        "the run record leaked the secret"
    );

    for args in [
        &["type", "x", "--route", "cdp"][..],
        &["key", "KeyA", "--route", "os"][..],
        &["type", "x", "--into", "e3"][..],
    ] {
        let r = p.run(args);
        assert_eq!(r.code, UNSUPPORTED, "{args:?}: {}", r.out);
    }

    let r = p.run(&[
        "type",
        "x",
        "--into",
        "#name",
        "--require-focus",
        "#pane-other",
    ]);
    assert_eq!(r.code, 6, "{}", r.out);
    assert_eq!(r.out["error"]["kind"], "guard_refused", "{}", r.out);
    let r = p.run(&["key", "KeyA", "--into", "#nothing-here"]);
    assert_eq!(r.out["error"]["kind"], "not_found", "{}", r.out);

    let caps = p.run(&["capabilities"]).out;
    for command in ["key", "type"] {
        let c = &caps["commands"][command];
        assert_eq!(c["supported"], true, "{command}: {c}");
        assert_eq!(c["routes"]["plugin"]["default"], true, "{command}: {c}");
        assert_eq!(
            c["routes"]["plugin"]["takes_focus"], false,
            "{command}: {c}"
        );
    }
}

#[test]
fn read_by_selector_returns_the_fields_windows_does() {
    let p = Pokit::launch_fixture("mac-read");
    let r = p.run(&[
        "eval",
        "document.querySelector('#name').value = 'kim'; \
         document.querySelector('#result').textContent = ' Hello, kim '",
    ]);
    assert_eq!(r.code, 0, "{}", r.out);

    let r = p.run(&["read", "#name"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(r.out["engine"], "plugin", "{}", r.out);
    assert_eq!(r.out["tag"], "input", "{}", r.out);
    assert_eq!(r.out["value"], "kim", "{}", r.out);
    assert_eq!(
        r.out["state"],
        json!({ "focused": false, "disabled": false, "checked": false, "visible": true }),
        "{}",
        r.out
    );

    let r = p.run(&["read", "#result"]);
    assert_eq!(r.out["text"], "Hello, kim", "{}", r.out);
    assert_eq!(r.out["value"], Value::Null, "{}", r.out);
    assert_eq!(r.out["state"]["checked"], Value::Null, "{}", r.out);

    let r = p.run(&["read", "#nothing-here"]);
    assert_eq!(r.code, 5, "{}", r.out);
    assert_eq!(r.out["error"]["kind"], "not_found", "{}", r.out);

    let r = p.run(&["read", "[[broken"]);
    assert_eq!(r.code, 1, "{}", r.out);
    assert_eq!(r.out["error"]["kind"], "error", "{}", r.out);
    assert!(
        r.out["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("invalid selector `[[broken`"),
        "{}",
        r.out
    );

    let r = p.run(&["read", "e12"]);
    assert_eq!(r.code, UNSUPPORTED, "{}", r.out);
    assert!(
        r.out["error"]["message"].as_str().unwrap().contains("#14"),
        "{}",
        r.out
    );
}

#[test]
fn wait_holds_until_an_element_appears_and_times_out_on_one_that_never_comes() {
    let p = Pokit::launch_fixture("mac-wait");
    let home = p.home.clone();
    let waiter = std::thread::spawn(move || {
        let w = Pokit::on_home(home);
        w.run(&["wait", "--selector", "#on-demand", "--for", "30000"])
    });
    std::thread::sleep(Duration::from_millis(1000));
    assert!(
        !waiter.is_finished(),
        "wait returned before #on-demand existed"
    );
    let r = p.run(&["eval", "document.querySelector('#show-now').click()"]);
    assert_eq!(r.code, 0, "{}", r.out);
    let r = waiter.join().unwrap();
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(r.out["engine"], "plugin", "{}", r.out);

    assert_eq!(p.run(&["wait", "--text", "Here"]).code, 0);
    assert_eq!(p.run(&["wait", "--expr", "1 + 1 === 2"]).code, 0);

    let start = std::time::Instant::now();
    let r = p.run(&["wait", "--selector", "#never", "--for", "1000"]);
    assert_eq!(r.code, 4, "{}", r.out);
    assert_eq!(r.out["error"]["kind"], "timeout");
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "wait ignored its own timeout and ran into the command's"
    );

    let r = p.run(&[
        "wait",
        "--expr",
        "this is not ( javascript",
        "--for",
        "8000",
    ]);
    assert_eq!(r.code, 1, "{}", r.out);
    assert_eq!(r.out["error"]["kind"], "js_error", "{}", r.out);
}

#[test]
fn launch_reports_the_session_and_the_run_record_keeps_each_command() {
    let mut p = Pokit::launch_fixture("mac-record");
    let launched = p.launched.clone().unwrap();
    assert!(launched["session_pid"].as_u64().is_some(), "{launched}");
    assert!(
        launched["clock"]["uncertainty_ms"].as_f64().is_some(),
        "{launched}"
    );
    let record_dir = std::path::PathBuf::from(launched["record_dir"].as_str().unwrap());
    assert!(record_dir.starts_with(p.home.join("runs")), "{launched}");

    assert_eq!(p.run(&["eval", "1 + 1"]).out["value"], 2);
    assert_eq!(p.run(&["close"]).code, 0);
    p.launched = None;

    let commands = || -> Vec<Value> {
        std::fs::read_to_string(record_dir.join("record.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .map(|l| l["command"].clone())
            .collect()
    };
    assert!(
        eventually(5000, || commands().contains(&json!("shutdown"))),
        "{:?}",
        commands()
    );
    let commands = commands();
    assert_eq!(
        commands[..3],
        [json!("launch"), json!("eval"), json!("close")],
        "{commands:?}"
    );
}

#[test]
fn launch_keeps_its_own_token_over_a_callers_env() {
    let mut p = Pokit::new("mac-env");
    let exe = fixture_test_build_exe().to_str().unwrap().to_string();
    let r = p.run(&["launch", "--env", "POKIT_TOKEN=from-the-caller", &exe]);
    assert_eq!(r.code, 0, "{}", r.out);
    p.launched = Some(r.out);
    assert_eq!(p.run(&["eval", "1 + 1"]).out["value"], 2);
}

#[test]
fn launching_a_build_without_the_plugin_fails_and_leaves_nothing() {
    let _front = FRONTMOST.lock().unwrap_or_else(|e| e.into_inner());
    let p = Pokit::new("mac-noplugin");
    let exe = fixture_exe().to_str().unwrap().to_string();
    let r = p.run(&["launch", "--ready-timeout", "3", &exe]);
    assert_ne!(r.code, 0, "{}", r.out);
    assert!(
        r.out["error"]["message"]
            .as_str()
            .unwrap()
            .contains("test build"),
        "{}",
        r.out
    );
    let left: Vec<_> = std::fs::read_dir(p.home.join("instances"))
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "a failed launch left {left:?}");
    let running = run_text("pgrep", &["-f", &exe]);
    assert!(
        running.trim().is_empty(),
        "a failed launch left the app running: {running}"
    );
}

#[test]
fn launch_leaves_the_frontmost_app_frontmost() {
    let _front = FRONTMOST.lock().unwrap_or_else(|e| e.into_inner());
    let Some(before) = frontmost_pid() else {
        eprintln!("no app is frontmost (screen locked?); nothing to check");
        return;
    };
    let watching = Arc::new(AtomicBool::new(true));
    let sampler = {
        let watching = watching.clone();
        std::thread::spawn(move || {
            let (mut seen, mut samples) = (vec![before], 0);
            while watching.load(Ordering::SeqCst) {
                samples += 1;
                if let Some(pid) = frontmost_pid() {
                    if seen.last() != Some(&pid) {
                        seen.push(pid);
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            (seen, samples)
        })
    };
    let p = Pokit::launch_fixture("mac-frontmost");
    std::thread::sleep(Duration::from_millis(1000));
    watching.store(false, Ordering::SeqCst);
    let (seen, samples) = sampler.join().unwrap();
    assert!(samples >= 10, "the front was sampled only {samples} times");
    assert!(
        !seen.contains(&p.app_pid()),
        "launch brought the app (pid {}) to the front; frontmost pids in order: {seen:?}",
        p.app_pid()
    );
}

#[test]
fn a_request_without_the_token_is_refused() {
    let p = Pokit::launch_fixture("mac-token");
    let port = plugin_port(&p);

    for request in [
        json!({ "op": "eval", "js": "window.__pokitUnauthorised = 1" }),
        json!({ "token": "not-the-token", "op": "eval", "js": "window.__pokitUnauthorised = 1" }),
    ] {
        let reply = plugin_request(port, &request);
        assert_eq!(reply["ok"], false, "{reply}");
        assert_eq!(reply["error"]["kind"], "unauthorised", "{reply}");
    }
    let r = p.run(&["eval", "window.__pokitUnauthorised === undefined"]);
    assert_eq!(
        r.out["value"], true,
        "a refused request ran in the page: {}",
        r.out
    );
}

#[test]
fn the_plugin_listens_on_loopback_only() {
    let p = Pokit::launch_fixture("mac-loopback");
    let port = plugin_port(&p);
    let listening = listening_addresses(p.app_pid());
    assert!(
        listening.iter().all(|a| a.starts_with("127.0.0.1:")),
        "the app listens beyond loopback: {listening:?}"
    );
    assert!(
        listening.contains(&format!("127.0.0.1:{port}")),
        "the plugin's port {port} is not among {listening:?}"
    );
}

#[test]
fn a_build_without_the_feature_has_no_plugin_and_opens_no_port() {
    let _front = FRONTMOST.lock().unwrap_or_else(|e| e.into_inner());
    let marker = b"POKIT_PORT_FILE";
    assert!(
        contains(&std::fs::read(fixture_test_build_exe()).unwrap(), marker),
        "the test build should carry the plugin"
    );
    assert!(
        !contains(&std::fs::read(fixture_exe()).unwrap(), marker),
        "a build without the feature carries the plugin"
    );

    let dir = std::env::temp_dir().join(format!("pokit-test-nofeature-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let port_file = dir.join("port");
    let mut app = Command::new(fixture_exe())
        .env("POKIT_TOKEN", "a-token")
        .env("POKIT_PORT_FILE", &port_file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid = app.id();
    let mut started = String::new();
    BufReader::new(app.stdout.take().unwrap())
        .read_line(&mut started)
        .unwrap();
    assert!(started.contains("started"), "{started:?}");
    std::thread::sleep(Duration::from_millis(3000));
    let port_written = port_file.exists();
    let listening = listening_addresses(pid);
    let _ = app.kill();
    let _ = app.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!port_written, "a build without the feature wrote a port");
    assert!(
        listening.is_empty(),
        "a build without the feature listens on {listening:?}"
    );
}

#[test]
fn commands_not_built_on_macos_exit_unsupported() {
    let p = Pokit::new("mac-unsupported");
    let caps = p.run(&["capabilities"]).out;
    for supported in ["launch", "close", "eval", "read", "wait", "capabilities"] {
        assert_eq!(
            caps["commands"][supported]["supported"], true,
            "{supported}: {caps}"
        );
    }
    for unsupported in ["attach", "snapshot", "click", "trace", "profile", "doctor"] {
        assert_eq!(
            caps["commands"][unsupported]["supported"], false,
            "{unsupported}: {caps}"
        );
    }
    assert!(
        caps["commands"]["trace"]["reason"]
            .as_str()
            .unwrap()
            .contains("Timeline"),
        "{caps}"
    );

    for args in [
        &["attach", "--port", "9222"][..],
        &["doctor"][..],
        &["snapshot"][..],
    ] {
        let r = p.run(args);
        assert_eq!(r.code, UNSUPPORTED, "{args:?}: {}", r.out);
        assert_eq!(r.out["error"]["kind"], "unsupported", "{args:?}: {}", r.out);
    }

    let r = p.run(&["trace", "start"]);
    assert!(
        r.out["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Timeline"),
        "{}",
        r.out
    );

    let exe = fixture_test_build_exe().to_str().unwrap().to_string();
    for flag in [&["--port", "9222"][..], &["--data-dir", "/tmp/x"][..]] {
        let args = [&["launch"][..], flag, &[exe.as_str()][..]].concat();
        let r = p.run(&args);
        assert_eq!(r.code, UNSUPPORTED, "{args:?}: {}", r.out);
        assert!(
            r.out["error"]["message"]
                .as_str()
                .unwrap()
                .contains(flag[0]),
            "{args:?}: {}",
            r.out
        );
    }

    let p = Pokit::launch_fixture("mac-unsupported-session");
    for args in [
        &["snapshot"][..],
        &["click", "#target"][..],
        &["trace", "start"][..],
        &["profile", "start"][..],
    ] {
        let r = p.run(args);
        assert_eq!(r.code, UNSUPPORTED, "{args:?}: {}", r.out);
        assert_eq!(r.out["error"]["kind"], "unsupported", "{args:?}: {}", r.out);
    }
    let r = p.run(&["profile", "start"]);
    assert!(
        r.out["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Timeline"),
        "{}",
        r.out
    );
}

/// The frontmost app's process id; none when nothing is, or the login window is (a locked screen).
fn frontmost_pid() -> Option<u32> {
    let asn = run_text("lsappinfo", &["front"]);
    let info = run_text("lsappinfo", &["info", "-only", "pid", asn.trim()]);
    let pid: u32 = info.split('=').nth(1)?.trim().parse().ok()?;
    let name = run_text("ps", &["-o", "comm=", "-p", &pid.to_string()]);
    (!name.trim().ends_with("/loginwindow")).then_some(pid)
}

/// The plugin's port, as `launch` reported it.
fn plugin_port(p: &Pokit) -> u16 {
    let launched = p.launched.as_ref().unwrap();
    launched["plugin_port"]
        .as_u64()
        .unwrap_or_else(|| panic!("no plugin_port in {launched}")) as u16
}

/// One request to the plugin, and its one-line reply.
fn plugin_request(port: u16, request: &Value) -> Value {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(format!("{request}\n").as_bytes()).unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap_or_else(|_| panic!("not JSON: {line:?}"))
}

/// The TCP addresses process `pid` listens on, as `ip:port`.
fn listening_addresses(pid: u32) -> Vec<String> {
    run_text(
        "lsof",
        &[
            "-nP",
            "-a",
            "-p",
            &pid.to_string(),
            "-iTCP",
            "-sTCP:LISTEN",
            "-Fn",
        ],
    )
    .lines()
    .filter_map(|l| l.strip_prefix('n'))
    .map(str::to_string)
    .collect()
}

fn run_text(program: &str, args: &[&str]) -> String {
    let out = Command::new(program)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Where the cursor is, in global display points.
fn cursor_location() -> (f64, f64) {
    #[repr(C)]
    struct Point {
        x: f64,
        y: f64,
    }
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn CGEventCreate(source: *const std::ffi::c_void) -> *const std::ffi::c_void;
        fn CGEventGetLocation(event: *const std::ffi::c_void) -> Point;
        fn CFRelease(object: *const std::ffi::c_void);
    }
    // SAFETY: an event made with no source only reads the current cursor; it is released here.
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        let at = CGEventGetLocation(event);
        CFRelease(event);
        (at.x, at.y)
    }
}

/// An input source selected for a test; the one selected before is selected again on drop.
struct InputSource {
    before: *const std::ffi::c_void,
}

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn TISCopyCurrentKeyboardInputSource() -> *const std::ffi::c_void;
    fn TISCreateInputSourceList(
        properties: *const std::ffi::c_void,
        include_all: bool,
    ) -> *const std::ffi::c_void;
    fn TISGetInputSourceProperty(
        source: *const std::ffi::c_void,
        key: *const std::ffi::c_void,
    ) -> *const std::ffi::c_void;
    fn TISSelectInputSource(source: *const std::ffi::c_void) -> i32;
    static kTISPropertyInputSourceID: *const std::ffi::c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFArrayGetCount(array: *const std::ffi::c_void) -> isize;
    fn CFArrayGetValueAtIndex(array: *const std::ffi::c_void, i: isize) -> *const std::ffi::c_void;
    fn CFStringGetCString(
        string: *const std::ffi::c_void,
        buffer: *mut std::ffi::c_char,
        size: isize,
        encoding: u32,
    ) -> bool;
    fn CFRelease(object: *const std::ffi::c_void);
}

impl InputSource {
    /// Selects the enabled input source `id`; `None` when it is not enabled or did not take.
    fn select(id: &str) -> Option<InputSource> {
        // SAFETY: Carbon's input source calls on objects it returns; each one copied or created
        // here is released, the current source on drop.
        unsafe {
            let list = TISCreateInputSourceList(std::ptr::null(), false);
            let found = (0..CFArrayGetCount(list))
                .map(|i| CFArrayGetValueAtIndex(list, i))
                .find(|s| source_id(*s).as_deref() == Some(id));
            let selected = found.map(|source| {
                let before = TISCopyCurrentKeyboardInputSource();
                (TISSelectInputSource(source) == 0).then_some(InputSource { before })
            });
            CFRelease(list);
            let selected = selected.flatten()?;
            let now = TISCopyCurrentKeyboardInputSource();
            let took = source_id(now).as_deref() == Some(id);
            CFRelease(now);
            took.then_some(selected)
        }
    }
}

impl Drop for InputSource {
    fn drop(&mut self) {
        // SAFETY: `before` is the source copied in `select`, released once here.
        unsafe {
            TISSelectInputSource(self.before);
            CFRelease(self.before);
        }
    }
}

/// An input source's identifier.
unsafe fn source_id(source: *const std::ffi::c_void) -> Option<String> {
    let id = TISGetInputSourceProperty(source, kTISPropertyInputSourceID);
    if id.is_null() {
        return None;
    }
    let mut buffer = [0 as std::ffi::c_char; 256];
    const UTF8: u32 = 0x0800_0100;
    CFStringGetCString(id, buffer.as_mut_ptr(), buffer.len() as isize, UTF8).then(|| {
        std::ffi::CStr::from_ptr(buffer.as_ptr())
            .to_string_lossy()
            .into_owned()
    })
}

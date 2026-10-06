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
    for supported in ["launch", "close", "eval", "capabilities"] {
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

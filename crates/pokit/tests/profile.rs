//! `trace` and `profile`, against the fixture app's stall field.

#![cfg(windows)]

mod common;

use common::*;
use serde_json::Value;

/// Holds `KeyA` 10 times into the stall field with the page stalling 60 ms per key.
fn stall_ten_keys(p: &Pokit) {
    assert_eq!(p.run(&["eval", "window.__stallMs = 60"]).code, 0);
    let r = p.run(&["hold", "KeyA", "--count", "10", "--into", "#stall"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert_eq!(
        p.run(&[
            "wait",
            "--expr",
            "document.querySelector('#key-count').dataset.count === '10'"
        ])
        .code,
        0
    );
}

fn ms(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"))
}

#[test]
fn a_trace_of_a_stalling_page_attributes_the_time_to_script() {
    let p = shared_fixture();
    let r = p.run(&["trace", "start"]);
    assert_eq!(r.code, 0, "{}", r.out);
    stall_ten_keys(&p);
    let t = p.run(&["trace", "stop"]);
    assert_eq!(t.code, 0, "{}", t.out);
    let t = t.out;

    let groups = t["groups_ms"].as_object().unwrap();
    let script = ms(&groups["script"]);
    assert!(
        script >= 10.0 * 60.0,
        "script {script} ms is less than the 10 × 60 ms the page stalled: {t}"
    );
    for (name, v) in groups {
        if name != "script" {
            assert!(ms(v) < script, "{name} outweighs script: {t}");
        }
    }
    assert_eq!(t["thread"]["name"], "CrRendererMain", "{t}");
    assert_eq!(t["data_loss"], false, "{t}");

    let saved: Value =
        serde_json::from_slice(&std::fs::read(t["path"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(
        saved["traceEvents"].as_array().map(Vec::len),
        t["events"].as_u64().map(|n| n as usize),
        "the saved trace is not the one summarised: {t}"
    );
}

#[test]
fn a_profile_of_a_stalling_page_names_the_stalling_function() {
    let p = shared_fixture();
    let r = p.run(&["profile", "start"]);
    assert_eq!(r.code, 0, "{}", r.out);
    stall_ten_keys(&p);
    let r = p.run(&["profile", "stop", "--top", "3"]);
    assert_eq!(r.code, 0, "{}", r.out);
    let prof = r.out;

    let functions = prof["functions"].as_array().unwrap();
    assert!(functions.len() <= 3, "{prof}");
    let stall = functions
        .iter()
        .find(|f| f["function"].as_str().unwrap().starts_with("stallOnKey @"))
        .unwrap_or_else(|| panic!("stallOnKey is not among the top 3 self-time entries: {prof}"));
    let location = stall["function"].as_str().unwrap();
    let mut tail = location.rsplitn(3, ':');
    let (col, line) = (tail.next().unwrap(), tail.next().unwrap());
    assert!(
        col.parse::<u32>().is_ok() && line.parse::<u32>().is_ok(),
        "the location is not `fn @file:line:col`: {location}"
    );
    assert!(
        std::path::Path::new(prof["path"].as_str().unwrap()).exists(),
        "{prof}"
    );
}

#[test]
fn stop_without_start_and_a_second_start_are_refused() {
    let p = shared_fixture();
    for what in ["trace", "profile"] {
        let r = p.run(&[what, "stop"]);
        assert_eq!(r.code, 1, "{what} stop without start: {}", r.out);
        assert_eq!(p.run(&[what, "start"]).code, 0);
        let r = p.run(&[what, "start"]);
        assert_eq!(r.code, 1, "a second {what} start: {}", r.out);
        let message = r.out["error"]["message"].as_str().unwrap_or("");
        assert!(
            message.contains(&format!("{what} stop")),
            "a second {what} start does not say to stop the first: {}",
            r.out
        );
        assert_eq!(p.run(&[what, "stop"]).code, 0);
    }
}

#[test]
fn two_starts_at_once_give_one_recording_and_pokits_own_refusal() {
    let p = shared_fixture();
    for what in ["trace", "profile"] {
        for round in 0..10 {
            let other = Pokit::on_home(p.home.clone());
            let racing = std::thread::spawn(move || other.run(&[what, "start"]));
            let mine = p.run(&[what, "start"]);
            let theirs = racing.join().unwrap();
            let codes = [mine.code, theirs.code];
            assert!(
                codes.contains(&0) && codes.contains(&1),
                "{what} round {round}: one start runs and one is refused: {} | {}",
                mine.out,
                theirs.out
            );
            let refused = if mine.code == 1 { &mine } else { &theirs };
            let message = refused.out["error"]["message"].as_str().unwrap_or("");
            assert!(
                message.contains("already running"),
                "{what} round {round}: refused with someone else's message: {}",
                refused.out
            );
            assert_eq!(p.run(&[what, "stop"]).code, 0, "{what} round {round}");
        }
    }

    // A measurement replaces a running one, so two at once both succeed; what is left must be
    // one whole measurement.
    for round in 0..10 {
        let other = Pokit::on_home(p.home.clone());
        let racing = std::thread::spawn(move || other.run(&["measure", "start"]));
        let mine = p.run(&["measure", "start"]);
        let theirs = racing.join().unwrap();
        assert_eq!(
            (mine.code, theirs.code),
            (0, 0),
            "round {round}: {} | {}",
            mine.out,
            theirs.out
        );
        let r = p.run(&["measure", "stop", "--quiet", "100"]);
        assert_eq!(r.code, 0, "measure round {round}: {}", r.out);
    }
}

#[test]
fn a_reload_during_a_trace_or_profile_voids_the_run_and_keeps_the_file() {
    let p = shared_fixture();
    for what in ["trace", "profile"] {
        assert_eq!(p.run(&[what, "start"]).code, 0);
        let before = p.run(&["eval", "performance.timeOrigin"]).out["value"].clone();
        assert_eq!(p.run(&["eval", "location.reload(); true"]).code, 0);
        assert!(
            eventually(10_000, || {
                let now = p.run(&["eval", "performance.timeOrigin"]);
                now.code == 0 && now.out["value"] != before
            }),
            "the page did not reload"
        );
        let r = p.run(&[what, "stop"]);
        assert_eq!(r.code, 8, "{what}: {}", r.out);
        assert_eq!(r.out["error"]["kind"], "page_reloaded", "{what}: {}", r.out);
        let path = r.out["error"]["path"].as_str().unwrap();
        assert!(std::path::Path::new(path).exists(), "{what}: {}", r.out);
        assert_eq!(
            p.run(&[what, "start"]).code,
            0,
            "{what} could not start again after a void run"
        );
        assert_eq!(p.run(&[what, "stop"]).code, 0);
    }
}

#[test]
fn closing_an_attached_session_ends_its_trace() {
    let owner = Pokit::launch_fixture("trace-owner");
    let port = owner.launched.as_ref().unwrap()["cdp_port"]
        .as_u64()
        .unwrap();
    let guest = Pokit::attach("trace-guest", port);
    assert_eq!(guest.run(&["trace", "start"]).code, 0);
    assert_eq!(guest.run(&["close"]).code, 0);

    let fresh = Pokit::attach("trace-fresh", port);
    let r = fresh.run(&["trace", "start"]);
    assert_eq!(
        r.code, 0,
        "the earlier session's trace is still running: {}",
        r.out
    );
    assert_eq!(fresh.run(&["trace", "stop"]).code, 0);
}

/// The most memory the process `pid` has held in its working set so far, in bytes.
fn peak_working_set(pid: u64) -> u64 {
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command"])
        .arg(format!("(Get-Process -Id {pid}).PeakWorkingSet64"))
        .output()
        .expect("powershell did not start");
    let text = String::from_utf8_lossy(&out.stdout);
    text.trim()
        .parse()
        .unwrap_or_else(|_| panic!("no working set for {pid}: {text}"))
}

#[test]
fn stopping_a_long_trace_holds_less_memory_than_the_trace_itself() {
    let p = Pokit::launch_fixture("trace-long");
    let session: Value =
        serde_json::from_slice(&std::fs::read(p.home.join("session.json")).unwrap()).unwrap();
    let pid = session["pid"].as_u64().unwrap();
    assert_eq!(p.run(&["trace", "start"]).code, 0);
    let r = p.run(&[
        "hold",
        "KeyA",
        "--count",
        "1000",
        "--interval",
        "20",
        "--into",
        "#stall",
    ]);
    assert_eq!(r.code, 0, "{}", r.out);
    let t = p.run(&["trace", "stop"]);
    assert_eq!(t.code, 0, "{}", t.out);

    let size = std::fs::metadata(t.out["path"].as_str().unwrap())
        .unwrap()
        .len();
    let peak = peak_working_set(pid);
    eprintln!(
        "trace {size} bytes, {} events; session peak {peak} bytes",
        t.out["events"]
    );
    assert!(
        size > 50 << 20,
        "the trace is too small to show anything: {size} bytes"
    );
    assert!(
        peak < size,
        "the session peaked at {peak} bytes, more than the {size}-byte trace it stopped"
    );
}

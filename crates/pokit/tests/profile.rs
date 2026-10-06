//! `trace` and `profile`, against the fixture app's stall field.

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
    let p = Pokit::launch_fixture("trace-stall");
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
    let p = Pokit::launch_fixture("profile-stall");
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
    let p = Pokit::launch_fixture("profile-misuse");
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
fn a_reload_during_a_trace_or_profile_voids_the_run_and_keeps_the_file() {
    let p = Pokit::launch_fixture("profile-reload");
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
        .unwrap()
        .to_string();
    // The attached sessions are marked launched, so that dropping them closes them.
    let mut guest = Pokit::new("trace-guest");
    let r = guest.run(&["attach", "--port", &port]);
    assert_eq!(r.code, 0, "{}", r.out);
    guest.launched = Some(r.out);
    assert_eq!(guest.run(&["trace", "start"]).code, 0);
    assert_eq!(guest.run(&["close"]).code, 0);
    guest.launched = None;

    let mut fresh = Pokit::new("trace-fresh");
    let r = fresh.run(&["attach", "--port", &port]);
    assert_eq!(r.code, 0, "{}", r.out);
    fresh.launched = Some(r.out);
    let r = fresh.run(&["trace", "start"]);
    assert_eq!(
        r.code, 0,
        "the earlier session's trace is still running: {}",
        r.out
    );
    assert_eq!(fresh.run(&["trace", "stop"]).code, 0);
}

//! `hold` and `measure`, against the fixture app's stall field.

#![cfg(windows)]

mod common;

use common::*;
use serde_json::Value;

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"))
}

/// Holds `KeyA` 20 times, 33 ms apart, into the stall field with the page stalling `stall_ms` per
/// key, inside `measure start --watch` / `measure stop`; returns the hold and the measurement.
fn held_and_measured(p: &Pokit, stall_ms: u64) -> (Value, Value) {
    let r = p.run(&["eval", &format!("window.__stallMs = {stall_ms}")]);
    assert_eq!(r.code, 0, "{}", r.out);
    let r = p.run(&[
        "measure",
        "start",
        "--watch",
        "#key-count",
        "--watch-attr",
        "data-count",
    ]);
    assert_eq!(r.code, 0, "{}", r.out);
    let hold = p.run(&[
        "hold",
        "KeyA",
        "--count",
        "20",
        "--interval",
        "33",
        "--into",
        "#stall",
    ]);
    assert_eq!(hold.code, 0, "{}", hold.out);
    let stop = p.run(&["measure", "stop"]);
    assert_eq!(stop.code, 0, "{}", stop.out);
    (hold.out, stop.out)
}

#[test]
fn a_stalling_page_is_measured_as_stalling_while_hold_keeps_its_own_pace() {
    let p = Pokit::launch_fixture("measure-stall");
    let (hold, m) = held_and_measured(&p, 60);

    assert_eq!(hold["acknowledged"], 21, "{hold}");
    let sent: Vec<f64> = hold["sent_at"].as_array().unwrap().iter().map(f).collect();
    assert_eq!(sent.len(), 21, "20 key-downs and one key-up: {hold}");
    let downs = sent[19] - sent[0];
    assert!(
        downs < 19.0 * 60.0,
        "the key-downs were spaced by the page's 60 ms stall, not sent at 33 ms: {downs} ms"
    );
    assert!(
        (19.0 * 33.0 * 0.9..19.0 * 33.0 * 1.1).contains(&downs),
        "the key-downs were not sent 33 ms apart: {downs} ms for 19 intervals"
    );

    assert_eq!(m["keys"]["handled"], 20, "{m}");
    assert_eq!(m["keys"]["repeats"], 19, "{m}");
    assert_eq!(
        m["latency"]["keys"], 20,
        "every key is paired with its send: {m}"
    );
    assert!(m["latency"]["p50_ms"].as_f64().unwrap() >= 0.0, "{m}");
    let span = f(&m["keys"]["handled_span_ms"]);
    assert!(
        (20.0 * 60.0..20.0 * 60.0 * 1.5).contains(&span),
        "handled span {span} ms is not near 20 × 60 ms: {m}"
    );
    assert!(
        m["frames"]["over_threshold"].as_u64().unwrap() >= 1,
        "no frame over 50 ms on a stalling page: {m}"
    );
    let over_ms = f(&m["frames"]["over_threshold_ms"]);
    assert!(
        (20.0 * 60.0 * 0.9..20.0 * 60.0 * 1.5).contains(&over_ms),
        "time in frames over 50 ms ({over_ms} ms) is not near 20 × 60 ms: {m}"
    );
    assert_eq!(m["watch"]["changes"], 20, "{m}");
    assert_eq!(m["watch"]["last_value"], "20", "{m}");
    assert_eq!(m["settled"], true, "{m}");
    assert_eq!(m["thresholds"]["over_ms"], 50.0, "{m}");
    assert_eq!(m["thresholds"]["quiet_ms"], 1000, "{m}");
    assert_eq!(m["hidden"]["was_hidden"], false, "{m}");
}

#[test]
fn a_page_that_does_not_stall_has_no_frames_over_50_ms() {
    let p = Pokit::launch_fixture("measure-calm");
    let (_, m) = held_and_measured(&p, 0);
    assert_eq!(m["keys"]["handled"], 20, "{m}");
    assert_eq!(m["frames"]["over_threshold"], 0, "{m}");
    assert_eq!(m["frames"]["over_threshold_ms"], 0.0, "{m}");
    assert_eq!(m["watch"]["changes"], 20, "{m}");
}

#[test]
fn input_carries_its_send_time_on_the_page_clock() {
    let p = Pokit::launch_fixture("measure-clock");
    let launched = p.launched.as_ref().unwrap();
    let uncertainty = f(&launched["clock"]["uncertainty_ms"]);
    assert!(uncertainty >= 0.0, "{launched}");

    let (hold, m) = held_and_measured(&p, 0);
    let sent = f(&hold["sent_at"][0]);
    let received = f(&m["keys"]["first_received_at"]);
    let slack = f(&hold["clock"]["uncertainty_ms"]) + 1.0;
    assert!(
        received >= sent - slack,
        "the page received the first key {} ms before pokit sent it: {hold} {m}",
        sent - received
    );
    assert!(
        received - sent < 1000.0,
        "the first key arrived {} ms after it was sent; the clocks are not aligned: {hold} {m}",
        received - sent
    );

    let key = p.run(&["key", "KeyB", "--into", "#name"]);
    assert_eq!(key.code, 0, "{}", key.out);
    assert_eq!(
        key.out["sent_at"].as_array().unwrap().len(),
        2,
        "{}",
        key.out
    );
}

#[test]
fn unwatched_fields_say_not_requested() {
    let p = Pokit::launch_fixture("measure-unwatched");
    assert_eq!(p.run(&["measure", "start"]).code, 0);
    let m = p.run(&["measure", "stop", "--quiet", "200"]);
    assert_eq!(m.code, 0, "{}", m.out);
    assert_eq!(m.out["watch"]["changes"], "not requested", "{}", m.out);
    assert_eq!(m.out["keys"]["handled"], 0, "{}", m.out);
    assert_eq!(
        m.out["keys"]["handled_span_ms"], "no keys handled",
        "{}",
        m.out
    );
}

#[test]
fn a_reload_between_start_and_stop_voids_the_run() {
    let p = Pokit::launch_fixture("measure-reload");
    assert_eq!(p.run(&["measure", "start"]).code, 0);
    let before = p.run(&["eval", "performance.timeOrigin"]).out["value"].clone();
    assert_eq!(p.run(&["eval", "location.reload(); true"]).code, 0);
    assert!(
        eventually(10_000, || {
            let now = p.run(&["eval", "performance.timeOrigin"]);
            now.code == 0 && now.out["value"] != before
        }),
        "the page did not reload"
    );
    let r = p.run(&["measure", "stop"]);
    assert_eq!(r.code, 8, "{}", r.out);
    assert_eq!(r.out["error"]["kind"], "page_reloaded", "{}", r.out);
}

#[test]
fn stop_without_start_and_a_watch_on_nothing_are_refused() {
    let p = Pokit::launch_fixture("measure-misuse");
    let r = p.run(&["measure", "stop"]);
    assert_eq!(r.code, 1, "{}", r.out);
    let r = p.run(&["measure", "start", "--watch", "#nothing-like-this"]);
    assert_eq!(r.code, 5, "{}", r.out);
    let r = p.run(&["eval", "!!window.__pokitMeasure"]);
    assert_eq!(
        r.out["value"], false,
        "a refused start left probes: {}",
        r.out
    );
}

#[test]
fn closing_an_attached_session_without_stop_leaves_no_probe() {
    let owner = Pokit::launch_fixture("measure-owner");
    let port = owner.launched.as_ref().unwrap()["cdp_port"]
        .as_u64()
        .unwrap()
        .to_string();
    // The attached sessions are marked launched, so that dropping them closes them.
    let mut guest = Pokit::new("measure-guest");
    let r = guest.run(&["attach", "--port", &port]);
    assert_eq!(r.code, 0, "{}", r.out);
    guest.launched = Some(r.out);
    assert_eq!(guest.run(&["measure", "start"]).code, 0);
    let r = owner.run(&["eval", "!!window.__pokitMeasure"]);
    assert_eq!(r.out["value"], true, "{}", r.out);

    assert_eq!(guest.run(&["close"]).code, 0);
    guest.launched = None;
    let mut fresh = Pokit::new("measure-fresh");
    let r = fresh.run(&["attach", "--port", &port]);
    assert_eq!(r.code, 0, "{}", r.out);
    fresh.launched = Some(r.out);
    let r = fresh.run(&["eval", "!!window.__pokitMeasure"]);
    assert_eq!(
        r.out["value"], false,
        "the probes outlived the session: {}",
        r.out
    );
}

#[test]
fn a_held_chord_counts_its_key_not_its_modifiers() {
    let p = Pokit::launch_fixture("measure-modifiers");
    assert_eq!(p.run(&["measure", "start"]).code, 0);
    let r = p.run(&["hold", "Ctrl+KeyB", "--count", "5", "--into", "#stall"]);
    assert_eq!(r.code, 0, "{}", r.out);
    let m = p.run(&["measure", "stop", "--quiet", "300"]);
    assert_eq!(m.code, 0, "{}", m.out);
    assert_eq!(m.out["keys"]["handled"], 5, "{}", m.out);
    assert_eq!(m.out["latency"]["keys"], 5, "{}", m.out);
    assert_eq!(m.out["latency"]["unpaired_keys"], 0, "{}", m.out);
}

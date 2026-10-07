//! `measure`: the probes installed in the page as JavaScript, and the summary of what they saw.

use crate::fields;
use crate::output::Fields;
use serde_json::{json, Value};

/// The thresholds and waits one measurement uses; every one is written into its result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    /// A frame longer than this keeps the page from counting as settled.
    pub long_frame_ms: f64,
    /// Frames longer than this are counted.
    pub over_ms: f64,
    /// How long the page must go without a long frame or a key to count as settled.
    pub quiet_ms: u64,
    /// How long `stop` waits for that before reporting the page unsettled.
    pub ceiling_ms: u64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            long_frame_ms: 25.0,
            over_ms: 50.0,
            quiet_ms: 1000,
            ceiling_ms: 10_000,
        }
    }
}

impl Thresholds {
    fn to_json(self) -> Value {
        json!({
            "long_frame_ms": self.long_frame_ms,
            "over_ms": self.over_ms,
            "quiet_ms": self.quiet_ms,
            "ceiling_ms": self.ceiling_ms,
        })
    }
}

/// What the mutation probe watches; nothing when the caller named nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct Watch {
    pub selector: String,
    pub attr: Option<String>,
}

/// Installs the probes as `window.__pokitMeasure`, tagged `id`, replacing any left from an
/// earlier start. Resolves to `{ origin }`, or `{ missing }` when the watched selector matches nothing.
pub fn install_script(id: &str, t: &Thresholds, watch: Option<&Watch>) -> String {
    let config = json!({
        "id": id,
        "longFrame": t.long_frame_ms,
        "watch": watch.map(|w| &w.selector),
        "attr": watch.and_then(|w| w.attr.as_ref()),
    });
    INSTALL.replace("__CONFIG__", &config.to_string())
}

const INSTALL: &str = r#"(() => {
    const cfg = __CONFIG__;
    if (window.__pokitMeasure) window.__pokitMeasure.remove();
    let watched = null;
    if (cfg.watch !== null) {
        watched = document.querySelector(cfg.watch);
        if (!watched) return { missing: cfg.watch };
    }
    const now = () => performance.now();
    const m = {
        id: cfg.id,
        origin: performance.timeOrigin,
        start: now(),
        frames: [],
        keys: [],
        changes: watched ? [] : null,
        hidden: document.visibilityState === 'visible' ? [] : [[now(), null]],
        lastBusy: now(),
        live: true,
    };
    let prev = null;
    const frame = (t) => {
        if (!m.live) return;
        if (prev !== null && t - prev > cfg.longFrame) m.lastBusy = t;
        prev = t;
        m.frames.push(t);
        m.raf = requestAnimationFrame(frame);
    };
    m.raf = requestAnimationFrame(frame);
    let open = null;
    const modifierKeys = ['Control', 'Shift', 'Alt', 'Meta'];
    const keyStart = (e) => {
        if (!e.isTrusted || modifierKeys.includes(e.key)) return;
        open = [e.timeStamp, now(), null, e.repeat];
        m.keys.push(open);
    };
    const keyEnd = (e) => {
        if (!e.isTrusted || !open) return;
        open[2] = now();
        m.lastBusy = open[2];
        open = null;
    };
    const visibility = () => {
        prev = null;
        const last = m.hidden[m.hidden.length - 1];
        if (document.visibilityState === 'visible') {
            if (last && last[1] === null) last[1] = now();
        } else if (!last || last[1] !== null) {
            m.hidden.push([now(), null]);
        }
    };
    window.addEventListener('keydown', keyStart, true);
    window.addEventListener('keydown', keyEnd, false);
    document.addEventListener('visibilitychange', visibility);
    let observer = null;
    if (watched) {
        const value = () => cfg.attr !== null ? watched.getAttribute(cfg.attr) : watched.textContent;
        observer = new MutationObserver((records) => {
            for (const _ of records) m.changes.push([now(), value()]);
        });
        observer.observe(watched, cfg.attr !== null
            ? { attributes: true, attributeFilter: [cfg.attr] }
            : { attributes: true, childList: true, characterData: true, subtree: true });
    }
    m.remove = () => {
        m.live = false;
        cancelAnimationFrame(m.raf);
        window.removeEventListener('keydown', keyStart, true);
        window.removeEventListener('keydown', keyEnd, false);
        document.removeEventListener('visibilitychange', visibility);
        if (observer) observer.disconnect();
        if (window.__pokitMeasure === m) delete window.__pokitMeasure;
    };
    window.__pokitMeasure = m;
    return { origin: m.origin };
})()"#;

/// The page's time origin, whether the probes tagged `id` are still installed, and how long the
/// page has gone without a long frame or a key.
pub fn state_script(id: &str) -> String {
    with_probes(
        id,
        "return { origin: performance.timeOrigin, installed: !!m, quiet_ms: m ? performance.now() - m.lastBusy : null };",
    )
}

/// Everything the probes tagged `id` recorded, every time on the page's `performance.now()`;
/// null when they are gone.
pub fn read_script(id: &str) -> String {
    with_probes(
        id,
        "if (!m) return null;
        return { origin: m.origin, start: m.start, now: performance.now(), last_busy: m.lastBusy,
                 frames: m.frames, keys: m.keys, changes: m.changes, hidden: m.hidden };",
    )
}

/// Removes the probes tagged `id`, and no others; true when there were any.
pub fn remove_script(id: &str) -> String {
    with_probes(id, "if (m) m.remove(); return !!m;")
}

/// Runs `body` with `m` bound to the probes tagged `id`, or null.
fn with_probes(id: &str, body: &str) -> String {
    format!(
        "(() => {{ const p = window.__pokitMeasure; const m = p && p.id === {} ? p : null; {body} }})()",
        json!(id)
    )
}

/// The `measure stop` result from what the probes recorded.
pub fn summarize(raw: &Value, t: &Thresholds) -> Fields {
    let origin = raw["origin"].as_f64().unwrap_or(0.0);
    let now = raw["now"].as_f64().unwrap_or(0.0);
    let times = |v: &Value| -> Vec<f64> {
        v.as_array()
            .map(|a| a.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default()
    };

    let hidden: Vec<(f64, f64)> = raw["hidden"]
        .as_array()
        .map(|spans| {
            spans
                .iter()
                .map(|s| (s[0].as_f64().unwrap_or(now), s[1].as_f64().unwrap_or(now)))
                .collect()
        })
        .unwrap_or_default();
    let frames = times(&raw["frames"]);
    let mut gaps: Vec<f64> = frames
        .windows(2)
        .filter(|w| !hidden.iter().any(|(from, to)| *from < w[1] && *to > w[0]))
        .map(|w| w[1] - w[0])
        .collect();
    gaps.sort_by(f64::total_cmp);
    let over: Vec<f64> = gaps.iter().copied().filter(|g| *g > t.over_ms).collect();
    let frames = if gaps.is_empty() {
        let none = "no frames painted";
        json!({ "count": 0, "p50_ms": none, "p95_ms": none, "max_ms": none,
                "over_threshold": 0, "over_threshold_ms": 0.0 })
    } else {
        json!({
            "count": gaps.len(),
            "p50_ms": round1(percentile(&gaps, 50.0)),
            "p95_ms": round1(percentile(&gaps, 95.0)),
            "max_ms": round1(*gaps.last().unwrap()),
            "over_threshold": over.len(),
            "over_threshold_ms": round1(over.iter().sum()),
        })
    };

    let start = raw["start"].as_f64().unwrap_or(0.0);
    let keys: Vec<&Value> = raw["keys"]
        .as_array()
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let repeats = keys.iter().filter(|k| k[3] == true).count();
    let last_input = keys.last().and_then(|k| k[0].as_f64()).unwrap_or(start);
    let settle_ms = raw["last_busy"].as_f64().unwrap_or(last_input) - last_input;
    let keys = match (keys.first(), keys.last()) {
        (Some(first), Some(last)) => {
            let begin = first[1].as_f64().unwrap_or(0.0);
            let end = last[2].as_f64().or(last[1].as_f64()).unwrap_or(begin);
            json!({
                "handled": keys.len(),
                "repeats": repeats,
                "handled_span_ms": round1(end - begin),
                "first_received_at": round1(origin + first[0].as_f64().unwrap_or(0.0)),
            })
        }
        _ => {
            let none = "no keys handled";
            json!({ "handled": 0, "repeats": 0, "handled_span_ms": none, "first_received_at": none })
        }
    };

    let watch = match raw["changes"].as_array() {
        Some(changes) => json!({
            "changes": changes.len(),
            "last_value": changes.last().map(|c| c[1].clone()).unwrap_or(Value::Null),
        }),
        None => json!({ "changes": "not requested", "last_value": "not requested" }),
    };

    let hidden_ms: f64 = hidden.iter().map(|(from, to)| to - from).sum();
    let was_hidden = !hidden.is_empty();

    fields! {
        "frames" => frames,
        "keys" => keys,
        "watch" => watch,
        "hidden" => json!({ "was_hidden": was_hidden, "hidden_ms": round1(hidden_ms) }),
        "settle_ms" => round1(settle_ms.max(0.0)),
        "thresholds" => t.to_json(),
    }
}

/// How long each key took from leaving pokit to the page starting to handle it: the measurement's
/// keys paired in order with the key-downs pokit sent during it (`sent`, page-clock epoch ms).
/// Only when the page handled exactly as many key-downs as pokit sent; otherwise no pairing can
/// be trusted, and the numbers say why.
pub fn latency(raw: &Value, sent: &[f64], uncertainty_ms: f64) -> Value {
    let origin = raw["origin"].as_f64().unwrap_or(0.0);
    let handled: Vec<f64> = raw["keys"]
        .as_array()
        .map(|keys| {
            keys.iter()
                .filter_map(|k| k[1].as_f64())
                .map(|t| origin + t)
                .collect()
        })
        .unwrap_or_default();
    let mut lat: Vec<f64> = if handled.len() == sent.len() {
        handled.iter().zip(sent).map(|(h, s)| h - s).collect()
    } else {
        Vec::new()
    };
    lat.sort_by(f64::total_cmp);
    let (p50, p95, max) = if lat.is_empty() {
        let why = if sent.is_empty() {
            json!("pokit sent no keys with `hold` during the measurement")
        } else {
            json!(format!(
                "the page handled {} key-downs and pokit sent {}, so they cannot be paired",
                handled.len(),
                sent.len()
            ))
        };
        (why.clone(), why.clone(), why)
    } else {
        (
            json!(round1(percentile(&lat, 50.0))),
            json!(round1(percentile(&lat, 95.0))),
            json!(round1(*lat.last().unwrap())),
        )
    };
    json!({
        "keys": lat.len(),
        "p50_ms": p50,
        "p95_ms": p95,
        "max_ms": max,
        "uncertainty_ms": uncertainty_ms,
        "handled_keys": handled.len(),
        "sent_keys": sent.len(),
    })
}

/// The nearest-rank percentile of sorted values.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0 + 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw() -> Value {
        json!({
            "origin": 1000.0, "start": 0.0, "now": 500.0, "last_busy": 132.0,
            "frames": [0.0, 16.0, 32.0, 92.0, 108.0, 124.0],
            "keys": [[10.0, 12.0, 72.0, false], [40.0, 72.0, 132.0, true], [70.0, 132.0, null, true]],
            "changes": [[72.0, "1"], [132.0, "2"]],
            "hidden": [[200.0, 260.0], [400.0, null]],
        })
    }

    #[test]
    fn frames_are_summarised_from_the_gaps_between_them() {
        let s = summarize(&raw(), &Thresholds::default());
        assert_eq!(
            s["frames"],
            json!({ "count": 5, "p50_ms": 16.0, "p95_ms": 60.0, "max_ms": 60.0,
                    "over_threshold": 1, "over_threshold_ms": 60.0 })
        );
    }

    #[test]
    fn percentiles_take_the_nearest_rank() {
        let values: Vec<f64> = (1..=10).map(f64::from).collect();
        assert_eq!(percentile(&values, 50.0), 5.0);
        assert_eq!(percentile(&values, 95.0), 10.0);
        assert_eq!(percentile(&values, 10.0), 1.0);
        assert_eq!(percentile(&[7.0], 50.0), 7.0);
    }

    #[test]
    fn keys_span_from_the_first_dispatch_to_the_last_end_or_start() {
        let s = summarize(&raw(), &Thresholds::default());
        assert_eq!(
            s["keys"],
            json!({ "handled": 3, "repeats": 2, "handled_span_ms": 120.0, "first_received_at": 1010.0 })
        );
    }

    #[test]
    fn settle_time_runs_from_the_last_key_received_to_the_last_busy_moment() {
        let s = summarize(&raw(), &Thresholds::default());
        assert_eq!(s["settle_ms"], 62.0);
        let mut no_keys = raw();
        no_keys["keys"] = json!([]);
        no_keys["start"] = json!(100.0);
        assert_eq!(
            summarize(&no_keys, &Thresholds::default())["settle_ms"],
            32.0
        );
    }

    #[test]
    fn a_watched_element_reports_its_changes_and_an_unwatched_one_says_not_requested() {
        let s = summarize(&raw(), &Thresholds::default());
        assert_eq!(s["watch"], json!({ "changes": 2, "last_value": "2" }));
        let mut unwatched = raw();
        unwatched["changes"] = Value::Null;
        let s = summarize(&unwatched, &Thresholds::default());
        assert_eq!(s["watch"]["changes"], "not requested");
    }

    #[test]
    fn hidden_periods_are_reported_and_an_open_one_runs_to_now() {
        let s = summarize(&raw(), &Thresholds::default());
        assert_eq!(
            s["hidden"],
            json!({ "was_hidden": true, "hidden_ms": 160.0 })
        );
    }

    #[test]
    fn the_gap_across_a_hidden_period_is_not_a_frame() {
        let raw = json!({
            "origin": 0.0, "start": 0.0, "now": 1200.0,
            "frames": [0.0, 16.0, 32.0, 900.0, 916.0, 932.0],
            "keys": [], "changes": null,
            "hidden": [[40.0, 880.0]],
        });
        let s = summarize(&raw, &Thresholds::default());
        assert_eq!(
            s["frames"],
            json!({ "count": 4, "p50_ms": 16.0, "p95_ms": 16.0, "max_ms": 16.0,
                    "over_threshold": 0, "over_threshold_ms": 0.0 })
        );
        assert_eq!(
            s["hidden"],
            json!({ "was_hidden": true, "hidden_ms": 840.0 })
        );
    }

    #[test]
    fn missing_frames_and_keys_carry_a_reason_not_a_zero() {
        let empty = json!({ "origin": 0.0, "now": 0.0, "frames": [5.0], "keys": [], "changes": null, "hidden": [] });
        let s = summarize(&empty, &Thresholds::default());
        assert_eq!(s["frames"]["p50_ms"], "no frames painted");
        assert_eq!(s["keys"]["handled_span_ms"], "no keys handled");
        assert_eq!(s["hidden"]["was_hidden"], false);
    }

    #[test]
    fn every_threshold_used_is_in_the_result() {
        let t = Thresholds {
            long_frame_ms: 30.0,
            over_ms: 40.0,
            quiet_ms: 500,
            ceiling_ms: 2000,
        };
        let s = summarize(&raw(), &t);
        assert_eq!(
            s["thresholds"],
            json!({ "long_frame_ms": 30.0, "over_ms": 40.0, "quiet_ms": 500, "ceiling_ms": 2000 })
        );
        assert_eq!(s["frames"]["over_threshold"], 1);
    }

    #[test]
    fn latency_pairs_keys_with_sends_in_order() {
        let raw = json!({
            "origin": 1000.0,
            "keys": [[0.0, 10.0, 12.0, false], [0.0, 50.0, 52.0, true], [0.0, 95.0, 96.0, true]],
        });
        let l = latency(&raw, &[1005.0, 1040.0, 1080.0], 0.3);
        assert_eq!(
            l,
            json!({ "keys": 3, "p50_ms": 10.0, "p95_ms": 15.0, "max_ms": 15.0,
                    "uncertainty_ms": 0.3, "handled_keys": 3, "sent_keys": 3 })
        );
        let none = latency(&raw, &[], 0.3);
        assert_eq!(none["keys"], 0);
        assert_eq!(
            none["p50_ms"],
            "pokit sent no keys with `hold` during the measurement"
        );
    }

    #[test]
    fn latency_is_not_paired_when_the_page_saw_other_keys_than_pokit_sent() {
        let raw = json!({
            "origin": 1000.0,
            "keys": [[0.0, 10.0, 12.0, false], [0.0, 50.0, 52.0, true], [0.0, 95.0, 96.0, true]],
        });
        let l = latency(&raw, &[1005.0, 1040.0, 1080.0, 1120.0], 0.3);
        assert_eq!(l["keys"], 0);
        assert_eq!(
            l["p95_ms"],
            "the page handled 3 key-downs and pokit sent 4, so they cannot be paired"
        );
        assert_eq!(l["handled_keys"], 3);
        assert_eq!(l["sent_keys"], 4);
    }

    #[test]
    fn the_install_script_carries_its_config_as_json() {
        let w = Watch {
            selector: "#a\"b".into(),
            attr: Some("data-count".into()),
        };
        let s = install_script("t1", &Thresholds::default(), Some(&w));
        assert!(
            s.contains(r##"{"attr":"data-count","id":"t1","longFrame":25.0,"watch":"#a\"b"}"##),
            "{s}"
        );
        assert!(!s.contains("__CONFIG__"));
    }
}

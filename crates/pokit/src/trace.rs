//! `trace`: what to record, and the busiest renderer main thread's time split into groups.

use crate::fields;
use crate::output::Fields;
use serde_json::{json, Value};
use std::collections::HashMap;

/// The trace categories recorded; everything else is excluded.
pub const CATEGORIES: &[&str] = &[
    "devtools.timeline",
    "disabled-by-default-devtools.timeline",
    "disabled-by-default-devtools.timeline.frame",
    "toplevel",
    "v8",
    "v8.execute",
    "blink.user_timing",
];

/// The groups a main thread's time is split into, in output order.
pub const GROUPS: &[&str] = &["script", "style", "layout", "paint", "gc", "other"];

/// The group an event names, if its name is one a group claims.
fn group_of(name: &str) -> Option<&'static str> {
    Some(match name {
        "EventDispatch"
        | "EvaluateScript"
        | "v8.evaluateModule"
        | "FunctionCall"
        | "TimerFire"
        | "FireIdleCallback"
        | "FireAnimationFrame"
        | "RunMicrotasks"
        | "V8.Execute"
        | "v8.compile"
        | "v8.compileModule"
        | "v8.parseOnBackground" => "script",
        "ScheduleStyleRecalculation" | "UpdateLayoutTree" | "RecalculateStyles" => "style",
        "InvalidateLayout" | "Layout" => "layout",
        "Animation" | "HitTest" | "PaintSetup" | "Paint" | "PaintImage" | "RasterTask"
        | "ScrollLayer" | "UpdateLayer" | "UpdateLayerTree" | "CompositeLayers" | "PrePaint"
        | "Layerize" => "paint",
        "MinorGC"
        | "MajorGC"
        | "BlinkGC.AtomicPhase"
        | "ThreadState::performIdleLazySweep"
        | "ThreadState::completeSweep"
        | "BlinkGCMarking" => "gc",
        _ => return None,
    })
}

/// One timed slice of a thread, in microseconds.
#[derive(Debug, Clone)]
struct Slice {
    name: String,
    ts: f64,
    dur: f64,
}

/// The trace's events, from either the `{ traceEvents }` object or a bare array.
pub fn events(trace: &Value) -> &[Value] {
    trace
        .get("traceEvents")
        .unwrap_or(trace)
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// The busiest renderer main thread's time, by group, from a trace's events.
pub fn summarize(events: &[Value]) -> Fields {
    let mut names: HashMap<(i64, i64), String> = HashMap::new();
    let mut slices: HashMap<(i64, i64), Vec<Slice>> = HashMap::new();
    let mut open: HashMap<(i64, i64), Vec<(String, f64)>> = HashMap::new();
    for e in events {
        let thread = (
            e["pid"].as_i64().unwrap_or(0),
            e["tid"].as_i64().unwrap_or(0),
        );
        let name = e["name"].as_str().unwrap_or("").to_string();
        let ts = e["ts"].as_f64().unwrap_or(0.0);
        match e["ph"].as_str() {
            Some("M") if name == "thread_name" => {
                if let Some(n) = e["args"]["name"].as_str() {
                    names.insert(thread, n.to_string());
                }
            }
            Some("X") => {
                if let Some(dur) = e["dur"].as_f64() {
                    slices
                        .entry(thread)
                        .or_default()
                        .push(Slice { name, ts, dur });
                }
            }
            Some("B") => open.entry(thread).or_default().push((name, ts)),
            Some("E") => {
                if let Some((name, start)) = open.get_mut(&thread).and_then(Vec::pop) {
                    slices.entry(thread).or_default().push(Slice {
                        name,
                        ts: start,
                        dur: ts - start,
                    });
                }
            }
            _ => {}
        }
    }

    let busiest = slices
        .iter_mut()
        .filter(|(t, _)| names.get(t).map(String::as_str) == Some("CrRendererMain"))
        .map(|(t, s)| {
            s.sort_by(|a, b| a.ts.total_cmp(&b.ts).then(b.dur.total_cmp(&a.dur)));
            let busy = top_level(s).iter().map(|x| x.dur).sum::<f64>();
            (*t, busy)
        })
        .max_by(|a, b| a.1.total_cmp(&b.1));

    let Some((thread, busy)) = busiest else {
        return fields! { "thread" => "no renderer main thread in the trace" };
    };
    let mut groups: HashMap<&str, f64> = GROUPS.iter().map(|g| (*g, 0.0)).collect();
    attribute(&slices[&thread], &mut groups);
    let window = slices[&thread]
        .iter()
        .fold((f64::MAX, f64::MIN), |(lo, hi), s| {
            (lo.min(s.ts), hi.max(s.ts + s.dur))
        });
    let by_group: serde_json::Map<String, Value> = GROUPS
        .iter()
        .map(|g| (g.to_string(), json!(ms(groups[g]))))
        .collect();
    fields! {
        "thread" => json!({ "pid": thread.0, "tid": thread.1, "name": "CrRendererMain" }),
        "window_ms" => ms((window.1 - window.0).max(0.0)),
        "busy_ms" => ms(busy),
        "groups_ms" => Value::Object(by_group),
    }
}

/// The slices no other slice contains, from slices sorted by start then longest first.
fn top_level(sorted: &[Slice]) -> Vec<&Slice> {
    let mut out: Vec<&Slice> = Vec::new();
    for s in sorted {
        match out.last() {
            Some(last) if s.ts < last.ts + last.dur => {}
            _ => out.push(s),
        }
    }
    out
}

/// Adds each slice's self time (its duration less its children's) to its group: the group its
/// name claims, else its parent's, else `other`.
fn attribute(sorted: &[Slice], groups: &mut HashMap<&str, f64>) {
    // The open slices: (end, group, time not yet claimed by a child).
    let mut stack: Vec<(f64, &'static str, f64)> = Vec::new();
    let close = |stack: &mut Vec<(f64, &'static str, f64)>, groups: &mut HashMap<&str, f64>| {
        let (_, group, own) = stack.pop().unwrap();
        *groups.get_mut(group).unwrap() += own.max(0.0);
    };
    for s in sorted {
        while stack.last().is_some_and(|top| s.ts >= top.0) {
            close(&mut stack, groups);
        }
        let parent = stack.last().map(|top| top.1);
        if let Some(top) = stack.last_mut() {
            top.2 -= s.dur;
        }
        let group = group_of(&s.name).or(parent).unwrap_or("other");
        stack.push((s.ts + s.dur, group, s.dur));
    }
    while !stack.is_empty() {
        close(&mut stack, groups);
    }
}

/// Microseconds as milliseconds, to a tenth.
fn ms(us: f64) -> f64 {
    (us / 100.0).round() / 10.0 + 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn x(tid: i64, name: &str, ts: f64, dur: f64) -> Value {
        json!({ "ph": "X", "pid": 1, "tid": tid, "name": name, "ts": ts, "dur": dur })
    }

    fn thread(tid: i64, name: &str) -> Value {
        json!({ "ph": "M", "pid": 1, "tid": tid, "name": "thread_name", "args": { "name": name } })
    }

    #[test]
    fn self_time_goes_to_the_named_group_and_unnamed_children_inherit_it() {
        let events = [
            thread(7, "CrRendererMain"),
            x(7, "RunTask", 0.0, 10_000.0),
            x(7, "EventDispatch", 1_000.0, 6_000.0),
            x(7, "SomethingInside", 2_000.0, 2_000.0),
            x(7, "Layout", 4_000.0, 1_000.0),
            x(7, "RunTask", 20_000.0, 3_000.0),
            x(7, "MinorGC", 20_000.0, 1_000.0),
        ];
        let s = summarize(&events);
        assert_eq!(
            s["groups_ms"],
            json!({ "script": 5.0, "style": 0.0, "layout": 1.0, "paint": 0.0, "gc": 1.0, "other": 6.0 })
        );
        assert_eq!(s["busy_ms"], 13.0);
        assert_eq!(s["window_ms"], 23.0);
    }

    #[test]
    fn the_busiest_renderer_main_thread_is_chosen() {
        let events = [
            thread(1, "CrRendererMain"),
            thread(2, "CrRendererMain"),
            thread(3, "CrBrowserMain"),
            x(1, "FunctionCall", 0.0, 1_000.0),
            x(2, "Paint", 0.0, 4_000.0),
            x(3, "FunctionCall", 0.0, 90_000.0),
        ];
        let s = summarize(&events);
        assert_eq!(s["thread"]["tid"], 2);
        assert_eq!(s["window_ms"], 4.0, "the window is the chosen thread's");
        assert_eq!(s["groups_ms"]["paint"], 4.0);
        assert_eq!(s["groups_ms"]["script"], 0.0);
    }

    #[test]
    fn begin_and_end_pairs_count_like_complete_events() {
        let events = [
            thread(7, "CrRendererMain"),
            json!({ "ph": "B", "pid": 1, "tid": 7, "name": "FunctionCall", "ts": 0.0 }),
            json!({ "ph": "E", "pid": 1, "tid": 7, "name": "FunctionCall", "ts": 3_000.0 }),
        ];
        assert_eq!(summarize(&events)["groups_ms"]["script"], 3.0);
    }

    #[test]
    fn a_trace_with_no_renderer_main_thread_says_so() {
        let s = summarize(&[x(1, "FunctionCall", 0.0, 1.0)]);
        assert_eq!(s["thread"], "no renderer main thread in the trace");
    }

    #[test]
    fn events_come_from_the_object_or_a_bare_array() {
        let e = x(1, "Paint", 0.0, 1.0);
        assert_eq!(events(&json!({ "traceEvents": [e.clone()] })).len(), 1);
        assert_eq!(events(&json!([e])).len(), 1);
    }
}

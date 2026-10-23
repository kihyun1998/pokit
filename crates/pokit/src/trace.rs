//! `trace`: what to record, and the busiest renderer main thread's time split into groups.

use crate::fields;
use crate::output::Fields;
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Read;

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

/// One timed slice of a thread, in microseconds, with the group its name claims.
#[derive(Debug, Clone)]
struct Slice {
    group: Option<&'static str>,
    ts: f64,
    dur: f64,
}

/// One trace event, as far as the summary reads it.
#[derive(Deserialize)]
struct Event {
    #[serde(default)]
    ph: Value,
    #[serde(default)]
    pid: Value,
    #[serde(default)]
    tid: Value,
    #[serde(default)]
    name: Value,
    #[serde(default)]
    ts: Value,
    #[serde(default)]
    dur: Value,
    #[serde(default)]
    args: Args,
}

#[derive(Deserialize, Default)]
struct Args {
    #[serde(default)]
    name: Value,
}

/// A thread: its process id and thread id.
type Thread = (i64, i64);

/// Each thread's named slices, gathered one event at a time.
#[derive(Default)]
struct Threads {
    events: usize,
    names: HashMap<Thread, String>,
    slices: HashMap<Thread, Vec<Slice>>,
    open: HashMap<Thread, Vec<(Option<&'static str>, f64)>>,
}

impl Threads {
    fn add(&mut self, e: Event) {
        self.events += 1;
        let thread = (e.pid.as_i64().unwrap_or(0), e.tid.as_i64().unwrap_or(0));
        let name = e.name.as_str().unwrap_or("");
        let ts = e.ts.as_f64().unwrap_or(0.0);
        match e.ph.as_str() {
            Some("M") if name == "thread_name" => {
                if let Some(n) = e.args.name.as_str() {
                    self.names.insert(thread, n.to_string());
                }
            }
            Some("X") => {
                if let Some(dur) = e.dur.as_f64() {
                    self.slices.entry(thread).or_default().push(Slice {
                        group: group_of(name),
                        ts,
                        dur,
                    });
                }
            }
            Some("B") => self
                .open
                .entry(thread)
                .or_default()
                .push((group_of(name), ts)),
            Some("E") => {
                if let Some((group, start)) = self.open.get_mut(&thread).and_then(Vec::pop) {
                    self.slices.entry(thread).or_default().push(Slice {
                        group,
                        ts: start,
                        dur: ts - start,
                    });
                }
            }
            _ => {}
        }
    }
}

/// Reads a trace's events into `Threads`, from either the `{ traceEvents }` object or a bare
/// array, one event at a time.
struct Trace<'a>(&'a mut Threads);

impl<'de> DeserializeSeed<'de> for Trace<'_> {
    type Value = ();

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Trace<'_> {
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a trace: an array of events or an object with `traceEvents`")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(e) = seq.next_element::<Event>()? {
            self.0.add(e);
        }
        Ok(())
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(key) = map.next_key::<String>()? {
            if key == "traceEvents" {
                map.next_value_seed(Trace(&mut *self.0))?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(())
    }
}

/// The number of events and the busiest renderer main thread's time, by group, from a trace
/// in JSON read from `reader`.
pub fn summarize(reader: impl Read) -> serde_json::Result<Fields> {
    let mut threads = Threads::default();
    let mut de = serde_json::Deserializer::from_reader(std::io::BufReader::new(reader));
    Trace(&mut threads).deserialize(&mut de)?;
    de.end()?;
    let events = threads.events;
    let mut f = busiest_main_thread(threads);
    f.insert("events".into(), json!(events));
    Ok(f)
}

/// The busiest renderer main thread's time, by group.
fn busiest_main_thread(t: Threads) -> Fields {
    let Threads {
        names, mut slices, ..
    } = t;
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
        let group = s.group.or(parent).unwrap_or("other");
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
        let s = summarize_events(&events);
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
        let s = summarize_events(&events);
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
        assert_eq!(summarize_events(&events)["groups_ms"]["script"], 3.0);
    }

    #[test]
    fn a_trace_with_no_renderer_main_thread_says_so() {
        let s = summarize_events(&[x(1, "FunctionCall", 0.0, 1.0)]);
        assert_eq!(s["thread"], "no renderer main thread in the trace");
    }

    #[test]
    fn events_come_from_the_object_or_a_bare_array() {
        let e = [thread(7, "CrRendererMain"), x(7, "Paint", 0.0, 1_000.0)];
        let object = json!({ "metadata": { "traceEvents": [] }, "traceEvents": e, "after": [1] });
        for trace in [object, json!(e)] {
            let s = summarize(trace.to_string().as_bytes()).unwrap();
            assert_eq!(s["events"], 2, "{trace}");
            assert_eq!(s["groups_ms"]["paint"], 1.0, "{trace}");
        }
    }

    #[test]
    fn an_event_s_other_fields_and_odd_values_are_read_past() {
        let events = [
            thread(7, "CrRendererMain"),
            json!({ "ph": "X", "pid": 1, "tid": 7, "name": "Paint", "ts": 0.0, "dur": 2_000.0,
                    "args": { "data": { "frame": "F", "nodes": [1, 2] } }, "cat": "devtools.timeline" }),
            json!({ "ph": "M", "pid": 1, "tid": 7, "name": "thread_name", "args": { "name": 5 } }),
            json!({ "ph": "X", "pid": "1", "tid": 7, "name": null, "ts": "0", "dur": 3_000.0 }),
            json!({ "ph": "i", "pid": 1, "tid": 7, "name": "Layout", "ts": 0.0, "s": "t" }),
        ];
        let s = summarize_events(&events);
        assert_eq!(s["events"], 5);
        assert_eq!(s["thread"]["tid"], 7, "{s:?}");
        assert_eq!(s["groups_ms"]["paint"], 2.0);
    }

    #[test]
    fn a_trace_cut_short_is_an_error() {
        let text = json!({ "traceEvents": [x(1, "Paint", 0.0, 1.0)] }).to_string();
        assert!(summarize(&text.as_bytes()[..text.len() - 2]).is_err());
        assert!(summarize(format!("{text} {{").as_bytes()).is_err());
    }

    fn summarize_events(events: &[Value]) -> Fields {
        summarize(json!({ "traceEvents": events }).to_string().as_bytes()).unwrap()
    }
}

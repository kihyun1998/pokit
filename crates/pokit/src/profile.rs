//! `profile`: a CPU profile's self time by function and by file.

use crate::fields;
use crate::output::Fields;
use serde_json::{json, Value};
use std::collections::HashMap;

/// The profiler's own nodes, reported apart from the page's functions.
const SPECIAL: &[(&str, &str)] = &[
    ("(idle)", "idle_ms"),
    ("(program)", "program_ms"),
    ("(garbage collector)", "gc_ms"),
];

/// Self time by function and by file, the `top` largest of each, from a `Profiler.Profile`.
pub fn summarize(profile: &Value, top: usize) -> Fields {
    let nodes: HashMap<i64, &Value> = profile["nodes"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|n| (n["id"].as_i64().unwrap_or(0), n))
                .collect()
        })
        .unwrap_or_default();
    let samples: Vec<i64> = list(&profile["samples"], Value::as_i64);
    let deltas: Vec<f64> = list(&profile["timeDeltas"], Value::as_f64);
    let start = profile["startTime"].as_f64().unwrap_or(0.0);
    let end = profile["endTime"].as_f64().unwrap_or(start);

    // Sample i runs from its own time to the next sample's, the last one to the profile's end.
    let mut times = Vec::with_capacity(deltas.len());
    let mut t = start;
    for d in &deltas {
        t += d;
        times.push(t);
    }
    let mut self_us: HashMap<i64, f64> = HashMap::new();
    for (i, node) in samples.iter().enumerate() {
        let (Some(at), next) = (times.get(i), times.get(i + 1).copied().unwrap_or(end)) else {
            continue;
        };
        *self_us.entry(*node).or_default() += (next - at).max(0.0);
    }

    let mut special: HashMap<&str, f64> = SPECIAL.iter().map(|(_, k)| (*k, 0.0)).collect();
    let mut by_function: HashMap<String, f64> = HashMap::new();
    let mut by_file: HashMap<String, f64> = HashMap::new();
    for (id, us) in &self_us {
        let Some(frame) = nodes.get(id).map(|n| &n["callFrame"]) else {
            continue;
        };
        let name = frame["functionName"].as_str().unwrap_or("");
        if name == "(root)" {
            continue;
        }
        if let Some((_, key)) = SPECIAL.iter().find(|(n, _)| *n == name) {
            *special.get_mut(key).unwrap() += us;
            continue;
        }
        *by_function.entry(location(frame)).or_default() += us;
        let url = frame["url"].as_str().unwrap_or("");
        let file = if url.is_empty() { "(native)" } else { url };
        *by_file.entry(file.to_string()).or_default() += us;
    }

    let mut f = fields! {
        "duration_ms" => ms(end - start),
        "functions" => ranked(by_function, "function", top),
        "files" => ranked(by_file, "file", top),
    };
    for (_, key) in SPECIAL {
        f.insert(key.to_string(), json!(ms(special[key])));
    }
    f
}

/// `name @url:line:col`, with the line and column counted from 1.
fn location(frame: &Value) -> String {
    let name = match frame["functionName"].as_str().unwrap_or("") {
        "" => "(anonymous)",
        n => n,
    };
    let url = frame["url"].as_str().unwrap_or("");
    if url.is_empty() {
        return name.to_string();
    }
    let line = frame["lineNumber"].as_i64().unwrap_or(-1) + 1;
    let col = frame["columnNumber"].as_i64().unwrap_or(-1) + 1;
    format!("{name} @{url}:{line}:{col}")
}

fn ranked(totals: HashMap<String, f64>, key: &str, top: usize) -> Value {
    let mut v: Vec<(String, f64)> = totals.into_iter().collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Value::Array(
        v.into_iter()
            .take(top)
            .map(|(name, us)| json!({ key: name, "self_ms": ms(us) }))
            .collect(),
    )
}

fn list<T>(v: &Value, get: impl Fn(&Value) -> Option<T>) -> Vec<T> {
    v.as_array()
        .map(|a| a.iter().filter_map(get).collect())
        .unwrap_or_default()
}

/// Microseconds as milliseconds, to a hundredth.
fn ms(us: f64) -> f64 {
    (us / 10.0).round() / 100.0 + 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: i64, name: &str, url: &str, line: i64, col: i64) -> Value {
        json!({ "id": id, "callFrame": { "functionName": name, "url": url,
                "lineNumber": line, "columnNumber": col, "scriptId": "1" } })
    }

    fn profile() -> Value {
        json!({
            "nodes": [
                node(1, "(root)", "", -1, -1),
                node(2, "stallOnKey", "http://app/index.html", 113, 4),
                node(3, "", "http://app/a.min.js", 0, 1200),
                node(4, "(idle)", "", -1, -1),
                node(5, "now", "", -1, -1),
            ],
            "startTime": 1000.0,
            "endTime": 2000.0,
            "samples": [2, 2, 3, 4, 5, 2],
            "timeDeltas": [0.0, 100.0, 300.0, 100.0, 100.0, 100.0],
        })
    }

    #[test]
    fn each_sample_runs_until_the_next_and_the_last_until_the_end() {
        let s = summarize(&profile(), 10);
        assert_eq!(
            s["functions"],
            json!([
                { "function": "stallOnKey @http://app/index.html:114:5", "self_ms": 0.7 },
                { "function": "(anonymous) @http://app/a.min.js:1:1201", "self_ms": 0.1 },
                { "function": "now", "self_ms": 0.1 },
            ])
        );
        assert_eq!(s["idle_ms"], 0.1);
        assert_eq!(s["duration_ms"], 1.0);
    }

    #[test]
    fn files_add_up_their_functions_and_native_code_has_no_file() {
        let s = summarize(&profile(), 10);
        assert_eq!(
            s["files"],
            json!([
                { "file": "http://app/index.html", "self_ms": 0.7 },
                { "file": "(native)", "self_ms": 0.1 },
                { "file": "http://app/a.min.js", "self_ms": 0.1 },
            ])
        );
    }

    #[test]
    fn only_the_top_entries_are_kept() {
        let s = summarize(&profile(), 1);
        assert_eq!(s["functions"].as_array().unwrap().len(), 1);
        assert_eq!(s["files"].as_array().unwrap().len(), 1);
    }
}

//! The session's log buffer: page console output, uncaught errors, backend output and its own notes.

use super::State;
use serde_json::{json, Value};

/// The most log entries kept; older ones are dropped first.
const MAX_LOGS: usize = 10_000;

pub(super) struct LogEntry {
    seq: u64,
    t_ms: u128,
    source: &'static str,
    level: String,
    target: String,
    text: String,
}

impl State {
    pub(super) fn log(&self, source: &'static str, level: &str, target: &str, text: String) {
        let seq = {
            let mut next = self.next_log.lock().unwrap();
            let seq = *next;
            *next += 1;
            seq
        };
        let mut logs = self.logs.lock().unwrap();
        logs.push(LogEntry {
            seq,
            t_ms: self.started.elapsed().as_millis(),
            source,
            level: level.to_string(),
            target: target.to_string(),
            text,
        });
        if logs.len() > MAX_LOGS {
            let excess = logs.len() - MAX_LOGS;
            logs.drain(..excess);
        }
    }

    /// The last few lines of backend output, for a launch that failed.
    pub(super) fn backend_tail(&self) -> String {
        let logs = self.logs.lock().unwrap();
        let tail: Vec<&str> = logs
            .iter()
            .rev()
            .filter(|l| l.source.starts_with("backend"))
            .take(5)
            .map(|l| l.text.as_str())
            .collect();
        if tail.is_empty() {
            "no backend output".into()
        } else {
            format!(
                "last backend output: {}",
                tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
            )
        }
    }

    /// Entries after sequence number `since`, and the number to pass next time.
    pub(super) fn logs_since(&self, since: u64) -> (Vec<Value>, u64) {
        let logs = self.logs.lock().unwrap();
        let entries: Vec<Value> = logs
            .iter()
            .filter(|l| l.seq > since)
            .map(|l| json!({ "seq": l.seq, "t_ms": l.t_ms as u64, "source": l.source, "level": l.level, "target": l.target, "text": l.text }))
            .collect();
        let next = logs.last().map(|l| l.seq).unwrap_or(since);
        (entries, next)
    }
}

//! The run record: one JSON line per command and event, and a screenshot where a command failed.

use super::State;
use crate::cdp::Cdp;
use crate::output::{Failure, Kind};
use base64::Engine as _;
use serde_json::{json, Value};
use std::path::Path;

impl State {
    pub(super) fn record(&self, command: &str, args: &Value, code: i32, out: &Value) {
        let mut seq = self.record_seq.lock().unwrap();
        *seq += 1;
        let line = json!({
            "seq": *seq,
            "t_ms": self.started.elapsed().as_millis() as u64,
            "command": command,
            "args": args,
            "exit": code,
            "output": out,
        });
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.record_dir.join("record.jsonl"))
        {
            let _ = writeln!(f, "{line}");
        }
    }

    pub(super) async fn failure_screenshot(&self) -> Option<String> {
        let (_, cdp) = self.current().ok()?;
        let seq = *self.record_seq.lock().unwrap() + 1;
        let path = self.record_dir.join(format!("fail-{seq}.png"));
        screenshot_to(&cdp, json!({ "format": "png" }), &path)
            .await
            .ok()?;
        Some(path.display().to_string())
    }
}

/// Takes a screenshot with `params` and writes it to `path`; returns its size in bytes.
pub(super) async fn screenshot_to(cdp: &Cdp, params: Value, path: &Path) -> Result<usize, Failure> {
    let shot = cdp.call("Page.captureScreenshot", params).await?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(shot["data"].as_str().unwrap_or(""))
        .map_err(|e| Failure::new(Kind::Error, format!("screenshot data: {e}")))?;
    std::fs::write(path, &bytes).map_err(|e| {
        Failure::new(
            Kind::Error,
            format!("could not write {}: {e}", path.display()),
        )
    })?;
    Ok(bytes.len())
}

//! The checks `doctor` runs against a debugging port, with or without a session.

use crate::fields;
use crate::output::{Failure, Kind, Outcome};
use serde_json::{json, Value};

/// The checks `doctor` runs against a debugging port.
pub async fn checks(port: u16) -> Vec<Value> {
    let mut checks = Vec::new();
    match crate::devtools::get_json(port, "/json/version").await {
        Ok(v) => {
            checks.push(json!({ "check": "port_reachable", "ok": true, "detail": format!("127.0.0.1:{port}") }));
            let browser = v["Browser"].as_str().unwrap_or("").to_string();
            checks.push(json!({ "check": "webview2", "ok": browser.starts_with("Edg/"), "detail": browser }));
        }
        Err(e) => checks.push(json!({ "check": "port_reachable", "ok": false, "detail": e })),
    }
    if let Ok(pages) = crate::devtools::pages(port).await {
        checks.push(
            json!({ "check": "page_targets", "ok": !pages.is_empty(), "detail": pages.len() }),
        );
    }
    checks
}

/// Passes when every check passed; otherwise fails with the names of the ones that did not.
pub fn outcome(checks: Vec<Value>) -> Outcome {
    let failed: Vec<String> = checks
        .iter()
        .filter(|c| c["ok"] != true)
        .filter_map(|c| c["check"].as_str().map(String::from))
        .collect();
    if failed.is_empty() {
        Ok(fields! { "checks" => checks })
    } else {
        Err(
            Failure::new(Kind::CheckFailed, format!("failed: {}", failed.join(", ")))
                .with("checks", checks),
        )
    }
}

//! The app's open pages: one connection per page target, their events, and whether each is ready
//! for input.

use super::commands::exception_text;
use super::launch::probe_ready;
use super::{State, Target};
use crate::cdp::{Cdp, Event};
use crate::output::{Failure, Kind};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Keeps a connection to every page target as windows open and close.
pub(super) async fn discover(state: Arc<State>) {
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Ok(pages) = crate::devtools::pages(state.config.cdp_port).await {
            state.sync_targets(&pages).await;
        }
    }
}

pub(super) async fn collect_events(state: Arc<State>, mut rx: mpsc::UnboundedReceiver<Event>) {
    while let Some((target, method, params)) = rx.recv().await {
        let url = state
            .targets
            .lock()
            .unwrap()
            .get(&target)
            .map(|t| t.url.clone())
            .unwrap_or_default();
        match method.as_str() {
            "Runtime.consoleAPICalled" => {
                let text = params["args"]
                    .as_array()
                    .map(|args| args.iter().map(remote_text).collect::<Vec<_>>().join(" "))
                    .unwrap_or_default();
                let level = params["type"].as_str().unwrap_or("log").to_string();
                state.log("console", &level, &url, text);
            }
            "Runtime.exceptionThrown" => {
                state.log(
                    "exception",
                    "error",
                    &url,
                    exception_text(&params["exceptionDetails"]),
                );
            }
            "Runtime.executionContextCreated" => {
                let aux = &params["context"]["auxData"];
                if aux["isDefault"] == true && aux["frameId"].as_str() == Some(target.as_str()) {
                    state.ready.lock().unwrap().remove(&target);
                }
            }
            _ => {}
        }
    }
}

fn remote_text(arg: &Value) -> String {
    match &arg["value"] {
        Value::String(s) => s.clone(),
        Value::Null => arg["description"]
            .as_str()
            .or(arg["unserializableValue"].as_str())
            .unwrap_or(arg["type"].as_str().unwrap_or(""))
            .to_string(),
        v => v.to_string(),
    }
}

impl State {
    /// Connects to page targets not yet held and drops ones that are gone. Only page sockets on
    /// this instance's own debugging port are followed.
    pub(super) async fn sync_targets(&self, pages: &[Value]) {
        let _one_at_a_time = self.sync_lock.lock().await;
        let port = self.config.cdp_port;
        let allowed = [
            format!("ws://127.0.0.1:{port}/"),
            format!("ws://localhost:{port}/"),
        ];
        for p in pages {
            let id = p["id"].as_str().unwrap_or("").to_string();
            let url = p["url"].as_str().unwrap_or("").to_string();
            let title = p["title"].as_str().unwrap_or("").to_string();
            let known = {
                let mut targets = self.targets.lock().unwrap();
                match targets.get_mut(&id) {
                    Some(t) if !t.cdp.is_closed() => {
                        t.url = url.clone();
                        t.title = title.clone();
                        true
                    }
                    _ => false,
                }
            };
            if known {
                continue;
            }
            let Some(ws) = p["webSocketDebuggerUrl"].as_str() else {
                continue;
            };
            if !allowed.iter().any(|a| ws.starts_with(a.as_str())) {
                self.log(
                    "session",
                    "warning",
                    &url,
                    format!("ignored a page socket off this instance's port: {ws}"),
                );
                continue;
            }
            if let Ok(cdp) = Cdp::connect(ws, id.clone(), self.events_tx.clone()).await {
                let _ = cdp.call("Runtime.enable", json!({})).await;
                self.targets
                    .lock()
                    .unwrap()
                    .insert(id.clone(), Target { cdp, url, title });
                let mut order = self.order.lock().unwrap();
                if !order.contains(&id) {
                    order.push(id);
                }
            }
        }
        let live: Vec<&str> = pages.iter().filter_map(|p| p["id"].as_str()).collect();
        self.targets
            .lock()
            .unwrap()
            .retain(|id, _| live.contains(&id.as_str()));
        self.order
            .lock()
            .unwrap()
            .retain(|id| live.contains(&id.as_str()));
    }

    pub(super) fn cdp(&self, id: &str) -> Result<Arc<Cdp>, Failure> {
        self.targets
            .lock()
            .unwrap()
            .get(id)
            .map(|t| t.cdp.clone())
            .ok_or_else(|| Failure::new(Kind::NotFound, format!("target {id} is no longer open")))
    }

    pub(super) fn current(&self) -> Result<(String, Arc<Cdp>), Failure> {
        let id = self.current.lock().unwrap().clone();
        let cdp = self.cdp(&id)?;
        Ok((id, cdp))
    }

    /// Waits, before input, until the target's current document is loaded, visible and painting.
    pub(super) async fn ensure_ready(&self, id: &str, cdp: &Cdp) -> Result<(), Failure> {
        if self.ready.lock().unwrap().contains(id) {
            return Ok(());
        }
        match probe_ready(cdp, "", Duration::from_secs(10)).await {
            Ok(_) => {
                self.ready.lock().unwrap().insert(id.to_string());
                Ok(())
            }
            Err(last) => Err(Failure::new(
                Kind::Timeout,
                "the page is not ready for input (loaded, visible, painting)",
            )
            .with("found", last)),
        }
    }
}

//! A hand-written CDP client over one page's WebSocket (ADR-0002): it sends only what pokit asks it to.

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

/// An event from one target: (target id, method, params).
pub type Event = (String, String, Value);

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

pub struct Cdp {
    out: mpsc::UnboundedSender<Message>,
    pending: Pending,
    next: AtomicU64,
    closed: Arc<AtomicBool>,
}

impl Cdp {
    /// Connects to a page's WebSocket; events go to `events` tagged with `target`.
    pub async fn connect(
        ws_url: &str,
        target: String,
        events: mpsc::UnboundedSender<Event>,
    ) -> Result<Arc<Cdp>, String> {
        let (ws, _) = tokio_tungstenite::connect_async(ws_url)
            .await
            .map_err(|e| format!("could not open {ws_url}: {e}"))?;
        let (mut sink, mut stream) = ws.split();
        let (out, mut out_rx) = mpsc::unbounded_channel::<Message>();
        let pending: Pending = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));

        tokio::spawn(async move {
            while let Some(msg) = out_rx.recv().await {
                if sink.send(msg).await.is_err() {
                    break;
                }
            }
        });

        let (p, c) = (pending.clone(), closed.clone());
        tokio::spawn(async move {
            while let Some(Ok(msg)) = stream.next().await {
                let Message::Text(text) = msg else { continue };
                let Ok(v) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                if let Some(id) = v["id"].as_u64() {
                    if let Some(tx) = p.lock().unwrap().remove(&id) {
                        let res = match v.get("error") {
                            Some(e) => {
                                Err(e["message"].as_str().unwrap_or("CDP error").to_string())
                            }
                            None => Ok(v["result"].clone()),
                        };
                        let _ = tx.send(res);
                    }
                } else if let Some(method) = v["method"].as_str() {
                    let _ = events.send((target.clone(), method.to_string(), v["params"].clone()));
                }
            }
            c.store(true, Ordering::SeqCst);
            for (_, tx) in p.lock().unwrap().drain() {
                let _ = tx.send(Err("the page's connection closed".into()));
            }
        });

        Ok(Arc::new(Cdp {
            out,
            pending,
            next: AtomicU64::new(1),
            closed,
        }))
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Sends `method` and waits up to 15 s for its result.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, CdpError> {
        self.call_timeout(method, params, Duration::from_secs(15))
            .await
    }

    /// Sends `method` and waits up to `timeout` for its result.
    pub async fn call_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, CdpError> {
        self.send(method, params)?.wait(timeout).await
    }

    /// Sends `method` without waiting; its result is awaited later through the returned reply.
    pub fn send(&self, method: &str, params: Value) -> Result<Reply, CdpError> {
        if self.is_closed() {
            return Err(CdpError::Closed(format!(
                "{method}: the page's connection is closed"
            )));
        }
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let msg = json!({ "id": id, "method": method, "params": params });
        if self.out.send(Message::Text(msg.to_string())).is_err() {
            self.pending.lock().unwrap().remove(&id);
            return Err(CdpError::Closed(format!(
                "{method}: the page's connection is closed"
            )));
        }
        Ok(Reply {
            id,
            method: method.to_string(),
            rx,
            pending: self.pending.clone(),
        })
    }
}

/// The result of a command already sent.
pub struct Reply {
    id: u64,
    method: String,
    rx: oneshot::Receiver<Result<Value, String>>,
    pending: Pending,
}

impl Reply {
    /// Waits up to `timeout` for the result.
    pub async fn wait(mut self, timeout: Duration) -> Result<Value, CdpError> {
        let method = &self.method;
        match tokio::time::timeout(timeout, &mut self.rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(CdpError::Protocol(format!("{method}: {e}"))),
            Ok(Err(_)) => Err(CdpError::Closed(format!("{method}: connection dropped"))),
            Err(_) => Err(CdpError::Timeout(format!(
                "{method}: no answer within {}s",
                timeout.as_secs()
            ))),
        }
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        self.pending.lock().unwrap().remove(&self.id);
    }
}

/// Why a CDP call did not return a result.
#[derive(Debug, Clone)]
pub enum CdpError {
    Timeout(String),
    Closed(String),
    Protocol(String),
}

impl std::fmt::Display for CdpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CdpError::Timeout(m) | CdpError::Closed(m) | CdpError::Protocol(m) => f.write_str(m),
        }
    }
}

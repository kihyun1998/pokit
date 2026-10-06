//! pokit's side of the test-build plugin's loopback channel (macOS): one JSON line out, one back.

use crate::output::{Failure, Kind};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// The environment variable the plugin reads its token from (`tauri_plugin_pokit::TOKEN_VAR`).
pub const TOKEN_VAR: &str = "POKIT_TOKEN";
/// The environment variable naming the file the plugin writes its port to
/// (`tauri_plugin_pokit::PORT_FILE_VAR`).
pub const PORT_FILE_VAR: &str = "POKIT_PORT_FILE";

/// How much longer than the page's own timeout pokit waits for the plugin's reply.
const REPLY_MARGIN: Duration = Duration::from_secs(2);

/// The plugin of one launched app.
pub struct Plugin {
    pub port: u16,
    token: String,
}

impl Plugin {
    pub fn new(port: u16, token: String) -> Self {
        Plugin { port, token }
    }

    /// Sends `request` with this launch's token and returns the reply's fields, or its failure.
    pub async fn request(&self, mut request: Value, timeout: Duration) -> Result<Value, Failure> {
        request["token"] = json!(self.token);
        let exchange = async {
            let stream = TcpStream::connect(("127.0.0.1", self.port)).await?;
            let (read, mut write) = stream.into_split();
            write.write_all(format!("{request}\n").as_bytes()).await?;
            let mut line = String::new();
            BufReader::new(read).read_line(&mut line).await?;
            Ok::<String, std::io::Error>(line)
        };
        let line = match tokio::time::timeout(timeout + REPLY_MARGIN, exchange).await {
            Err(_) => return Err(Failure::new(Kind::Timeout, "the plugin did not reply")),
            Ok(Err(e)) => {
                return Err(Failure::new(
                    Kind::Error,
                    format!("could not reach the plugin: {e}"),
                ))
            }
            Ok(Ok(line)) => line,
        };
        let reply: Value = serde_json::from_str(&line).map_err(|e| {
            Failure::new(Kind::Error, format!("the plugin's reply is not JSON: {e}"))
        })?;
        if reply["ok"] == true {
            return Ok(reply);
        }
        let error = &reply["error"];
        let kind = match error["kind"].as_str() {
            Some("js_error") => Kind::JsError,
            Some("timeout") => Kind::Timeout,
            Some("not_found") => Kind::NotFound,
            Some("unsupported") => Kind::Unsupported,
            _ => Kind::Error,
        };
        Err(Failure::new(
            kind,
            error["message"].as_str().unwrap_or("the plugin failed"),
        ))
    }

    /// Evaluates `js` in webview `label`, awaiting a promise; the value comes back as JSON.
    pub async fn eval(&self, label: &str, js: &str, timeout: Duration) -> Result<Value, Failure> {
        let reply = self
            .request(
                json!({ "op": "eval", "webview": label, "js": js, "timeout_ms": timeout.as_millis() as u64 }),
                timeout,
            )
            .await?;
        Ok(reply["value"].clone())
    }
}

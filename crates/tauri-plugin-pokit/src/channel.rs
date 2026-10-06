//! The loopback channel: one JSON request line in, one JSON reply line out, per connection.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};

/// The largest request line the channel reads.
const MAX_REQUEST: u64 = 4 << 20;
/// How long a connection may take to send its request line.
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the channel waits before accepting again after a failed accept.
const ACCEPT_RETRY: Duration = Duration::from_millis(50);
/// How long an `eval` may take when the request names no timeout.
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Opens the channel when `pokit launch` asked for it, and writes its port to the port file.
pub fn open<R: Runtime>(app: &AppHandle<R>) {
    let Ok(token) = std::env::var(crate::TOKEN_VAR) else {
        return;
    };
    let Some(port_file) = std::env::var_os(crate::PORT_FILE_VAR).map(PathBuf::from) else {
        return;
    };
    if token.is_empty() {
        return;
    }
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("pokit: could not open the plugin channel: {e}");
            return;
        }
    };
    let port = match listener.local_addr() {
        Ok(a) => a.port(),
        Err(e) => {
            eprintln!("pokit: could not read the plugin channel's port: {e}");
            return;
        }
    };
    if let Err(e) = write_port(&port_file, port) {
        eprintln!("pokit: could not write {}: {e}", port_file.display());
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else {
                std::thread::sleep(ACCEPT_RETRY);
                continue;
            };
            let (app, token) = (app.clone(), token.clone());
            std::thread::spawn(move || serve(stream, &app, &token));
        }
    });
}

/// Writes the port to a file beside the port file, then renames it into place.
fn write_port(path: &Path, port: u16) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, port.to_string())?;
    std::fs::rename(&tmp, path)
}

fn serve<R: Runtime>(stream: TcpStream, app: &AppHandle<R>, token: &str) {
    if stream.set_read_timeout(Some(READ_TIMEOUT)).is_err() {
        return;
    }
    let mut line = String::new();
    if BufReader::new((&stream).take(MAX_REQUEST))
        .read_line(&mut line)
        .is_err()
    {
        return;
    }
    let reply = match serde_json::from_str::<Value>(&line) {
        Err(e) => failure("bad_request", format!("not a JSON request: {e}")),
        Ok(request) if !same_token(request["token"].as_str().unwrap_or(""), token) => failure(
            "unauthorised",
            "the request does not carry this launch's token",
        ),
        Ok(request) => answer(app, &request),
    };
    let _ = (&stream).write_all(format!("{reply}\n").as_bytes());
}

/// Compares two tokens in time that depends only on their length.
fn same_token(given: &str, expected: &str) -> bool {
    given.len() == expected.len()
        && given
            .bytes()
            .zip(expected.bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

fn answer<R: Runtime>(app: &AppHandle<R>, request: &Value) -> Value {
    match request["op"].as_str() {
        Some("ping") => json!({ "ok": true }),
        Some("webviews") => {
            let webviews: Vec<Value> = app
                .webview_windows()
                .into_iter()
                .map(|(label, w)| {
                    json!({ "label": label, "url": w.url().map(|u| u.to_string()).unwrap_or_default() })
                })
                .collect();
            json!({ "ok": true, "webviews": webviews })
        }
        Some("eval") => {
            let label = request["webview"].as_str().unwrap_or("main");
            let Some(webview) = app.get_webview_window(label) else {
                return failure("not_found", format!("no webview labelled `{label}`"));
            };
            let Some(js) = request["js"].as_str() else {
                return failure("bad_request", "eval needs `js`");
            };
            let timeout =
                Duration::from_millis(request["timeout_ms"].as_u64().unwrap_or(DEFAULT_TIMEOUT_MS));
            evaluate(&webview, js, timeout)
        }
        other => failure("bad_request", format!("unknown op {other:?}")),
    }
}

#[cfg(target_os = "macos")]
fn evaluate<R: Runtime>(webview: &tauri::WebviewWindow<R>, js: &str, timeout: Duration) -> Value {
    crate::eval::evaluate(webview, js, timeout)
}

#[cfg(not(target_os = "macos"))]
fn evaluate<R: Runtime>(
    _webview: &tauri::WebviewWindow<R>,
    _js: &str,
    _timeout: Duration,
) -> Value {
    failure(
        "unsupported",
        "the plugin evaluates JavaScript on macOS only",
    )
}

/// A failed reply: `kind` is what pokit maps to its exit code.
pub fn failure(kind: &str, message: impl Into<String>) -> Value {
    json!({ "ok": false, "error": { "kind": kind, "message": message.into() } })
}

#[cfg(test)]
mod tests {
    use super::same_token;

    #[test]
    fn a_token_matches_only_itself() {
        assert!(same_token("abc123", "abc123"));
        assert!(!same_token("abc124", "abc123"));
        assert!(!same_token("abc12", "abc123"));
        assert!(!same_token("", "abc123"));
    }
}

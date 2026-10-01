//! A command's request to the session, over loopback.

use crate::home::SessionInfo;
use crate::request::Request;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Sends one request and returns the session's exit code and printed object. The listener on the
/// session port must first answer `hello` with the session's proof; nothing else is sent before that.
pub fn request(
    info: &SessionInfo,
    request: &Request,
    timeout: Duration,
) -> Result<(i32, Value), String> {
    let mut stream =
        TcpStream::connect_timeout(&([127, 0, 0, 1], info.port).into(), Duration::from_secs(3))
            .map_err(|e| format!("session is not answering on port {}: {e}", info.port))?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
    stream
        .write_all(b"{\"command\":\"hello\"}\n")
        .map_err(|e| format!("could not reach the session: {e}"))?;
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut hello = String::new();
    reader
        .read_line(&mut hello)
        .map_err(|e| format!("no answer from the session port: {e}"))?;
    let proof = serde_json::from_str::<Value>(&hello)
        .ok()
        .and_then(|v| v["proof"].as_str().map(String::from));
    if info.proof.is_empty() || proof.as_deref() != Some(info.proof.as_str()) {
        return Err(format!(
            "the process on port {} is not this session; nothing was sent",
            info.port
        ));
    }
    stream.set_read_timeout(Some(timeout)).ok();
    let req = json!({ "token": info.token, "request": request });
    stream
        .write_all(format!("{req}\n").as_bytes())
        .map_err(|e| format!("could not send to the session: {e}"))?;
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| {
        format!(
            "no answer from the session within {}s: {e}",
            timeout.as_secs()
        )
    })?;
    let resp: Value = serde_json::from_str(&line)
        .map_err(|e| format!("unreadable answer from the session: {e}"))?;
    let code = resp["exit"].as_i64().unwrap_or(1) as i32;
    Ok((code, resp["output"].clone()))
}

/// Whether the session in `info` answers a ping.
pub fn alive(info: &SessionInfo) -> bool {
    matches!(
        request(info, &Request::Ping, Duration::from_secs(3)),
        Ok((0, _))
    )
}

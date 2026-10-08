//! A command's request to the session, over loopback.

use crate::home::SessionInfo;
use crate::request::Request;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

/// How long a command waits to connect, and for the `hello` answer.
const HELLO_TIMEOUT: Duration = Duration::from_secs(3);

/// Sends one request and returns the session's exit code and printed object. The listener on the
/// session port must first answer `hello` with the session's proof; nothing else is sent before that.
pub fn request(
    info: &SessionInfo,
    request: &Request,
    timeout: Duration,
) -> Result<(i32, Value), String> {
    let stream = TcpStream::connect_timeout(&([127, 0, 0, 1], info.port).into(), HELLO_TIMEOUT)
        .map_err(|e| format!("session is not answering on port {}: {e}", info.port))?;
    stream.set_read_timeout(Some(HELLO_TIMEOUT)).ok();
    (&stream)
        .write_all(b"{\"command\":\"hello\"}\n")
        .map_err(|e| format!("could not reach the session: {e}"))?;
    let mut reader = BufReader::new(&stream);
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
    (&stream)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A session on a loopback port that answers `hello`, then the request after `delay`.
    fn slow_session(delay: Duration) -> SessionInfo {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            (&stream).write_all(b"{\"proof\":\"p\"}\n").unwrap();
            line.clear();
            reader.read_line(&mut line).unwrap();
            std::thread::sleep(delay);
            (&stream)
                .write_all(b"{\"exit\":0,\"output\":{\"ok\":true}}\n")
                .unwrap();
        });
        serde_json::from_value(json!({
            "pid": 1, "status": "ready", "error": null, "port": port, "token": "t",
            "mode": "launch", "app_pid": null, "app_exe": null, "cdp_port": 1, "proof": "p",
        }))
        .unwrap()
    }

    #[test]
    fn an_answer_slower_than_the_hello_timeout_is_still_read() {
        let info = slow_session(HELLO_TIMEOUT + Duration::from_secs(1));
        let r = request(&info, &Request::Ping, HELLO_TIMEOUT * 4);
        assert_eq!(r, Ok((0, json!({ "ok": true }))));
    }

    #[test]
    fn an_answer_slower_than_the_command_timeout_is_given_up_on() {
        let info = slow_session(Duration::from_secs(2));
        let r = request(&info, &Request::Ping, Duration::from_millis(500));
        assert!(r
            .unwrap_err()
            .starts_with("no answer from the session within"));
    }
}

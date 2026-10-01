//! The DevTools HTTP endpoints (`/json/version`, `/json/list`) on a local port.

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// GETs `path` from `127.0.0.1:port` and parses the body as JSON.
pub async fn get_json(port: u16, path: &str) -> Result<Value, String> {
    let fut = async {
        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .map_err(|e| e.to_string())?;
        let req =
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
        stream
            .write_all(req.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        // The body ends where Content-Length says.
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let n = stream.read(&mut chunk).await.map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(end) = find(&buf, b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
                let body = &buf[end + 4..];
                if let Some(len) = content_length(&head) {
                    if body.len() >= len {
                        let body = String::from_utf8_lossy(&body[..len]).to_string();
                        return serde_json::from_str(&body)
                            .map_err(|e| format!("{path} is not JSON: {e}"));
                    }
                } else if head.contains("transfer-encoding: chunked")
                    && body.ends_with(b"0\r\n\r\n")
                {
                    let body = dechunk(&String::from_utf8_lossy(body));
                    return serde_json::from_str(&body)
                        .map_err(|e| format!("{path} is not JSON: {e}"));
                }
            }
        }
        let text = String::from_utf8_lossy(&buf).to_string();
        let (_, body) = text
            .split_once("\r\n\r\n")
            .ok_or("malformed HTTP response")?;
        serde_json::from_str(body).map_err(|e| format!("{path} is not JSON: {e}"))
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), fut)
        .await
        .map_err(|_| format!("{path} on port {port} did not answer within 5s"))?
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn content_length(head: &str) -> Option<usize> {
    head.lines().find_map(|l| {
        l.strip_prefix("content-length:")
            .and_then(|v| v.trim().parse().ok())
    })
}

fn dechunk(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some((size, tail)) = rest.split_once("\r\n") {
        let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if n == 0 || tail.len() < n {
            break;
        }
        out.push_str(&tail[..n]);
        rest = tail[n..].trim_start_matches("\r\n");
    }
    out
}

/// The page targets on `port`.
pub async fn pages(port: u16) -> Result<Vec<Value>, String> {
    let list = get_json(port, "/json/list").await?;
    Ok(list
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t["type"] == "page")
        .collect())
}

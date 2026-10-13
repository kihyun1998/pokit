//! The session's start on macOS: the launched app's test build, reached through pokit's plugin.

use super::page::PluginPage;
use super::{fail_start, Config, State};
use crate::home::{self, Mode, SessionInfo};
use crate::plugin::{Plugin, PORT_FILE_VAR, TOKEN_VAR};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Whether the app's own page has loaded (not the `about:blank` a webview starts on), and is the
/// one `--main-url` names; asked of the main webview until it says so.
fn ready_probe(main_url: Option<&str>) -> String {
    let wanted = main_url
        .map(|part| format!(" && location.href.includes({})", json!(part)))
        .unwrap_or_default();
    format!("location.href !== 'about:blank' && document.readyState === 'complete'{wanted}")
}

/// Launches the app's test build and waits until its plugin answers and its main page has
/// loaded; `None` once the start has failed and said so.
pub(super) async fn start(
    config: &Config,
    info: &mut SessionInfo,
    record_dir: PathBuf,
) -> Option<(Arc<State>, Option<tokio::process::Child>)> {
    let pid = info.pid;
    if config.mode != Mode::Launch {
        fail_start(pid, config, None, "attach is not built on macOS".into());
        return None;
    }
    let instance_dir = home::home()
        .join("instances")
        .join(format!("{}-{pid}", home::stamp()));
    let plugin_token = home::token();
    let fail = |app: Option<u32>, error: String| {
        fail_start(pid, config, app, error);
        let _ = std::fs::remove_dir_all(&instance_dir);
        None
    };
    let mut child = match spawn_app(config, &instance_dir, &plugin_token) {
        Ok(c) => c,
        Err(e) => return fail(None, e),
    };
    info.app_pid = child.id();
    let deadline = Instant::now() + Duration::from_secs(config.ready_timeout_secs);
    let port = match wait_port(&instance_dir.join("port"), &mut child, deadline).await {
        Ok(p) => p,
        Err(e) => return fail(info.app_pid, e),
    };
    let plugin = Plugin::new(port, plugin_token);
    let webview = match wait_ready(&plugin, config.main_url.as_deref(), deadline).await {
        Ok(m) => m,
        Err(e) => return fail(info.app_pid, e),
    };
    let page = PluginPage { plugin, webview };
    let state = session(config.clone(), record_dir, page, info.app_pid, instance_dir);
    Some((state, Some(child)))
}

/// The session on a launched app reached through `page`, owning `instance_dir`.
fn session(
    config: Config,
    record_dir: PathBuf,
    page: PluginPage,
    app_pid: Option<u32>,
    instance_dir: PathBuf,
) -> Arc<State> {
    let (events_tx, _) = mpsc::unbounded_channel();
    Arc::new(State {
        plugin: Some(page),
        app_pid,
        owned_data_dir: Some(instance_dir),
        ..State::new(config, record_dir, events_tx)
    })
}

/// Starts the app with the plugin's token and port file in its environment.
fn spawn_app(
    config: &Config,
    instance_dir: &Path,
    plugin_token: &str,
) -> Result<tokio::process::Child, String> {
    let exe = config
        .exe
        .as_ref()
        .ok_or("launch needs the app's executable")?;
    std::fs::create_dir_all(instance_dir).map_err(|e| {
        format!(
            "could not create the instance directory {}: {e}",
            instance_dir.display()
        )
    })?;
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(&config.args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(false);
    for (k, v) in &config.env {
        cmd.env(k, v);
    }
    cmd.env(TOKEN_VAR, plugin_token)
        .env(PORT_FILE_VAR, instance_dir.join("port"));
    cmd.spawn()
        .map_err(|e| format!("could not start {exe}: {e}"))
}

/// Waits for the plugin to write its port, failing early if the app exits first.
async fn wait_port(
    path: &Path,
    child: &mut tokio::process::Child,
    deadline: Instant,
) -> Result<u16, String> {
    loop {
        if let Some(port) = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| t.trim().parse().ok())
        {
            return Ok(port);
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("the app exited before its plugin opened ({status}); is it a test build with pokit's feature on?"));
        }
        if Instant::now() > deadline {
            return Err("the app's plugin did not open its port; is it a test build with pokit's feature on?".into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Waits until the main webview's page has loaded; returns that webview's label.
async fn wait_ready(
    plugin: &Plugin,
    main_url: Option<&str>,
    deadline: Instant,
) -> Result<String, String> {
    let step = Duration::from_secs(2);
    loop {
        let main = plugin
            .request(json!({ "op": "webviews" }), step)
            .await
            .ok()
            .and_then(|reply| pick_main(&reply["webviews"], main_url));
        if let Some(main) = main {
            if plugin.eval(&main, &ready_probe(main_url), step).await.ok() == Some(json!(true)) {
                return Ok(main);
            }
        }
        if Instant::now() > deadline {
            return Err("the app's main page did not finish loading".into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The webview whose URL contains `main_url`; without one, `main`, else the first.
fn pick_main(webviews: &Value, main_url: Option<&str>) -> Option<String> {
    let list = webviews.as_array()?;
    let label = |w: &Value| w["label"].as_str().map(str::to_string);
    match main_url {
        Some(part) => list
            .iter()
            .find(|w| w["url"].as_str().is_some_and(|u| u.contains(part)))
            .and_then(label),
        None => list
            .iter()
            .find(|w| w["label"] == "main")
            .or_else(|| list.first())
            .and_then(label),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::Request;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    /// A stand-in for the app's plugin: answers each `eval` with what `answer` gives its script.
    async fn fake_plugin(answer: fn(&str) -> Value) -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                let _ = BufReader::new(read).read_line(&mut line).await;
                let req: Value = serde_json::from_str(&line).unwrap();
                let reply = json!({ "ok": true, "value": answer(req["js"].as_str().unwrap()) });
                let _ = write.write_all(format!("{reply}\n").as_bytes()).await;
            }
        });
        port
    }

    fn config() -> Config {
        Config {
            mode: Mode::Launch,
            exe: None,
            args: vec![],
            env: vec![],
            cdp_port: 0,
            data_dir: None,
            main_url: None,
            idle_timeout_secs: 600,
            ready_timeout_secs: 30,
        }
    }

    #[tokio::test]
    async fn a_session_through_the_plugin_answers_eval_and_keeps_the_run_record() {
        let port = fake_plugin(|js| {
            if js.contains("performance.timeOrigin") {
                json!(1_000_000.0)
            } else {
                json!(2)
            }
        })
        .await;
        let dir = std::env::temp_dir().join(format!("pokit-unit-{}", home::token()));
        let record_dir = dir.join("run");
        std::fs::create_dir_all(&record_dir).unwrap();
        let state = session(
            config(),
            record_dir.clone(),
            PluginPage {
                plugin: Plugin::new(port, "t".into()),
                webview: "main".into(),
            },
            None,
            dir.join("instance"),
        );
        state.align_main_clock().await;

        let status = state.status();
        assert_eq!(status["session_pid"], std::process::id());
        assert_eq!(status["record_dir"], record_dir.display().to_string());
        assert_eq!(status["clock"]["target"], "main", "{status:?}");
        assert_eq!(status["plugin_port"], port);

        let (code, out) = state
            .handle(Request::Eval {
                expression: "1 + 1".into(),
                timeout_ms: 1000,
            })
            .await;
        assert_eq!((code, &out["value"]), (0, &json!(2)), "{out}");

        let (_, out) = state.handle(Request::Snapshot).await;
        assert_eq!(out["error"]["kind"], "unsupported", "{out}");

        let record = std::fs::read_to_string(record_dir.join("record.jsonl")).unwrap();
        let commands: Vec<String> = record
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap()["command"].to_string())
            .collect();
        assert_eq!(commands, ["\"eval\"", "\"snapshot\""], "{record}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

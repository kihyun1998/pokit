//! The session on macOS: the launched app's test build, reached through pokit's plugin.

use super::{fail_start, Config, MAX_REQUEST};
use crate::fields;
use crate::home::{self, Mode, SessionInfo, Status};
use crate::output::{self, Failure, Fields, Kind, Outcome};
use crate::plugin::{Plugin, PORT_FILE_VAR, TOKEN_VAR};
use crate::request::Request;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// Whether the app's own page has loaded (not the `about:blank` a webview starts on), and is the
/// one `--main-url` names; asked of the main webview until it says so.
fn ready_probe(main_url: Option<&str>) -> String {
    let wanted = main_url
        .map(|part| format!(" && location.href.includes({})", json!(part)))
        .unwrap_or_default();
    format!("location.href !== 'about:blank' && document.readyState === 'complete'{wanted}")
}

struct PluginSession {
    config: Config,
    plugin: Plugin,
    /// The webview commands go to.
    main: String,
    app_pid: Option<u32>,
    instance_dir: PathBuf,
    last_activity: Mutex<Instant>,
}

pub(super) async fn serve(config: Config) {
    let pid = std::process::id();
    let listener = match TcpListener::bind(("127.0.0.1", 0)).await {
        Ok(l) => l,
        Err(e) => {
            return fail_start(
                pid,
                &config,
                None,
                format!("could not open the session port: {e}"),
            )
        }
    };
    let token = home::token();
    let proof = home::token();
    let mut info = SessionInfo {
        pid,
        status: Status::Starting,
        error: None,
        port: listener.local_addr().unwrap().port(),
        token: token.clone(),
        mode: config.mode,
        app_pid: None,
        app_exe: config.exe.clone(),
        app_started: None,
        cdp_port: 0,
        proof: proof.clone(),
    };
    let _ = home::write_session(&info);
    if config.mode != Mode::Launch {
        return fail_start(pid, &config, None, "attach is not built on macOS".into());
    }

    let instance_dir = home::home()
        .join("instances")
        .join(format!("{}-{pid}", home::stamp()));
    let plugin_token = home::token();
    let fail = |app: Option<u32>, error: String| {
        fail_start(pid, &config, app, error);
        let _ = std::fs::remove_dir_all(&instance_dir);
    };
    let mut child = match spawn_app(&config, &instance_dir, &plugin_token) {
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
    let main = match wait_ready(&plugin, config.main_url.as_deref(), deadline).await {
        Ok(m) => m,
        Err(e) => return fail(info.app_pid, e),
    };

    let session = Arc::new(PluginSession {
        config: config.clone(),
        plugin,
        main,
        app_pid: info.app_pid,
        instance_dir,
        last_activity: Mutex::new(Instant::now()),
    });
    info.status = Status::Ready;
    let _ = home::write_session(&info);
    tokio::spawn(watchdog(session.clone(), child));

    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let st = session.clone();
        let (token, proof) = (token.clone(), proof.clone());
        tokio::spawn(async move {
            let (read, mut write) = stream.into_split();
            let mut reader = BufReader::new(read.take(MAX_REQUEST));
            let mut line = String::new();
            if reader.read_line(&mut line).await.is_err() {
                return;
            }
            let Ok(mut req) = serde_json::from_str::<Value>(&line) else {
                return;
            };
            if req["command"] == "hello" {
                let answer = json!({ "proof": proof });
                if write
                    .write_all(format!("{answer}\n").as_bytes())
                    .await
                    .is_err()
                {
                    return;
                }
                line.clear();
                if reader.read_line(&mut line).await.is_err() {
                    return;
                }
                let Ok(next) = serde_json::from_str::<Value>(&line) else {
                    return;
                };
                req = next;
            }
            if req["token"].as_str() != Some(token.as_str()) {
                let _ = write
                    .write_all(b"{\"exit\":1,\"output\":{\"ok\":false,\"error\":{\"kind\":\"error\",\"message\":\"bad token\"}}}\n")
                    .await;
                return;
            }
            *st.last_activity.lock().unwrap() = Instant::now();
            let (name, outcome, closing) =
                match serde_json::from_value::<Request>(req["request"].clone()) {
                    Ok(request) => {
                        let name = request.kind().name().to_string();
                        let closing = matches!(request, Request::Close);
                        (name, st.handle(request).await, closing)
                    }
                    Err(e) => {
                        let name = req["request"]["command"].as_str().unwrap_or("").to_string();
                        let failure =
                            Failure::new(Kind::Error, format!("unknown command `{name}`: {e}"));
                        (name, Err(failure), false)
                    }
                };
            let (code, out) = output::render(&name, &outcome);
            let resp = json!({ "exit": code, "output": out });
            let _ = write.write_all(format!("{resp}\n").as_bytes()).await;
            let _ = write.flush().await;
            if closing {
                st.shutdown(true);
            }
        });
    }
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

/// Ends the session when it has been idle too long or the launched app has exited.
async fn watchdog(session: Arc<PluginSession>, mut child: tokio::process::Child) {
    let idle = Duration::from_secs(session.config.idle_timeout_secs);
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if matches!(child.try_wait(), Ok(Some(_))) {
            session.shutdown(false);
        }
        if session.last_activity.lock().unwrap().elapsed() > idle {
            session.shutdown(true);
        }
    }
}

impl PluginSession {
    async fn handle(&self, request: Request) -> Outcome {
        match request {
            Request::Ping => Ok(Fields::new()),
            Request::Status => Ok(self.status()),
            Request::Close => Ok(fields! { "closed" => true }),
            Request::Eval {
                expression,
                timeout_ms,
            } => {
                let value = self
                    .plugin
                    .eval(&self.main, &expression, Duration::from_millis(timeout_ms))
                    .await?;
                Ok(fields! { "value" => value })
            }
            other => Err(crate::support::refused(other.kind().name())),
        }
    }

    fn status(&self) -> Fields {
        fields! {
            "mode" => self.config.mode.name(),
            "app_pid" => self.app_pid,
            "plugin_port" => self.plugin.port,
            "main" => json!({ "webview": self.main }),
        }
    }

    /// Ends the launched app unless it has already exited, removes what the session wrote, and exits.
    fn shutdown(&self, end_app: bool) -> ! {
        home::remove_session_if(std::process::id());
        if let (true, Some(pid)) = (end_app, self.app_pid) {
            let _ = crate::proc::kill_tree(pid);
        }
        let _ = std::fs::remove_dir_all(&self.instance_dir);
        std::process::exit(0);
    }
}

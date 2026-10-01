//! The session (ADR-0001): a background process that holds the connection to one app instance
//! and answers every other command.

use crate::cdp::{Cdp, Event};
use crate::chord::{self, KeyPress};
use crate::fields;
use crate::home::{self, SessionInfo};
use crate::output::{self, Failure, Fields, Kind, Outcome};
use crate::redact::redact;
use crate::snapshot;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// How the session was asked to start; passed from the CLI to the session process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// `launch` or `attach`.
    pub mode: String,
    pub exe: Option<String>,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// The debugging port; 0 lets the browser pick one and report it in `DevToolsActivePort`.
    pub cdp_port: u16,
    pub data_dir: Option<String>,
    /// A substring of the main page's URL; the first page when absent.
    pub main_url: Option<String>,
    pub idle_timeout_secs: u64,
    pub ready_timeout_secs: u64,
}

/// The largest request line the session reads.
const MAX_REQUEST: u64 = 4 << 20;
/// The most log entries kept; older ones are dropped first.
const MAX_LOGS: usize = 10_000;
/// The object group every remote object pokit resolves belongs to, released after each command.
const OBJECT_GROUP: &str = "pokit";

struct Target {
    cdp: Arc<Cdp>,
    url: String,
    title: String,
}

struct LogEntry {
    seq: u64,
    t_ms: u128,
    source: &'static str,
    level: String,
    target: String,
    text: String,
}

struct Refs {
    /// Ref → (target id, backend DOM node id), for the most recent snapshot only.
    map: HashMap<String, (String, i64)>,
    next: u64,
}

struct State {
    config: Config,
    /// The job holding the launched app and everything it starts; ends them all at once.
    job: Option<crate::proc::Job>,
    started: Instant,
    app_pid: Option<u32>,
    data_dir: Option<PathBuf>,
    /// The profile directory pokit created for this instance, removed when the session ends.
    owned_data_dir: Option<PathBuf>,
    record_dir: PathBuf,
    targets: Mutex<HashMap<String, Target>>,
    order: Mutex<Vec<String>>,
    sync_lock: tokio::sync::Mutex<()>,
    /// Targets whose current document has been seen loaded, visible and painting.
    ready: Mutex<HashSet<String>>,
    main: Mutex<String>,
    current: Mutex<String>,
    refs: Mutex<Refs>,
    logs: Mutex<Vec<LogEntry>>,
    next_log: Mutex<u64>,
    secrets: Mutex<Vec<String>>,
    record_seq: Mutex<u64>,
    last_activity: Mutex<Instant>,
    events_tx: mpsc::UnboundedSender<Event>,
    /// `performance.timeOrigin` of the main page when it became ready.
    main_origin: Mutex<Value>,
}

/// Runs the session process until `close`, idle timeout, or the launched app exits.
pub fn run(config: Config) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(serve(config));
}

async fn serve(mut config: Config) {
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
    let port = listener.local_addr().unwrap().port();
    let token = home::token();
    let proof = home::token();
    let mut info = SessionInfo {
        pid,
        status: "starting".into(),
        error: None,
        port,
        token: token.clone(),
        mode: config.mode.clone(),
        app_pid: None,
        app_exe: config.exe.clone(),
        app_started: None,
        cdp_port: config.cdp_port,
        proof: proof.clone(),
    };
    let _ = home::write_session(&info);

    let record_dir = home::home()
        .join("runs")
        .join(format!("{}-{pid}", home::stamp()));
    let _ = std::fs::create_dir_all(&record_dir);
    let (events_tx, events_rx) = mpsc::unbounded_channel();

    let mut app: Option<tokio::process::Child> = None;
    let mut data_dir = None;
    let mut job = None;
    if config.mode == "launch" {
        match spawn_app(&config) {
            Ok((child, dir)) => {
                info.app_pid = child.id();
                info.app_started = info.app_pid.and_then(crate::proc::process_start_time);
                job = crate::proc::Job::kill_on_close()
                    .filter(|j| info.app_pid.map(|pid| j.assign(pid)).unwrap_or(false));
                app = Some(child);
                data_dir = Some(dir);
            }
            Err(e) => return fail_start(pid, &config, None, e),
        }
        if config.cdp_port == 0 {
            let dir = data_dir.clone().unwrap();
            match active_port(&dir, app.as_mut().unwrap(), config.ready_timeout_secs).await {
                Ok(p) => config.cdp_port = p,
                Err(e) => return fail_start(pid, &config, info.app_pid, e),
            }
        }
        info.cdp_port = config.cdp_port;
    }

    let state = Arc::new(State {
        owned_data_dir: if config.data_dir.is_none() {
            data_dir.clone()
        } else {
            None
        },
        config: config.clone(),
        job,
        started: Instant::now(),
        app_pid: info.app_pid,
        data_dir,
        record_dir,
        targets: Mutex::default(),
        order: Mutex::default(),
        sync_lock: tokio::sync::Mutex::new(()),
        ready: Mutex::default(),
        main: Mutex::default(),
        current: Mutex::default(),
        refs: Mutex::new(Refs {
            map: HashMap::new(),
            next: 1,
        }),
        logs: Mutex::default(),
        next_log: Mutex::new(1),
        secrets: Mutex::default(),
        record_seq: Mutex::new(0),
        last_activity: Mutex::new(Instant::now()),
        events_tx,
        main_origin: Mutex::new(Value::Null),
    });

    if let Some(child) = app.as_mut() {
        type Reader = Box<dyn tokio::io::AsyncRead + Unpin + Send>;
        let outputs: [(&'static str, Option<Reader>); 2] = [
            (
                "backend-stdout",
                child.stdout.take().map(|s| Box::new(s) as Reader),
            ),
            (
                "backend-stderr",
                child.stderr.take().map(|s| Box::new(s) as Reader),
            ),
        ];
        for (source, reader) in outputs {
            if let Some(reader) = reader {
                let st = state.clone();
                tokio::spawn(async move {
                    let mut lines = BufReader::new(reader).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        st.log(source, "info", "", line);
                    }
                });
            }
        }
    }
    tokio::spawn(collect_events(state.clone(), events_rx));

    if let Err(e) = wait_ready(&state, app.as_mut()).await {
        state.remove_owned_data_dir_after(info.app_pid);
        return fail_start(pid, &config, None, e);
    }

    info.status = "ready".into();
    let _ = home::write_session(&info);
    let env_keys: Vec<&str> = config.env.iter().map(|(k, _)| k.as_str()).collect();
    let started = Value::Object(state.status());
    state.record(
        &config.mode,
        &json!({ "exe": config.exe, "args": config.args, "env_keys": env_keys, "main_url": config.main_url }),
        0,
        &started,
    );

    let app = Arc::new(tokio::sync::Mutex::new(app));
    tokio::spawn(discover(state.clone()));
    tokio::spawn(watchdog(state.clone(), app.clone()));

    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let st = state.clone();
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
            let command = req["command"].as_str().unwrap_or("").to_string();
            *st.last_activity.lock().unwrap() = Instant::now();
            let (code, out) = st.handle(&command, req["args"].clone()).await;
            let resp = json!({ "exit": code, "output": out });
            let _ = write.write_all(format!("{resp}\n").as_bytes()).await;
            let _ = write.flush().await;
            if command == "close" {
                st.shutdown();
            }
        });
    }
}

fn fail_start(pid: u32, config: &Config, app_pid: Option<u32>, error: String) {
    if let Some(app) = app_pid {
        let _ = crate::proc::kill_tree(app);
    }
    let info = SessionInfo {
        pid,
        status: "failed".into(),
        error: Some(error),
        port: 0,
        token: String::new(),
        mode: config.mode.clone(),
        app_pid: None,
        app_exe: config.exe.clone(),
        app_started: None,
        cdp_port: config.cdp_port,
        proof: String::new(),
    };
    let _ = home::write_session(&info);
}

/// Starts the app with its own WebView2 profile and the debugging port open.
fn spawn_app(config: &Config) -> Result<(tokio::process::Child, PathBuf), String> {
    let exe = config
        .exe
        .as_ref()
        .ok_or("launch needs the app's executable")?;
    let dir = match &config.data_dir {
        Some(d) => PathBuf::from(d),
        None => {
            home::home()
                .join("instances")
                .join(format!("{}-{}", home::stamp(), std::process::id()))
        }
    };
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create the data directory {}: {e}", dir.display()))?;
    let _ = std::fs::remove_file(active_port_file(&dir));
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(&config.args)
        .env("WEBVIEW2_USER_DATA_FOLDER", &dir)
        .env(
            "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
            format!("--remote-debugging-port={}", config.cdp_port),
        )
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(false);
    for (k, v) in &config.env {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    cmd.creation_flags(crate::proc::CREATE_NO_WINDOW);
    let child = cmd
        .spawn()
        .map_err(|e| format!("could not start {exe}: {e}"))?;
    Ok((child, dir))
}

/// Where the browser reports the debugging port it picked, under a WebView2 profile directory.
fn active_port_file(data_dir: &Path) -> PathBuf {
    data_dir.join("EBWebView").join("DevToolsActivePort")
}

/// Waits for the browser to report the debugging port it picked.
async fn active_port(
    data_dir: &Path,
    app: &mut tokio::process::Child,
    timeout_secs: u64,
) -> Result<u16, String> {
    let file = active_port_file(data_dir);
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        if let Ok(text) = std::fs::read_to_string(&file) {
            if let Some(port) = text
                .lines()
                .next()
                .and_then(|l| l.trim().parse::<u16>().ok())
            {
                return Ok(port);
            }
        }
        if let Ok(Some(status)) = app.try_wait() {
            return Err(format!("the app exited during launch ({status})"));
        }
        if Instant::now() > deadline {
            return Err(format!(
                "no debugging port reported in {} within {timeout_secs}s",
                file.display()
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Waits for the main page to be listed, connected, loaded, visible and painting.
async fn wait_ready(
    state: &Arc<State>,
    mut app: Option<&mut tokio::process::Child>,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(state.config.ready_timeout_secs);
    let port = state.config.cdp_port;
    let mut last_err = String::new();
    loop {
        if let Some(child) = app.as_deref_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!(
                    "the app exited during launch ({status}); {}",
                    state.backend_tail()
                ));
            }
        }
        match crate::devtools::pages(port).await {
            Ok(pages) => {
                let main = pages.iter().find(|p| match &state.config.main_url {
                    Some(m) => p["url"].as_str().unwrap_or("").contains(m.as_str()),
                    None => true,
                });
                if let Some(main) = main {
                    let id = main["id"].as_str().unwrap_or("").to_string();
                    state.sync_targets(&pages).await;
                    if let Ok(cdp) = state.cdp(&id) {
                        *state.main.lock().unwrap() = id.clone();
                        *state.current.lock().unwrap() = id.clone();
                        let wanted = state.config.main_url.clone().unwrap_or_default();
                        let left = deadline.saturating_duration_since(Instant::now());
                        return match probe_ready(&cdp, &wanted, left).await {
                            Ok(v) => {
                                *state.main_origin.lock().unwrap() = v["origin"].clone();
                                state.ready.lock().unwrap().insert(id);
                                Ok(())
                            }
                            Err(last) => Err(format!("the main page did not become ready: {last}")),
                        };
                    }
                } else {
                    last_err = format!("no page target matches {:?}", state.config.main_url);
                }
            }
            Err(e) => last_err = e,
        }
        if Instant::now() > deadline {
            return Err(format!(
                "no page on debugging port {port} within {}s: {last_err}",
                state.config.ready_timeout_secs
            ));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Resolves to the page's URL, time origin, load and visibility state, and whether it is loaded,
/// visible and has painted two frames.
const READY_PROBE: &str = "new Promise((resolve) => {
    const base = { url: location.href, origin: performance.timeOrigin,
                   state: document.readyState, visibility: document.visibilityState };
    if (document.readyState !== 'complete' || document.visibilityState !== 'visible') {
        return resolve({ ...base, painting: false });
    }
    const timer = setTimeout(() => resolve({ ...base, painting: false }), 1000);
    requestAnimationFrame(() => requestAnimationFrame(() => { clearTimeout(timer); resolve({ ...base, painting: true }); }));
})";

/// Polls `READY_PROBE` until the real document (not the initial about:blank, and containing
/// `wanted` in its URL) is loaded, visible and painting; the last probe result on timeout.
async fn probe_ready(cdp: &Cdp, wanted: &str, timeout: Duration) -> Result<Value, Value> {
    let deadline = Instant::now() + timeout;
    let mut last = Value::Null;
    loop {
        if let Ok(v) = evaluate(cdp, READY_PROBE).await {
            let url = v["url"].as_str().unwrap_or("");
            if v["painting"] == true && url != "about:blank" && url.contains(wanted) {
                return Ok(v);
            }
            last = v;
        }
        if Instant::now() > deadline {
            return Err(last);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Keeps a connection to every page target as windows open and close.
async fn discover(state: Arc<State>) {
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Ok(pages) = crate::devtools::pages(state.config.cdp_port).await {
            state.sync_targets(&pages).await;
        }
    }
}

/// Ends the session when it has been idle too long or the launched app has exited.
async fn watchdog(state: Arc<State>, app: Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>) {
    let idle = Duration::from_secs(state.config.idle_timeout_secs);
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if state.last_activity.lock().unwrap().elapsed() > idle {
            state.log(
                "session",
                "info",
                "",
                format!("idle for {}s; ending the session", idle.as_secs()),
            );
            state.shutdown();
        }
        if let Some(child) = app.lock().await.as_mut() {
            if let Ok(Some(_)) = child.try_wait() {
                state.log(
                    "session",
                    "info",
                    "",
                    "the launched app exited; ending the session".into(),
                );
                state.shutdown();
            }
        }
    }
}

async fn collect_events(state: Arc<State>, mut rx: mpsc::UnboundedReceiver<Event>) {
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
    fn log(&self, source: &'static str, level: &str, target: &str, text: String) {
        let seq = {
            let mut next = self.next_log.lock().unwrap();
            let seq = *next;
            *next += 1;
            seq
        };
        let mut logs = self.logs.lock().unwrap();
        logs.push(LogEntry {
            seq,
            t_ms: self.started.elapsed().as_millis(),
            source,
            level: level.to_string(),
            target: target.to_string(),
            text,
        });
        if logs.len() > MAX_LOGS {
            let excess = logs.len() - MAX_LOGS;
            logs.drain(..excess);
        }
    }

    fn backend_tail(&self) -> String {
        let logs = self.logs.lock().unwrap();
        let tail: Vec<&str> = logs
            .iter()
            .rev()
            .filter(|l| l.source.starts_with("backend"))
            .take(5)
            .map(|l| l.text.as_str())
            .collect();
        if tail.is_empty() {
            "no backend output".into()
        } else {
            format!(
                "last backend output: {}",
                tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
            )
        }
    }

    /// Connects to page targets not yet held and drops ones that are gone. Only page sockets on
    /// this instance's own debugging port are followed.
    async fn sync_targets(&self, pages: &[Value]) {
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

    fn cdp(&self, id: &str) -> Result<Arc<Cdp>, Failure> {
        self.targets
            .lock()
            .unwrap()
            .get(id)
            .map(|t| t.cdp.clone())
            .ok_or_else(|| Failure::new(Kind::NotFound, format!("target {id} is no longer open")))
    }

    fn current(&self) -> Result<(String, Arc<Cdp>), Failure> {
        let id = self.current.lock().unwrap().clone();
        let cdp = self.cdp(&id)?;
        Ok((id, cdp))
    }

    /// Waits, before input, until the target's current document is loaded, visible and painting.
    async fn ensure_ready(&self, id: &str, cdp: &Cdp) -> Result<(), Failure> {
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

    /// Removes the profile directory pokit created, once `app`'s processes have let go of it.
    fn remove_owned_data_dir_after(&self, app: Option<u32>) {
        let Some(dir) = &self.owned_data_dir else {
            return;
        };
        if let Some(pid) = app {
            let _ = crate::proc::kill_tree(pid);
        }
        for _ in 0..50 {
            if std::fs::remove_dir_all(dir).is_ok() || !dir.exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn shutdown(&self) -> ! {
        home::remove_session_if(std::process::id());
        let mut ended = json!({ "mode": self.config.mode });
        if self.config.mode == "launch" {
            if let Some(pid) = self.app_pid {
                ended["kill"] = match &self.job {
                    Some(job) if job.terminate() => json!("job terminated"),
                    _ => json!(crate::proc::kill_tree(pid)),
                };
            }
            self.remove_owned_data_dir_after(None);
            ended["profile_left"] = json!(self.owned_data_dir.as_ref().map(|d| d.exists()));
        }
        self.record("shutdown", &Value::Null, 0, &ended);
        std::process::exit(0);
    }

    async fn handle(self: &Arc<Self>, command: &str, args: Value) -> (i32, Value) {
        let secret_type = command == "type" && args["secret"] == true;
        if secret_type {
            if let Some(text) = args["text"].as_str() {
                let mut secrets = self.secrets.lock().unwrap();
                if !secrets.iter().any(|s| s == text) {
                    secrets.push(text.to_string());
                }
            }
        }
        let outcome = match command {
            "ping" => Ok(Fields::new()),
            "status" => Ok(self.status()),
            "close" => Ok(fields! { "closed" => self.config.mode == "launch" }),
            "targets" => self.targets_cmd(&args).await,
            "eval" => self.eval_cmd(&args).await,
            "snapshot" => self.snapshot_cmd().await,
            "read" => self.read_cmd(&args).await,
            "click" => self.click_cmd(&args).await,
            "type" => self.type_cmd(&args).await,
            "key" => self.key_cmd(&args).await,
            "wait" => self.wait_cmd(&args).await,
            "capture" => self.capture_cmd(&args).await,
            "logs" => self.logs_cmd(&args),
            "doctor" => self.doctor_cmd().await,
            other => Err(Failure::new(
                Kind::Error,
                format!("unknown command `{other}`"),
            )),
        };
        if matches!(command, "read" | "click" | "type" | "key" | "capture") {
            self.release_objects().await;
        }
        let screenshot = match (&outcome, command) {
            (Err(_), "type") if secret_type => None,
            (Err(_), "read" | "click" | "type" | "key" | "wait" | "eval" | "snapshot") => {
                self.failure_screenshot().await
            }
            _ => None,
        };
        let outcome = match (outcome, screenshot) {
            (Err(f), Some(path)) => Err(f.with("screenshot", path)),
            (o, _) => o,
        };
        let (code, mut out) = output::render(command, &outcome);
        let secrets = self.secrets.lock().unwrap().clone();
        redact(&mut out, &secrets);
        if !matches!(command, "ping" | "status") {
            let mut args = args;
            if args["secret"] == true {
                args["text"] = json!(crate::redact::MASK);
            }
            redact(&mut args, &secrets);
            let mut recorded = out.clone();
            if let Some(entries) = recorded
                .get("entries")
                .and_then(Value::as_array)
                .map(Vec::len)
            {
                recorded["entries"] = json!(format!("{entries} entries, not copied"));
            }
            self.record(command, &args, code, &recorded);
        }
        (code, out)
    }

    /// Releases every remote object pokit resolved during a command, on every page.
    async fn release_objects(&self) {
        let cdps: Vec<Arc<Cdp>> = self
            .targets
            .lock()
            .unwrap()
            .values()
            .map(|t| t.cdp.clone())
            .collect();
        for cdp in cdps {
            let _ = cdp
                .call(
                    "Runtime.releaseObjectGroup",
                    json!({ "objectGroup": OBJECT_GROUP }),
                )
                .await;
        }
    }

    fn record(&self, command: &str, args: &Value, code: i32, out: &Value) {
        let mut seq = self.record_seq.lock().unwrap();
        *seq += 1;
        let line = json!({
            "seq": *seq,
            "t_ms": self.started.elapsed().as_millis() as u64,
            "command": command,
            "args": args,
            "exit": code,
            "output": out,
        });
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.record_dir.join("record.jsonl"))
        {
            let _ = writeln!(f, "{line}");
        }
    }

    async fn failure_screenshot(&self) -> Option<String> {
        let (_, cdp) = self.current().ok()?;
        let seq = *self.record_seq.lock().unwrap() + 1;
        let path = self.record_dir.join(format!("fail-{seq}.png"));
        screenshot_to(&cdp, json!({ "format": "png" }), &path)
            .await
            .ok()?;
        Some(path.display().to_string())
    }

    fn status(&self) -> Fields {
        let main = self.main.lock().unwrap().clone();
        let (title, url) = self
            .targets
            .lock()
            .unwrap()
            .get(&main)
            .map(|t| (t.title.clone(), t.url.clone()))
            .unwrap_or_default();
        fields! {
            "mode" => self.config.mode,
            "session_pid" => std::process::id(),
            "app_pid" => self.app_pid,
            "cdp_port" => self.config.cdp_port,
            "data_dir" => self.data_dir.as_ref().map(|d| d.display().to_string()),
            "record_dir" => self.record_dir.display().to_string(),
            "idle_timeout_secs" => self.config.idle_timeout_secs,
            "main" => json!({ "id": main, "title": title, "url": url, "time_origin": *self.main_origin.lock().unwrap() }),
        }
    }

    async fn targets_cmd(&self, args: &Value) -> Outcome {
        if let Ok(pages) = crate::devtools::pages(self.config.cdp_port).await {
            self.sync_targets(&pages).await;
        }
        if let Some(sel) = args["select"].as_str() {
            let order = self.order.lock().unwrap().clone();
            let found = {
                let targets = self.targets.lock().unwrap();
                let by_id = order.iter().find(|id| id.as_str() == sel);
                let by_index = sel.parse::<usize>().ok().and_then(|i| order.get(i));
                let by_text = order.iter().find(|id| {
                    targets
                        .get(*id)
                        .map(|t| t.url.contains(sel) || t.title.contains(sel))
                        .unwrap_or(false)
                });
                by_id.or(by_index).or(by_text).cloned()
            };
            match found {
                Some(id) => *self.current.lock().unwrap() = id,
                None => {
                    return Err(Failure::new(
                        Kind::NotFound,
                        format!("no target matches `{sel}`"),
                    ))
                }
            }
        }
        let (main, current) = (
            self.main.lock().unwrap().clone(),
            self.current.lock().unwrap().clone(),
        );
        let order = self.order.lock().unwrap().clone();
        let targets = self.targets.lock().unwrap();
        let list: Vec<Value> = order
            .iter()
            .enumerate()
            .filter_map(|(i, id)| {
                targets.get(id).map(|t| {
                    json!({ "index": i, "id": id, "title": t.title, "url": t.url, "main": *id == main, "current": *id == current })
                })
            })
            .collect();
        Ok(fields! { "targets" => list })
    }

    async fn eval_cmd(&self, args: &Value) -> Outcome {
        let (_, cdp) = self.current()?;
        let expr = args["expression"].as_str().unwrap_or("");
        let timeout = Duration::from_millis(args["timeout_ms"].as_u64().unwrap_or(15_000));
        let value = evaluate_within(&cdp, expr, timeout).await?;
        Ok(fields! { "value" => value })
    }

    async fn snapshot_cmd(&self) -> Outcome {
        let (target, cdp) = self.current()?;
        cdp.call("Accessibility.enable", json!({})).await?;
        let tree = cdp.call("Accessibility.getFullAXTree", json!({})).await;
        let _ = cdp.call("Accessibility.disable", json!({})).await;
        let tree = tree?;
        let mut refs = self.refs.lock().unwrap();
        let snap = snapshot::render(
            tree["nodes"].as_array().map(Vec::as_slice).unwrap_or(&[]),
            refs.next,
        );
        refs.next = snap.next_ref;
        refs.map = snap
            .refs
            .iter()
            .map(|(r, node)| (r.clone(), (target.clone(), *node)))
            .collect();
        Ok(fields! { "snapshot" => snap.text, "refs" => snap.refs.len() })
    }

    /// Resolves a ref (`e12`) or CSS selector to a remote object: (target id, its page, object id).
    async fn resolve(&self, target: &str) -> Result<(String, Arc<Cdp>, String), Failure> {
        if snapshot::is_ref(target) {
            let entry = self.refs.lock().unwrap().map.get(target).cloned();
            let Some((tid, node)) = entry else {
                return Err(Failure::new(
                    Kind::StaleRef,
                    format!("ref {target} is not in the latest snapshot; take a new snapshot"),
                )
                .with("ref", target));
            };
            let cdp = self.cdp(&tid)?;
            let obj = cdp
                .call(
                    "DOM.resolveNode",
                    json!({ "backendNodeId": node, "objectGroup": OBJECT_GROUP }),
                )
                .await
                .map_err(|_| {
                    Failure::new(
                        Kind::StaleRef,
                        format!("the element for ref {target} is no longer in the page"),
                    )
                    .with("ref", target)
                })?;
            let id = obj["object"]["objectId"].as_str().unwrap_or("").to_string();
            return Ok((tid, cdp, id));
        }
        let (tid, cdp) = self.current()?;
        let expr = format!("document.querySelector({})", json!(target));
        let r = cdp
            .call(
                "Runtime.evaluate",
                json!({ "expression": expr, "objectGroup": OBJECT_GROUP }),
            )
            .await?;
        if let Some(e) = r.get("exceptionDetails") {
            return Err(Failure::new(
                Kind::Error,
                format!("invalid selector `{target}`: {}", exception_text(e)),
            ));
        }
        match r["result"]["objectId"].as_str() {
            Some(id) => Ok((tid, cdp, id.to_string())),
            None => Err(
                Failure::new(Kind::NotFound, format!("no element matches `{target}`"))
                    .with("selector", target),
            ),
        }
    }

    async fn read_cmd(&self, args: &Value) -> Outcome {
        let target = args["target"].as_str().unwrap_or("");
        let (_, cdp, obj) = self.resolve(target).await?;
        let v = call_on(
            &cdp,
            &obj,
            "function() {
                const r = this.getBoundingClientRect();
                return {
                    tag: this.tagName.toLowerCase(),
                    text: (this.innerText ?? this.textContent ?? '').trim(),
                    value: 'value' in this ? String(this.value) : null,
                    state: {
                        focused: document.activeElement === this,
                        disabled: !!this.disabled,
                        checked: 'checked' in this ? !!this.checked : null,
                        visible: r.width > 0 && r.height > 0,
                    },
                };
            }",
            &[],
        )
        .await?;
        let mut f = Fields::new();
        if let Value::Object(m) = v {
            f.extend(m);
        }
        Ok(f)
    }

    /// Refuses input unless the focused element lies inside the required focus selector.
    async fn guard_focus(&self, cdp: &Cdp, args: &Value) -> Result<(), Failure> {
        let Some(sel) = args["require_focus"].as_str() else {
            return Ok(());
        };
        let expr = format!(
            "(() => {{
                const r = document.querySelector({sel});
                const a = document.activeElement;
                const d = a ? a.tagName.toLowerCase() + (a.id ? '#' + a.id : '') : null;
                return {{ matched: !!r, holds: !!(r && a && (r === a || r.contains(a))), active: d }};
            }})()",
            sel = json!(sel)
        );
        let v = evaluate(cdp, &expr).await?;
        if v["holds"] == true {
            return Ok(());
        }
        let message = if v["matched"] == true {
            format!("focus is outside `{sel}`; nothing was sent")
        } else {
            format!("no element matches the required focus `{sel}`; nothing was sent")
        };
        Err(Failure::new(Kind::GuardRefused, message)
            .with("expected", sel)
            .with("found", v["active"].clone()))
    }

    /// Refuses input into `obj` unless it lies inside the required focus selector; nothing on the page changes.
    async fn guard_target(
        &self,
        cdp: &Cdp,
        obj: &str,
        into: &str,
        args: &Value,
    ) -> Result<(), Failure> {
        let Some(sel) = args["require_focus"].as_str() else {
            return Ok(());
        };
        let v = call_on(
            cdp,
            obj,
            "function(sel) {
                const r = document.querySelector(sel);
                return { matched: !!r, inside: !!(r && (r === this || r.contains(this))) };
            }",
            &[json!(sel)],
        )
        .await?;
        if v["inside"] == true {
            return Ok(());
        }
        let message = if v["matched"] == true {
            format!("`{into}` is outside `{sel}`; nothing was sent")
        } else {
            format!("no element matches the required focus `{sel}`; nothing was sent")
        };
        Err(Failure::new(Kind::GuardRefused, message)
            .with("expected", sel)
            .with("found", into))
    }

    /// Resolves `into` when given, checks the focus guard before anything changes, then focuses it;
    /// without `into`, checks that the current focus holds. Returns the page to send input to.
    async fn focus_for_input(&self, args: &Value) -> Result<Arc<Cdp>, Failure> {
        match args["into"].as_str() {
            Some(into) => {
                let (tid, cdp, obj) = self.resolve(into).await?;
                self.guard_target(&cdp, &obj, into, args).await?;
                self.ensure_ready(&tid, &cdp).await?;
                call_on(&cdp, &obj, "function() { this.focus(); return true; }", &[]).await?;
                self.guard_focus(&cdp, args).await?;
                Ok(cdp)
            }
            None => {
                let (tid, cdp) = self.current()?;
                self.guard_focus(&cdp, args).await?;
                self.ensure_ready(&tid, &cdp).await?;
                Ok(cdp)
            }
        }
    }

    async fn click_cmd(&self, args: &Value) -> Outcome {
        let (cdp, x, y) = match args["target"].as_str() {
            Some(t) => {
                let (tid, cdp, obj) = self.resolve(t).await?;
                self.guard_focus(&cdp, args).await?;
                self.ensure_ready(&tid, &cdp).await?;
                let r = scroll_and_measure(&cdp, &obj).await?;
                if r["width"].as_f64() == Some(0.0) && r["height"].as_f64() == Some(0.0) {
                    return Err(Failure::new(
                        Kind::NotFound,
                        format!("`{t}` has no size on screen"),
                    ));
                }
                let x =
                    r["left"].as_f64().unwrap_or(0.0) + r["width"].as_f64().unwrap_or(0.0) / 2.0;
                let y =
                    r["top"].as_f64().unwrap_or(0.0) + r["height"].as_f64().unwrap_or(0.0) / 2.0;
                (cdp, x, y)
            }
            None => {
                let (tid, cdp) = self.current()?;
                let (Some(x), Some(y)) = (args["x"].as_f64(), args["y"].as_f64()) else {
                    return Err(Failure::new(
                        Kind::Error,
                        "click needs a ref, a selector, or --x and --y",
                    ));
                };
                self.guard_focus(&cdp, args).await?;
                self.ensure_ready(&tid, &cdp).await?;
                (cdp, x, y)
            }
        };
        let mouse = |kind: &str, button: &str, count: i64| json!({ "type": kind, "x": x, "y": y, "button": button, "clickCount": count });
        cdp.call("Input.dispatchMouseEvent", mouse("mouseMoved", "none", 0))
            .await?;
        let action = if args["hover"] == true {
            "hover"
        } else {
            let button = if args["right"] == true {
                "right"
            } else {
                "left"
            };
            let count = if args["double"] == true { 2 } else { 1 };
            for n in 1..=count {
                cdp.call("Input.dispatchMouseEvent", mouse("mousePressed", button, n))
                    .await?;
                cdp.call(
                    "Input.dispatchMouseEvent",
                    mouse("mouseReleased", button, n),
                )
                .await?;
            }
            match (button, count) {
                ("right", _) => "right_click",
                (_, 2) => "double_click",
                _ => "click",
            }
        };
        Ok(fields! { "action" => action, "x" => x, "y" => y })
    }

    async fn type_cmd(&self, args: &Value) -> Outcome {
        let text = args["text"]
            .as_str()
            .unwrap_or("")
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        let cdp = self.focus_for_input(args).await?;
        let mut keyed = 0;
        let mut inserted = 0;
        for c in text.chars() {
            let press = if c == '\n' {
                chord::parse_chord("Enter").ok()
            } else {
                chord::key_for_char(c)
            };
            match press {
                Some(p) => {
                    send_key(&cdp, &p).await?;
                    keyed += 1;
                }
                None => {
                    cdp.call("Input.insertText", json!({ "text": c.to_string() }))
                        .await?;
                    inserted += 1;
                }
            }
        }
        Ok(
            fields! { "typed_chars" => keyed + inserted, "key_events" => keyed, "inserted_chars" => inserted },
        )
    }

    async fn key_cmd(&self, args: &Value) -> Outcome {
        let chord = args["chord"].as_str().unwrap_or("");
        let press = chord::parse_chord(chord).map_err(|e| Failure::new(Kind::Error, e))?;
        let cdp = self.focus_for_input(args).await?;
        send_key(&cdp, &press).await?;
        Ok(fields! { "key" => press.key, "code" => press.code, "modifiers" => press.modifiers })
    }

    async fn wait_cmd(&self, args: &Value) -> Outcome {
        let timeout = Duration::from_millis(args["timeout_ms"].as_u64().unwrap_or(10_000));
        let (expr, expected, absent) = if let Some(s) = args["selector"].as_str() {
            (
                format!("!!document.querySelector({})", json!(s)),
                format!("an element matching `{s}`"),
                "no element matches".to_string(),
            )
        } else if let Some(t) = args["text"].as_str() {
            (
                format!("(document.body?.innerText ?? '').includes({})", json!(t)),
                format!("the text {}", json!(t)),
                "not in the page's text".to_string(),
            )
        } else if let Some(e) = args["expr"].as_str() {
            (
                format!("!!({e})"),
                format!("`{e}` to be true"),
                "false".to_string(),
            )
        } else {
            return Err(Failure::new(
                Kind::Error,
                "wait needs --selector, --text or --expr",
            ));
        };
        let start = Instant::now();
        let mut found = absent;
        loop {
            let (_, cdp) = self.current()?;
            match evaluate(&cdp, &expr).await {
                Ok(Value::Bool(true)) => {
                    return Ok(fields! { "waited_ms" => start.elapsed().as_millis() as u64 });
                }
                Ok(_) => {}
                Err(f) if f.kind == Kind::JsError => return Err(f.with("expected", expected)),
                Err(f) => found = f.message,
            }
            if start.elapsed() >= timeout {
                return Err(Failure::new(
                    Kind::Timeout,
                    format!("waited {}ms for {expected}", timeout.as_millis()),
                )
                .with("expected", expected)
                .with("found", found));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn capture_cmd(&self, args: &Value) -> Outcome {
        let mut params = json!({ "format": "png" });
        let cdp = match args["target"].as_str() {
            Some(t) => {
                let (_, cdp, obj) = self.resolve(t).await?;
                let r = scroll_and_measure(&cdp, &obj).await?;
                let x = r["left"].as_f64().unwrap_or(0.0) + r["scroll_x"].as_f64().unwrap_or(0.0);
                let y = r["top"].as_f64().unwrap_or(0.0) + r["scroll_y"].as_f64().unwrap_or(0.0);
                params["clip"] = json!({ "x": x, "y": y, "width": r["width"], "height": r["height"], "scale": 1 });
                cdp
            }
            None => self.current()?.1,
        };
        let path = match args["out"].as_str() {
            Some(p) => PathBuf::from(p),
            None => self.record_dir.join(format!(
                "capture-{}.png",
                *self.record_seq.lock().unwrap() + 1
            )),
        };
        let bytes = screenshot_to(&cdp, params, &path).await?;
        Ok(fields! { "path" => path.display().to_string(), "bytes" => bytes })
    }

    fn logs_cmd(&self, args: &Value) -> Outcome {
        let since = args["since"].as_u64().unwrap_or(0);
        let logs = self.logs.lock().unwrap();
        let entries: Vec<Value> = logs
            .iter()
            .filter(|l| l.seq > since)
            .map(|l| json!({ "seq": l.seq, "t_ms": l.t_ms as u64, "source": l.source, "level": l.level, "target": l.target, "text": l.text }))
            .collect();
        let next = logs.last().map(|l| l.seq).unwrap_or(since);
        Ok(fields! { "entries" => entries, "next" => next })
    }

    async fn doctor_cmd(&self) -> Outcome {
        let checks = doctor_checks(self.config.cdp_port).await;
        doctor_outcome(checks)
    }
}

/// The checks `doctor` runs against a debugging port.
pub async fn doctor_checks(port: u16) -> Vec<Value> {
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

pub fn doctor_outcome(checks: Vec<Value>) -> Outcome {
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

async fn send_key(cdp: &Cdp, p: &KeyPress) -> Result<(), Failure> {
    let mut down = json!({
        "type": if p.text.is_some() { "keyDown" } else { "rawKeyDown" },
        "key": p.key,
        "code": p.code,
        "windowsVirtualKeyCode": p.vk,
        "modifiers": p.modifiers,
    });
    if let Some(t) = &p.text {
        down["text"] = json!(t);
        down["unmodifiedText"] = json!(t);
    }
    cdp.call("Input.dispatchKeyEvent", down).await?;
    cdp.call(
        "Input.dispatchKeyEvent",
        json!({ "type": "keyUp", "key": p.key, "code": p.code, "windowsVirtualKeyCode": p.vk, "modifiers": p.modifiers }),
    )
    .await?;
    Ok(())
}

/// Scrolls the element into view and returns its viewport rectangle and the page's scroll offset.
async fn scroll_and_measure(cdp: &Cdp, obj: &str) -> Result<Value, Failure> {
    call_on(
        cdp,
        obj,
        "function() {
            this.scrollIntoView({ block: 'center', inline: 'center' });
            const r = this.getBoundingClientRect();
            return { left: r.left, top: r.top, width: r.width, height: r.height,
                     scroll_x: window.scrollX, scroll_y: window.scrollY };
        }",
        &[],
    )
    .await
}

/// Takes a screenshot with `params` and writes it to `path`; returns its size in bytes.
async fn screenshot_to(cdp: &Cdp, params: Value, path: &Path) -> Result<usize, Failure> {
    let shot = cdp.call("Page.captureScreenshot", params).await?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(shot["data"].as_str().unwrap_or(""))
        .map_err(|e| Failure::new(Kind::Error, format!("screenshot data: {e}")))?;
    std::fs::write(path, &bytes).map_err(|e| {
        Failure::new(
            Kind::Error,
            format!("could not write {}: {e}", path.display()),
        )
    })?;
    Ok(bytes.len())
}

fn exception_text(details: &Value) -> String {
    details["exception"]["description"]
        .as_str()
        .or(details["text"].as_str())
        .unwrap_or("exception")
        .to_string()
}

async fn evaluate(cdp: &Cdp, expr: &str) -> Result<Value, Failure> {
    evaluate_within(cdp, expr, Duration::from_secs(15)).await
}

async fn evaluate_within(cdp: &Cdp, expr: &str, timeout: Duration) -> Result<Value, Failure> {
    let params = json!({ "expression": expr, "returnByValue": true, "awaitPromise": true });
    let r = cdp
        .call_timeout("Runtime.evaluate", params, timeout)
        .await?;
    if let Some(e) = r.get("exceptionDetails") {
        return Err(Failure::new(Kind::JsError, exception_text(e)));
    }
    Ok(r["result"]["value"].clone())
}

async fn call_on(
    cdp: &Cdp,
    object_id: &str,
    function: &str,
    arguments: &[Value],
) -> Result<Value, Failure> {
    let arguments: Vec<Value> = arguments.iter().map(|v| json!({ "value": v })).collect();
    let r = cdp
        .call(
            "Runtime.callFunctionOn",
            json!({ "objectId": object_id, "functionDeclaration": function, "arguments": arguments,
                    "returnByValue": true, "awaitPromise": true }),
        )
        .await?;
    if let Some(e) = r.get("exceptionDetails") {
        return Err(Failure::new(Kind::JsError, exception_text(e)));
    }
    Ok(r["result"]["value"].clone())
}

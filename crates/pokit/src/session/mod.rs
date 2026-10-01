//! The session (ADR-0001): a background process that holds the connection to one app instance
//! and answers every other command.

mod commands;
mod launch;
mod logs;
mod pages;
mod record;

use crate::cdp::{Cdp, Event};
use crate::fields;
use crate::home::{self, Mode, SessionInfo, Status};
use crate::output::{self, Failure, Fields, Kind};
use crate::request::Request;
use logs::LogEntry;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// How the session was asked to start; passed from the CLI to the session process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub mode: Mode,
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

struct Target {
    cdp: Arc<Cdp>,
    url: String,
    title: String,
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
        status: Status::Starting,
        error: None,
        port,
        token: token.clone(),
        mode: config.mode,
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
    if config.mode == Mode::Launch {
        match launch::spawn_app(&config) {
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
            match launch::active_port(&dir, app.as_mut().unwrap(), config.ready_timeout_secs).await
            {
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
    tokio::spawn(pages::collect_events(state.clone(), events_rx));

    if let Err(e) = launch::wait_ready(&state, app.as_mut()).await {
        state.try_remove_owned_data_dir_after(info.app_pid);
        return fail_start(pid, &config, None, e);
    }

    info.status = Status::Ready;
    let _ = home::write_session(&info);
    let env_keys: Vec<&str> = config.env.iter().map(|(k, _)| k.as_str()).collect();
    let started = Value::Object(state.status());
    state.record(
        config.mode.name(),
        &json!({ "exe": config.exe, "args": config.args, "env_keys": env_keys, "main_url": config.main_url }),
        0,
        &started,
    );

    let app = Arc::new(tokio::sync::Mutex::new(app));
    tokio::spawn(pages::discover(state.clone()));
    tokio::spawn(launch::sweep_profiles(pid));
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
            *st.last_activity.lock().unwrap() = Instant::now();
            let request = match serde_json::from_value::<Request>(req["request"].clone()) {
                Ok(r) => r,
                Err(e) => {
                    let name = req["request"]["command"].as_str().unwrap_or("").to_string();
                    let failure =
                        Failure::new(Kind::Error, format!("unknown command `{name}`: {e}"));
                    let (code, out) = output::render(&name, &Err(failure));
                    st.record(&name, &req["request"]["args"], code, &out);
                    let resp = json!({ "exit": code, "output": out });
                    let _ = write.write_all(format!("{resp}\n").as_bytes()).await;
                    return;
                }
            };
            let closing = matches!(request, Request::Close);
            let (code, out) = st.handle(request).await;
            let resp = json!({ "exit": code, "output": out });
            let _ = write.write_all(format!("{resp}\n").as_bytes()).await;
            let _ = write.flush().await;
            if closing {
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
        status: Status::Failed,
        error: Some(error),
        port: 0,
        token: String::new(),
        mode: config.mode,
        app_pid: None,
        app_exe: config.exe.clone(),
        app_started: None,
        cdp_port: config.cdp_port,
        proof: String::new(),
    };
    let _ = home::write_session(&info);
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

impl State {
    /// Makes one attempt to remove the profile directory pokit created, after ending `app`;
    /// what is still held is left for the next launch's sweep.
    fn try_remove_owned_data_dir_after(&self, app: Option<u32>) {
        let Some(dir) = &self.owned_data_dir else {
            return;
        };
        if let Some(pid) = app {
            let _ = crate::proc::kill_tree(pid);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    fn shutdown(&self) -> ! {
        home::remove_session_if(std::process::id());
        let mut ended = json!({ "mode": self.config.mode.name() });
        if self.config.mode == Mode::Launch {
            if let Some(pid) = self.app_pid {
                ended["kill"] = match &self.job {
                    Some(job) if job.terminate() => json!("job terminated"),
                    _ => json!(crate::proc::kill_tree(pid)),
                };
            }
            self.try_remove_owned_data_dir_after(None);
            ended["profile_left"] = json!(self.owned_data_dir.as_ref().map(|d| d.exists()));
        }
        self.record("shutdown", &Value::Null, 0, &ended);
        std::process::exit(0);
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
            "mode" => self.config.mode.name(),
            "session_pid" => std::process::id(),
            "app_pid" => self.app_pid,
            "cdp_port" => self.config.cdp_port,
            "data_dir" => self.data_dir.as_ref().map(|d| d.display().to_string()),
            "record_dir" => self.record_dir.display().to_string(),
            "idle_timeout_secs" => self.config.idle_timeout_secs,
            "main" => json!({ "id": main, "title": title, "url": url, "time_origin": *self.main_origin.lock().unwrap() }),
        }
    }
}

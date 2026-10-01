//! pokit: drives a running webview desktop app for an agent, answering every command in JSON.

mod cdp;
mod chord;
mod client;
mod devtools;
mod home;
mod output;
mod proc;
mod redact;
mod session;
mod snapshot;

use clap::{Parser, Subcommand};
use output::{Failure, Kind};
use serde_json::{json, Value};
use std::io::Read;
use std::process::ExitCode;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(
    name = "pokit",
    version,
    about = "Real input into a running webview app, and what the page did"
)]
struct Cli {
    /// Seconds a command may take before it gives up.
    #[arg(long, global = true, default_value_t = 30)]
    timeout: u64,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start a test instance of an app with its own WebView2 profile, and a session on it.
    Launch {
        exe: String,
        /// Arguments passed to the app.
        #[arg(last = true)]
        args: Vec<String>,
        /// Debugging port; the browser picks a free one when omitted.
        #[arg(long)]
        port: Option<u16>,
        /// WebView2 profile directory; a fresh one under the pokit home when omitted.
        #[arg(long)]
        data_dir: Option<String>,
        /// Environment for the app, `KEY=VALUE`, repeatable. Not masked: not for secrets.
        #[arg(long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,
        /// A substring of the main page's URL.
        #[arg(long)]
        main_url: Option<String>,
        #[arg(long, default_value_t = 600)]
        idle_timeout: u64,
        #[arg(long, default_value_t = 30)]
        ready_timeout: u64,
    },
    /// Start a session on an already-running instance with its debugging port open.
    Attach {
        #[arg(long)]
        port: u16,
        #[arg(long)]
        main_url: Option<String>,
        #[arg(long, default_value_t = 600)]
        idle_timeout: u64,
        #[arg(long, default_value_t = 10)]
        ready_timeout: u64,
    },
    /// End the session; closes the instance only if pokit launched it.
    Close,
    /// List the app's windows and pages, or switch to one.
    Targets {
        /// Id, index, or a substring of the URL or title.
        #[arg(long)]
        select: Option<String>,
    },
    /// The page's structure as text, with refs.
    Snapshot,
    /// An element's text, value and state.
    Read { target: String },
    /// Click, double-click, right-click or hover an element or a point.
    Click {
        target: Option<String>,
        #[arg(long)]
        x: Option<f64>,
        #[arg(long)]
        y: Option<f64>,
        #[arg(long)]
        double: bool,
        #[arg(long)]
        right: bool,
        #[arg(long)]
        hover: bool,
        #[arg(long)]
        require_focus: Option<String>,
    },
    /// Type text into the focused element, or into `--into`.
    Type {
        text: Option<String>,
        /// Read the text from stdin and mask it everywhere.
        #[arg(long)]
        secret: bool,
        /// Read the text from this environment variable and mask it everywhere.
        #[arg(long)]
        secret_env: Option<String>,
        #[arg(long)]
        into: Option<String>,
        #[arg(long)]
        require_focus: Option<String>,
    },
    /// Press a chord written with physical key names, e.g. `Ctrl+Equal`, into the focused element or `--into`.
    Key {
        chord: String,
        #[arg(long)]
        into: Option<String>,
        #[arg(long)]
        require_focus: Option<String>,
    },
    /// Block until an element appears, a text is on the page, or an expression is true.
    Wait {
        #[arg(long)]
        selector: Option<String>,
        #[arg(long)]
        text: Option<String>,
        #[arg(long)]
        expr: Option<String>,
        /// Milliseconds.
        #[arg(long = "for", default_value_t = 10_000)]
        wait_ms: u64,
    },
    /// A PNG of the page or of one element.
    Capture {
        target: Option<String>,
        #[arg(long)]
        out: Option<String>,
    },
    /// Console output, uncaught errors and backend output collected by the session.
    Logs {
        /// Only entries after this sequence number.
        #[arg(long)]
        since: Option<u64>,
    },
    /// Evaluate JavaScript in the page and return the value as JSON.
    Eval {
        expression: Option<String>,
        #[arg(long)]
        file: Option<String>,
    },
    /// What this platform supports.
    Capabilities,
    /// Check the preconditions for a session.
    Doctor {
        #[arg(long)]
        port: Option<u16>,
    },
    #[command(name = "__session", hide = true)]
    Session { config: String },
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            use clap::error::ErrorKind as E;
            if matches!(
                e.kind(),
                E::DisplayHelp | E::DisplayVersion | E::DisplayHelpOnMissingArgumentOrSubcommand
            ) {
                e.exit();
            }
            let name = std::env::args().nth(1).unwrap_or_default();
            let message = e.render().to_string();
            let (code, out) =
                output::render(&name, &Err(Failure::new(Kind::Usage, message.trim())));
            println!("{out}");
            return ExitCode::from(code as u8);
        }
    };
    if let Command::Session { config } = &cli.command {
        let bytes = base64_decode(config);
        let config: session::Config = serde_json::from_slice(&bytes).expect("session config");
        session::run(config);
        return ExitCode::SUCCESS;
    }
    let (name, code, out) = dispatch(cli);
    let mut out = out;
    if out.get("command").is_none() {
        out["command"] = json!(name);
    }
    println!("{out}");
    ExitCode::from(code as u8)
}

fn outcome_json(name: &str, outcome: output::Outcome) -> (String, i32, Value) {
    let (code, out) = output::render(name, &outcome);
    (name.to_string(), code, out)
}

fn dispatch(cli: Cli) -> (String, i32, Value) {
    let timeout = Duration::from_secs(cli.timeout);
    let (name, args) = match cli.command {
        Command::Capabilities => return outcome_json("capabilities", Ok(capabilities())),
        Command::Launch {
            exe,
            args,
            port,
            data_dir,
            env,
            main_url,
            idle_timeout,
            ready_timeout,
        } => {
            let env = match parse_env(&env) {
                Ok(e) => e,
                Err(f) => return outcome_json("launch", Err(f)),
            };
            let config = session::Config {
                mode: "launch".into(),
                exe: Some(exe),
                args,
                env,
                cdp_port: port.unwrap_or(0),
                data_dir,
                main_url,
                idle_timeout_secs: idle_timeout,
                ready_timeout_secs: ready_timeout,
            };
            return start_session("launch", config, timeout);
        }
        Command::Attach {
            port,
            main_url,
            idle_timeout,
            ready_timeout,
        } => {
            let config = session::Config {
                mode: "attach".into(),
                exe: None,
                args: vec![],
                env: vec![],
                cdp_port: port,
                data_dir: None,
                main_url,
                idle_timeout_secs: idle_timeout,
                ready_timeout_secs: ready_timeout,
            };
            return start_session("attach", config, timeout);
        }
        Command::Doctor { port: Some(port) } => {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let checks = rt.block_on(session::doctor_checks(port));
            return outcome_json("doctor", session::doctor_outcome(checks));
        }
        Command::Doctor { port: None } => {
            if home::read_session()
                .filter(|s| s.status == "ready")
                .is_none()
            {
                let check = json!({ "check": "session", "ok": false, "detail": "no session; run launch or attach, or pass --port" });
                return outcome_json("doctor", session::doctor_outcome(vec![check]));
            }
            ("doctor", json!({}))
        }
        Command::Close => ("close", json!({})),
        Command::Targets { select } => ("targets", json!({ "select": select })),
        Command::Snapshot => ("snapshot", json!({})),
        Command::Read { target } => ("read", json!({ "target": target })),
        Command::Click {
            target,
            x,
            y,
            double,
            right,
            hover,
            require_focus,
        } => (
            "click",
            json!({ "target": target, "x": x, "y": y, "double": double, "right": right, "hover": hover, "require_focus": require_focus }),
        ),
        Command::Type {
            text,
            secret,
            secret_env,
            into,
            require_focus,
        } => {
            let (text, secret) = if let Some(var) = secret_env {
                match std::env::var(&var) {
                    Ok(v) => (v, true),
                    Err(_) => {
                        return outcome_json(
                            "type",
                            Err(Failure::new(
                                Kind::Error,
                                format!("environment variable {var} is not set"),
                            )),
                        )
                    }
                }
            } else if secret {
                let mut s = String::new();
                let _ = std::io::stdin().read_to_string(&mut s);
                (s.trim_end_matches(['\r', '\n']).to_string(), true)
            } else {
                match text {
                    Some(t) => (t, false),
                    None => {
                        return outcome_json(
                            "type",
                            Err(Failure::new(
                                Kind::Error,
                                "type needs text, --secret or --secret-env",
                            )),
                        )
                    }
                }
            };
            (
                "type",
                json!({ "text": text, "secret": secret, "into": into, "require_focus": require_focus }),
            )
        }
        Command::Key {
            chord,
            into,
            require_focus,
        } => (
            "key",
            json!({ "chord": chord, "into": into, "require_focus": require_focus }),
        ),
        Command::Wait {
            selector,
            text,
            expr,
            wait_ms,
        } => (
            "wait",
            json!({ "selector": selector, "text": text, "expr": expr, "timeout_ms": wait_ms }),
        ),
        Command::Capture { target, out } => (
            "capture",
            json!({ "target": target, "out": out.map(|o| absolute(&o)) }),
        ),
        Command::Logs { since } => ("logs", json!({ "since": since })),
        Command::Eval { expression, file } => {
            let expression = match (expression, file) {
                (_, Some(f)) => match std::fs::read_to_string(&f) {
                    Ok(s) => s,
                    Err(e) => {
                        return outcome_json(
                            "eval",
                            Err(Failure::new(
                                Kind::Error,
                                format!("could not read {f}: {e}"),
                            )),
                        )
                    }
                },
                (Some(e), None) => e,
                (None, None) => {
                    return outcome_json(
                        "eval",
                        Err(Failure::new(
                            Kind::Error,
                            "eval needs an expression or --file",
                        )),
                    )
                }
            };
            (
                "eval",
                json!({ "expression": expression, "timeout_ms": cli.timeout * 1000 }),
            )
        }
        Command::Session { .. } => unreachable!(),
    };
    let wait_extra = args["timeout_ms"]
        .as_u64()
        .map(Duration::from_millis)
        .unwrap_or_default();
    send(name, args, timeout + wait_extra)
}

fn send(name: &str, args: Value, timeout: Duration) -> (String, i32, Value) {
    let Some(info) = home::read_session().filter(|s| s.status == "ready") else {
        return outcome_json(
            name,
            Err(Failure::new(
                Kind::NoSession,
                "no session; run `pokit launch` or `pokit attach` first",
            )),
        );
    };
    match client::request(&info, name, args, timeout) {
        Ok((code, out)) => (name.to_string(), code, out),
        Err(e) => outcome_json(name, Err(Failure::new(Kind::NoSession, e))),
    }
}

fn start_session(name: &str, config: session::Config, timeout: Duration) -> (String, i32, Value) {
    if !cfg!(windows) {
        return outcome_json(
            name,
            Err(Failure::new(
                Kind::Unsupported,
                "only the Windows backend exists so far; macOS is step 4 (#6)",
            )),
        );
    }
    {
        if let Err(f) = clear_stale_session() {
            return outcome_json(name, Err(f));
        }
        let encoded = base64_encode(&serde_json::to_vec(&config).unwrap());
        let exe = std::env::current_exe().expect("own executable path");
        let pid = match proc::spawn_detached(&exe, &["__session", &encoded]) {
            Ok(pid) => pid,
            Err(e) => {
                return outcome_json(
                    name,
                    Err(Failure::new(
                        Kind::Error,
                        format!("could not start the session: {e}"),
                    )),
                )
            }
        };
        let deadline = Instant::now() + Duration::from_secs(config.ready_timeout_secs) + timeout;
        loop {
            match home::read_session().filter(|s| s.pid == pid) {
                Some(s) if s.status == "ready" => {
                    return match client::request(&s, "status", Value::Null, timeout) {
                        Ok((code, mut out)) => {
                            out["command"] = json!(name);
                            (name.to_string(), code, out)
                        }
                        Err(e) => outcome_json(name, Err(Failure::new(Kind::Error, e))),
                    };
                }
                Some(s) if s.status == "failed" => {
                    let _ = std::fs::remove_file(home::session_file());
                    let msg = s
                        .error
                        .unwrap_or_else(|| "the session failed to start".into());
                    return outcome_json(name, Err(Failure::new(Kind::Error, msg)));
                }
                _ => {}
            }
            if Instant::now() > deadline {
                let _ = proc::kill_tree(pid);
                return outcome_json(
                    name,
                    Err(Failure::new(
                        Kind::Timeout,
                        "the session did not become ready",
                    )),
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Refuses while a live session exists; removes a dead one, closing the instance it had launched.
fn clear_stale_session() -> Result<(), Failure> {
    let Some(info) = home::read_session() else {
        return Ok(());
    };
    if info.status == "ready" && client::alive(&info) {
        return Err(Failure::new(
            Kind::SessionExists,
            "a session is already running; run `pokit close` first",
        )
        .with("session_pid", info.pid));
    }
    if info.mode == "launch" {
        if let (Some(pid), Some(exe)) = (info.app_pid, &info.app_exe) {
            let wanted = std::path::Path::new(exe)
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase());
            let same_image = proc::image_name(pid).map(|n| n.to_lowercase()) == wanted;
            let same_process =
                info.app_started.is_some() && proc::process_start_time(pid) == info.app_started;
            if same_image && same_process {
                let _ = proc::kill_tree(pid);
            }
        }
    }
    let _ = std::fs::remove_file(home::session_file());
    Ok(())
}

fn capabilities() -> output::Fields {
    let windows = cfg!(windows);
    let cdp = |ok: bool| {
        if ok {
            json!({ "supported": true, "route": "cdp" })
        } else {
            unsupported()
        }
    };
    fields! {
        "platform" => std::env::consts::OS,
        "commands" => json!({
            "launch": cdp(windows), "attach": cdp(windows), "close": cdp(windows), "targets": cdp(windows),
            "snapshot": cdp(windows), "read": cdp(windows), "click": cdp(windows), "type": cdp(windows),
            "key": cdp(windows), "wait": cdp(windows), "capture": cdp(windows), "logs": cdp(windows),
            "eval": cdp(windows), "doctor": cdp(windows), "capabilities": { "supported": true },
        }),
    }
}

fn unsupported() -> Value {
    json!({ "supported": false, "reason": "only the Windows backend exists so far; macOS is step 4 (#6)" })
}

fn parse_env(pairs: &[String]) -> Result<Vec<(String, String)>, Failure> {
    pairs
        .iter()
        .map(|p| {
            p.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .ok_or_else(|| {
                    Failure::new(Kind::Error, format!("--env needs KEY=VALUE, got `{p}`"))
                })
        })
        .collect()
}

/// `path` made absolute against this process's working directory.
fn absolute(path: &str) -> String {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        return path.to_string();
    }
    std::env::current_dir()
        .map(|d| d.join(p).display().to_string())
        .unwrap_or_else(|_| path.to_string())
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn base64_decode(s: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .expect("session config encoding")
}

//! pokit: drives a running webview desktop app for an agent, answering every command in JSON.

mod cdp;
mod chord;
mod client;
mod clipboard;
mod clock;
mod devtools;
mod doctor;
mod hangul;
mod home;
mod measure;
mod native;
mod os_input;
mod output;
mod plugin;
mod proc;
mod profile;
mod redact;
mod request;
mod session;
mod snapshot;
mod support;
mod trace;
#[cfg(windows)]
mod window;

use clap::{Parser, Subcommand};
use home::{Mode, Status};
use output::{Failure, Kind};
use request::Request;
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
enum MeasureAction {
    /// Install the frame and keydown probes, and a mutation probe on `--watch`.
    Start {
        /// An element whose changes are counted.
        #[arg(long)]
        watch: Option<String>,
        /// Count only changes to this attribute of the watched element.
        #[arg(long)]
        watch_attr: Option<String>,
        /// Milliseconds; a longer frame keeps the page from counting as settled.
        #[arg(long, default_value_t = 25.0)]
        long_frame: f64,
        /// Milliseconds; frames longer than this are counted.
        #[arg(long, default_value_t = 50.0)]
        over: f64,
    },
    /// Wait for the page to settle, then report what the probes saw and remove them.
    Stop {
        /// Milliseconds without a long frame or a key for the page to count as settled.
        #[arg(long, default_value_t = 1000)]
        quiet: u64,
        /// Milliseconds to wait for that before reporting the page unsettled.
        #[arg(long, default_value_t = 10_000)]
        ceiling: u64,
    },
}

#[derive(Subcommand)]
enum WindowAction {
    /// Bring the app's main window to the front, taking the user's focus: OS input needs it.
    Activate,
    /// Move the window holding the current page, in the window's logical pixels, without
    /// bringing it to the front.
    Move {
        #[arg(long, allow_hyphen_values = true, value_parser = finite)]
        x: f64,
        #[arg(long, allow_hyphen_values = true, value_parser = finite)]
        y: f64,
    },
    /// Resize the window holding the current page, outer size in the window's logical pixels,
    /// without bringing it to the front; reports the page's new viewport.
    Resize {
        #[arg(long, value_parser = positive)]
        width: f64,
        #[arg(long, value_parser = positive)]
        height: f64,
        /// Size the page's viewport to `--width` x `--height` CSS pixels instead of the window.
        #[arg(long)]
        viewport: bool,
    },
}

#[derive(Subcommand)]
enum NativeAction {
    /// The menu bars of the app's windows, the context menu and the dialogs it has open.
    List,
    /// Choose a menu entry, written as a path: `File > Open`; in the open context menu when
    /// there is one.
    Choose { path: String },
    /// Close the context menu the app has open without choosing anything.
    Dismiss,
    /// Pick a file in the file dialog the app has open, as typing its path and pressing the
    /// default button would.
    Pick {
        path: String,
        /// The dialog's title, when the app has more than one file dialog open.
        #[arg(long)]
        dialog: Option<String>,
    },
    /// Click the app's tray icon (Tauri's tray), as the mouse on it would. A menu it opens, on a
    /// right click or, as Tauri does by default, a left one, is reached by `list` and `choose` as
    /// the context menu, at the user's cursor.
    Tray {
        /// Which tray icon, from 0, when the app has more than one.
        #[arg(long, default_value_t = 0)]
        index: usize,
        #[arg(long, conflicts_with = "double")]
        right: bool,
        #[arg(long)]
        double: bool,
    },
    /// Press a button in a dialog the app has open.
    Answer {
        button: String,
        /// The dialog's title, when the app has more than one open.
        #[arg(long)]
        dialog: Option<String>,
    },
}

#[derive(Subcommand)]
enum ClipboardAction {
    /// The clipboard's text.
    Read,
    /// Put text on the clipboard, kept out of clipboard history and cloud sync.
    Write {
        text: Option<String>,
        /// Read the text from stdin and mask it everywhere.
        #[arg(long)]
        secret: bool,
        /// Read the text from this environment variable and mask it everywhere.
        #[arg(long)]
        secret_env: Option<String>,
    },
}

#[derive(Subcommand)]
enum TraceAction {
    /// Start recording the trace.
    Start,
    /// End the trace, save it to the run record, and split the busiest main thread's time.
    Stop,
}

#[derive(Subcommand)]
enum ProfileAction {
    /// Start sampling the page's JavaScript.
    Start {
        /// Microseconds between samples.
        #[arg(long, default_value_t = 100)]
        interval_us: u64,
    },
    /// End the profile, save it to the run record, and rank self time by function and by file.
    Stop {
        /// How many functions and files to list.
        #[arg(long, default_value_t = 20)]
        top: usize,
    },
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
        /// `os` sends OS input, which needs the app in front (`window activate`) and moves the cursor.
        #[arg(long, value_enum, default_value_t = request::Route::Cdp)]
        route: request::Route,
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
        /// `os` types through the OS, which needs the app in front, without IME composition.
        #[arg(long, value_enum, default_value_t = request::Route::Cdp)]
        route: request::Route,
    },
    /// Press a chord written with physical key names, e.g. `Ctrl+Equal`, into the focused element or `--into`.
    Key {
        chord: String,
        #[arg(long)]
        into: Option<String>,
        #[arg(long)]
        require_focus: Option<String>,
        /// `os` presses the keys through the OS, which needs the app in front.
        #[arg(long, value_enum, default_value_t = request::Route::Cdp)]
        route: request::Route,
    },
    /// Send N key-downs at a fixed interval without waiting for the page, then one key-up.
    Hold {
        chord: String,
        #[arg(long)]
        count: u32,
        /// Milliseconds between key-downs.
        #[arg(long, default_value_t = 33)]
        interval: u64,
        #[arg(long)]
        into: Option<String>,
        #[arg(long)]
        require_focus: Option<String>,
        /// `os` sends the keys through the OS, which needs the app in front (`window activate`).
        #[arg(long, value_enum, default_value_t = request::Route::Cdp)]
        route: request::Route,
        /// Run the same keys on CDP, then on the OS, each measured, and report each route's
        /// send-to-handling latency and their difference. Needs the app in front.
        #[arg(long)]
        compare: bool,
    },
    /// Drag from an element to another or to a page point, with the left button held. Through
    /// the OS the path may leave the window, as a hand's can.
    Drag {
        from: String,
        /// The element to drop on, which must be in view.
        #[arg(long, conflicts_with_all = ["to_x", "to_y"], required_unless_present = "to_x")]
        to: Option<String>,
        /// The page point to drop at, in CSS pixels; outside the page needs `--route os`.
        #[arg(long, allow_hyphen_values = true, value_parser = finite, requires = "to_y")]
        to_x: Option<f64>,
        #[arg(long, allow_hyphen_values = true, value_parser = finite, requires = "to_x")]
        to_y: Option<f64>,
        /// A page point the path passes through, `X,Y`; repeat for more.
        #[arg(long, allow_hyphen_values = true, value_parser = point)]
        via: Vec<(f64, f64)>,
        /// Pointer moves on each straight part of the path.
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..=1000))]
        steps: u32,
        /// `os` drags through the OS, which needs the app in front and moves the cursor.
        #[arg(long, value_enum, default_value_t = request::Route::Cdp)]
        route: request::Route,
    },
    /// Turn the mouse wheel over an element or a point, or the middle of the page.
    Wheel {
        target: Option<String>,
        #[arg(long)]
        x: Option<f64>,
        #[arg(long)]
        y: Option<f64>,
        /// Notches to turn; positive scrolls down, negative up.
        #[arg(long, allow_hyphen_values = true)]
        notches: i32,
        /// `os` turns the wheel through the OS, which needs the app in front and moves the cursor.
        #[arg(long, value_enum, default_value_t = request::Route::Cdp)]
        route: request::Route,
    },
    /// Probe frames, keys and an optional element between `measure start` and `measure stop`.
    Measure {
        #[command(subcommand)]
        action: MeasureAction,
    },
    /// Bring the launched app to the front, or move and resize the window holding the current
    /// page.
    Window {
        #[command(subcommand)]
        action: WindowAction,
    },
    /// Read the launched app's menu bars and dialogs, choose a menu entry, or answer a dialog.
    Native {
        #[command(subcommand)]
        action: NativeAction,
    },
    /// Read or write the clipboard's text; the user's clipboard is given back at `close`.
    Clipboard {
        #[command(subcommand)]
        action: ClipboardAction,
    },
    /// Record a timeline trace around the commands run between `trace start` and `trace stop`.
    Trace {
        #[command(subcommand)]
        action: TraceAction,
    },
    /// Record a CPU profile around the commands run between `profile start` and `profile stop`.
    Profile {
        #[command(subcommand)]
        action: ProfileAction,
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
    /// A PNG of the page or of one element, or with `--window` of the window holding the page.
    Capture {
        target: Option<String>,
        #[arg(long)]
        out: Option<String>,
        /// The window holding the page with the app's own menus and dialogs over it, cut to the
        /// window; other apps' windows and the page's own popups are left out.
        #[arg(long, conflicts_with = "target")]
        window: bool,
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
    let request = match cli.command {
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
            let webview2_only = [
                ("--port", port.is_some()),
                ("--data-dir", data_dir.is_some()),
            ];
            if let (true, Some((flag, _))) = (
                cfg!(target_os = "macos"),
                webview2_only.iter().find(|(_, given)| *given),
            ) {
                let message = format!(
                    "{flag} names WebView2's debugging port or profile; it has no meaning on macOS"
                );
                let refused = Failure::new(Kind::Unsupported, message).with("platform", "macos");
                return outcome_json("launch", Err(refused));
            }
            let env = match parse_env(&env) {
                Ok(e) => e,
                Err(f) => return outcome_json("launch", Err(f)),
            };
            let config = session::Config {
                mode: Mode::Launch,
                exe: Some(exe),
                args,
                env,
                cdp_port: port.unwrap_or(0),
                data_dir,
                main_url,
                idle_timeout_secs: idle_timeout,
                ready_timeout_secs: ready_timeout,
            };
            return start_session(config, timeout);
        }
        Command::Attach {
            port,
            main_url,
            idle_timeout,
            ready_timeout,
        } => {
            let config = session::Config {
                mode: Mode::Attach,
                exe: None,
                args: vec![],
                env: vec![],
                cdp_port: port,
                data_dir: None,
                main_url,
                idle_timeout_secs: idle_timeout,
                ready_timeout_secs: ready_timeout,
            };
            return start_session(config, timeout);
        }
        Command::Doctor { .. } if cfg!(target_os = "macos") => {
            return outcome_json("doctor", Err(support::refused("doctor")))
        }
        Command::Doctor { port: Some(port) } => {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let checks = rt.block_on(doctor::checks(port));
            return outcome_json("doctor", doctor::outcome(checks));
        }
        Command::Doctor { port: None } => {
            if home::read_session()
                .filter(|s| s.status == Status::Ready)
                .is_none()
            {
                let check = json!({ "check": "session", "ok": false, "detail": "no session; run launch or attach, or pass --port" });
                return outcome_json("doctor", doctor::outcome(vec![check]));
            }
            Request::Doctor
        }
        Command::Close => Request::Close,
        Command::Targets { select } => Request::Targets { select },
        Command::Snapshot => Request::Snapshot,
        Command::Read { target } => Request::Read { target },
        Command::Click {
            target,
            x,
            y,
            double,
            right,
            hover,
            require_focus,
            route,
        } => Request::Click {
            target,
            x,
            y,
            double,
            right,
            hover,
            require_focus,
            route,
        },
        Command::Type {
            text,
            secret,
            secret_env,
            into,
            require_focus,
            route,
        } => {
            let (text, secret) = match secret_text("type", text, secret, secret_env) {
                Ok(t) => t,
                Err(out) => return out,
            };
            Request::Type {
                text,
                secret,
                into,
                require_focus,
                route,
            }
        }
        Command::Key {
            chord,
            into,
            require_focus,
            route,
        } => Request::Key {
            chord,
            into,
            require_focus,
            route,
        },
        Command::Hold {
            chord,
            count,
            interval,
            into,
            require_focus,
            route,
            compare,
        } => Request::Hold {
            chord,
            count,
            interval_ms: interval,
            into,
            require_focus,
            route,
            compare,
        },
        Command::Drag {
            from,
            to,
            to_x,
            to_y,
            via,
            steps,
            route,
        } => Request::Drag {
            from,
            to,
            to_x,
            to_y,
            via,
            steps,
            route,
        },
        Command::Wheel {
            target,
            x,
            y,
            notches,
            route,
        } => Request::Wheel {
            target,
            x,
            y,
            notches,
            route,
        },
        Command::Measure {
            action:
                MeasureAction::Start {
                    watch,
                    watch_attr,
                    long_frame,
                    over,
                },
        } => Request::MeasureStart {
            watch,
            watch_attr,
            long_frame_ms: long_frame,
            over_ms: over,
        },
        Command::Measure {
            action: MeasureAction::Stop { quiet, ceiling },
        } => Request::MeasureStop {
            quiet_ms: quiet,
            ceiling_ms: ceiling,
        },
        Command::Window {
            action: WindowAction::Activate,
        } => Request::WindowActivate,
        Command::Window {
            action: WindowAction::Move { x, y },
        } => Request::WindowMove { x, y },
        Command::Window {
            action:
                WindowAction::Resize {
                    width,
                    height,
                    viewport,
                },
        } => Request::WindowResize {
            width,
            height,
            viewport,
        },
        Command::Native {
            action: NativeAction::List,
        } => Request::NativeList,
        Command::Native {
            action: NativeAction::Choose { path },
        } => Request::NativeChoose { path },
        Command::Native {
            action: NativeAction::Answer { button, dialog },
        } => Request::NativeAnswer { button, dialog },
        Command::Native {
            action: NativeAction::Dismiss,
        } => Request::NativeDismiss,
        Command::Native {
            action: NativeAction::Pick { path, dialog },
        } => Request::NativePick {
            path: absolute(&path),
            dialog,
        },
        Command::Native {
            action:
                NativeAction::Tray {
                    index,
                    right,
                    double,
                },
        } => Request::NativeTray {
            index,
            right,
            double,
        },
        Command::Clipboard {
            action: ClipboardAction::Read,
        } => Request::ClipboardRead,
        Command::Clipboard {
            action:
                ClipboardAction::Write {
                    text,
                    secret,
                    secret_env,
                },
        } => match secret_text("clipboard_write", text, secret, secret_env) {
            Ok((text, secret)) => Request::ClipboardWrite { text, secret },
            Err(out) => return out,
        },
        Command::Trace {
            action: TraceAction::Start,
        } => Request::TraceStart,
        Command::Trace {
            action: TraceAction::Stop,
        } => Request::TraceStop,
        Command::Profile {
            action: ProfileAction::Start { interval_us },
        } => Request::ProfileStart { interval_us },
        Command::Profile {
            action: ProfileAction::Stop { top },
        } => Request::ProfileStop { top },
        Command::Wait {
            selector,
            text,
            expr,
            wait_ms,
        } => Request::Wait {
            selector,
            text,
            expr,
            timeout_ms: wait_ms,
        },
        Command::Capture {
            target,
            out,
            window,
        } => Request::Capture {
            target,
            out: out.map(|o| absolute(&o)),
            window,
        },
        Command::Logs { since } => Request::Logs { since },
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
            Request::Eval {
                expression,
                timeout_ms: cli.timeout * 1000,
            }
        }
        Command::Session { .. } => unreachable!(),
    };
    let wait = timeout + request.extra_wait();
    send(request, wait)
}

fn send(request: Request, timeout: Duration) -> (String, i32, Value) {
    let name = request.kind().name();
    if !support::session_answers(name) {
        return outcome_json(name, Err(support::refused(name)));
    }
    let Some(info) = home::read_session().filter(|s| s.status == Status::Ready) else {
        return outcome_json(
            name,
            Err(Failure::new(
                Kind::NoSession,
                "no session; run `pokit launch` or `pokit attach` first",
            )),
        );
    };
    match client::request(&info, &request, timeout) {
        #[cfg(windows)]
        Ok((code, mut out)) => {
            if let Some(g) = out.as_object_mut().and_then(|o| o.remove("give_back")) {
                out["foreground_given_back"] = json!(give_back(&g));
            }
            (name.to_string(), code, out)
        }
        #[cfg(not(windows))]
        Ok((code, out)) => (name.to_string(), code, out),
        Err(e) => outcome_json(name, Err(Failure::new(Kind::NoSession, e))),
    }
}

/// Gives the foreground back to the window the session names, while the app it names holds it,
/// for up to a second; whether that window is in front at the end.
#[cfg(windows)]
fn give_back(g: &Value) -> bool {
    let (Some(hwnd), Some(app)) = (g["hwnd"].as_i64(), g["app_pid"].as_u64()) else {
        return false;
    };
    let deadline = Instant::now() + Duration::from_secs(1);
    while proc::foreground().1 == app as u32 && Instant::now() < deadline {
        proc::set_foreground(hwnd as isize);
        std::thread::sleep(Duration::from_millis(50));
    }
    proc::foreground().0 == hwnd as isize
}

fn start_session(config: session::Config, timeout: Duration) -> (String, i32, Value) {
    let name = config.mode.name();
    if !support::answers(name) {
        return outcome_json(name, Err(support::refused(name)));
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
                Some(s) if s.status == Status::Ready => {
                    return match client::request(&s, &Request::Status, timeout) {
                        Ok((code, mut out)) => {
                            out["command"] = json!(name);
                            (name.to_string(), code, out)
                        }
                        Err(e) => outcome_json(name, Err(Failure::new(Kind::Error, e))),
                    };
                }
                Some(s) if s.status == Status::Failed => {
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
    if info.status == Status::Ready && client::alive(&info) {
        return Err(Failure::new(
            Kind::SessionExists,
            "a session is already running; run `pokit close` first",
        )
        .with("session_pid", info.pid));
    }
    if info.mode == Mode::Launch {
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

/// What this platform supports, for every command the CLI defines.
fn capabilities() -> output::Fields {
    use clap::CommandFactory;
    let mut commands = serde_json::Map::new();
    for sub in Cli::command()
        .get_subcommands()
        .filter(|s| !s.is_hide_set())
    {
        let support = match sub.get_name() {
            "capabilities" => json!({ "supported": true }),
            "window" if support::answers("window") => {
                json!({ "supported": true, "actions": {
                    "activate": { "takes_focus": true },
                    "move": { "takes_focus": false },
                    "resize": { "takes_focus": false },
                } })
            }
            "native" if cfg!(windows) => json!({
                "supported": true,
                "route": output::ENGINE,
                "takes_focus": "a context or tray menu or a file dialog the app opens holds the foreground while it is open; `native choose`, `native dismiss` and `native pick` give it back",
                "tray": "icons made with tray-icon (Tauri's tray) only",
            }),
            "capture" if cfg!(windows) => json!({
                "supported": true,
                "route": output::ENGINE,
                "window": { "native_ui": true, "takes_focus": false, "needs": "a launched session" },
            }),
            name if support::answers(name) => match support::routes(name) {
                Some(routes) => json!({ "supported": true, "routes": routes }),
                None => json!({ "supported": true, "route": output::ENGINE }),
            },
            name => json!({ "supported": false, "reason": support::reason(name) }),
        };
        commands.insert(sub.get_name().to_string(), support);
    }
    fields! {
        "platform" => std::env::consts::OS,
        "commands" => Value::Object(commands),
    }
}

/// The text a command was given: from stdin with `--secret`, from an environment variable with
/// `--secret-env` (both masked everywhere), or as its argument.
fn secret_text(
    name: &str,
    text: Option<String>,
    secret: bool,
    secret_env: Option<String>,
) -> Result<(String, bool), (String, i32, Value)> {
    if let Some(var) = secret_env {
        return std::env::var(&var).map(|v| (v, true)).map_err(|_| {
            outcome_json(
                name,
                Err(Failure::new(
                    Kind::Error,
                    format!("environment variable {var} is not set"),
                )),
            )
        });
    }
    if secret {
        let mut s = String::new();
        let _ = std::io::stdin().read_to_string(&mut s);
        return Ok((s.trim_end_matches(['\r', '\n']).to_string(), true));
    }
    text.map(|t| (t, false)).ok_or_else(|| {
        outcome_json(
            name,
            Err(Failure::new(
                Kind::Error,
                format!("{name} needs text, --secret or --secret-env"),
            )),
        )
    })
}

/// A finite number.
fn finite(s: &str) -> Result<f64, String> {
    match s.parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(v),
        _ => Err(format!("`{s}` is not a finite number")),
    }
}

/// A page point written `X,Y`.
fn point(s: &str) -> Result<(f64, f64), String> {
    let (x, y) = s
        .split_once(',')
        .ok_or_else(|| format!("`{s}` is not a point written X,Y"))?;
    Ok((finite(x.trim())?, finite(y.trim())?))
}

/// A finite number above 0.
fn positive(s: &str) -> Result<f64, String> {
    finite(s).and_then(|v| {
        if v > 0.0 {
            Ok(v)
        } else {
            Err(format!("`{s}` is not above 0"))
        }
    })
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

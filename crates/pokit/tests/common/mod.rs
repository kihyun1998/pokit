//! Drives the real `pokit` binary against the fixture app: a test either in its own pokit home
//! with its own instance, or on the instance its test binary shares, one test at a time.

#![allow(dead_code)]

#[cfg(windows)]
pub mod window;

use serde_json::Value;
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// The fixture app, built once per test run into its own target directory.
pub fn fixture_exe() -> &'static Path {
    static EXE: OnceLock<PathBuf> = OnceLock::new();
    EXE.get_or_init(|| build_fixture("fixture", &[]))
}

/// The fixture app's test build, with pokit's plugin feature on, in a target directory of its own.
pub fn fixture_test_build_exe() -> &'static Path {
    static EXE: OnceLock<PathBuf> = OnceLock::new();
    EXE.get_or_init(|| build_fixture("fixture-pokit", &["--features", "pokit"]))
}

fn build_fixture(target_name: &str, extra: &[&str]) -> PathBuf {
    let root = workspace_root();
    let target = root.join("target").join(target_name);
    let built = Command::new(env!("CARGO"))
        .args(["build", "-q", "-p", "pokit-fixture", "--target-dir"])
        .arg(&target)
        .args(extra)
        .current_dir(&root)
        .stdin(Stdio::null())
        .output()
        .expect("cargo build of the fixture app did not start");
    if !built.status.success() {
        let err = String::from_utf8_lossy(&built.stderr);
        let lines: Vec<&str> = err.lines().collect();
        let tail = lines[lines.len().saturating_sub(60)..].join("\n");
        panic!("fixture app failed to build ({}):\n{tail}", built.status);
    }
    target.join("debug").join(if cfg!(windows) {
        "pokit-fixture.exe"
    } else {
        "pokit-fixture"
    })
}

pub struct Run {
    pub code: i32,
    pub out: Value,
}

static BUSY: Mutex<bool> = Mutex::new(false);
static FREE: Condvar = Condvar::new();
thread_local! {
    static HELD: Cell<u32> = const { Cell::new(0) };
}

/// A test's turn with the fixture apps: one test of a test binary at a time launches or drives
/// one. Taken again on a thread that holds it, it is held until the last is dropped.
pub struct Turn(());

impl Turn {
    pub fn take() -> Turn {
        HELD.with(|held| {
            if held.get() == 0 {
                let mut busy = BUSY.lock().unwrap_or_else(|e| e.into_inner());
                while *busy {
                    busy = FREE.wait(busy).unwrap_or_else(|e| e.into_inner());
                }
                *busy = true;
            }
            held.set(held.get() + 1);
        });
        Turn(())
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        HELD.with(|held| {
            held.set(held.get() - 1);
            if held.get() == 0 {
                *BUSY.lock().unwrap_or_else(|e| e.into_inner()) = false;
                FREE.notify_one();
            }
        });
    }
}

/// One pokit home with, once launched, one session on the fixture app; closed on drop. A test's
/// own holds the test's turn.
pub struct Pokit {
    pub home: PathBuf,
    pub launched: Option<Value>,
    turn: Option<Turn>,
    /// Whether dropping it removes the home.
    owns_home: bool,
}

impl Pokit {
    pub fn new(name: &str) -> Self {
        let turn = Turn::take();
        let mut p = Pokit::unturned(name);
        p.turn = Some(turn);
        p
    }

    /// Another handle on an existing home, for running commands from a second thread; dropping
    /// it leaves the home and its session alone.
    pub fn on_home(home: PathBuf) -> Self {
        Pokit {
            home,
            launched: None,
            turn: None,
            owns_home: false,
        }
    }

    fn unturned(name: &str) -> Self {
        static SWEPT: OnceLock<()> = OnceLock::new();
        SWEPT.get_or_init(|| {
            sweep_homes(&std::env::temp_dir(), KEEP_FAILED_HOME);
        });
        let home = std::env::temp_dir().join(format!("pokit-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        Pokit {
            home,
            launched: None,
            turn: None,
            owns_home: true,
        }
    }

    pub fn launch_fixture(name: &str) -> Self {
        let mut p = Pokit::new(name);
        let exe = if cfg!(target_os = "macos") {
            fixture_test_build_exe()
        } else {
            fixture_exe()
        };
        let exe = exe.to_str().unwrap().to_string();
        let r = p.run(&["launch", &exe]);
        assert_eq!(r.code, 0, "launch failed: {}", r.out);
        p.launched = Some(r.out);
        p
    }

    pub fn run(&self, args: &[&str]) -> Run {
        self.run_with_stdin(args, None)
    }

    /// The value of `expression` on the page, or `Null` whatever goes wrong: for reporting on a
    /// test that is already failing.
    fn eval_quietly(&self, expression: &str) -> Value {
        Command::new(env!("CARGO_BIN_EXE_pokit"))
            .args(["eval", expression])
            .env("POKIT_HOME", &self.home)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok())
            .map_or(Value::Null, |v| v["value"].clone())
    }

    pub fn run_with_stdin(&self, args: &[&str], stdin: Option<&str>) -> Run {
        self.run_in(&std::env::current_dir().unwrap(), args, stdin)
    }

    /// Runs pokit with `dir` as its working directory.
    pub fn run_in(&self, dir: &Path, args: &[&str], stdin: Option<&str>) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_pokit"));
        cmd.args(args)
            .current_dir(dir)
            .env("POKIT_HOME", &self.home)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("pokit did not start");
        if let Some(text) = stdin {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
        }
        let output = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let out = serde_json::from_str(stdout.trim()).unwrap_or_else(|_| {
            panic!(
                "stdout is not one JSON object: {stdout:?} (stderr: {})",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        Run {
            code: output.status.code().unwrap_or(-1),
            out,
        }
    }

    /// An element's text, as `read` gives it.
    pub fn read_text(&self, target: &str) -> String {
        self.run(&["read", target]).out["text"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }

    /// An element's value, as `read` gives it.
    pub fn read_value(&self, target: &str) -> String {
        self.run(&["read", target]).out["value"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }

    pub fn app_pid(&self) -> u32 {
        self.launched.as_ref().unwrap()["app_pid"].as_u64().unwrap() as u32
    }

    /// Writes `session` as this home's session file, as if a session had written it.
    pub fn write_session_file(&self, session: &Value) {
        std::fs::write(self.home.join("session.json"), session.to_string()).unwrap();
    }

    /// Every file under this home, as text, for checking what pokit wrote.
    pub fn all_written_text(&self) -> String {
        fn walk(dir: &Path, out: &mut String) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if let Ok(text) = std::fs::read_to_string(&path) {
                    out.push_str(&text);
                }
            }
        }
        let mut out = String::new();
        walk(&self.home.join("runs"), &mut out);
        out
    }
}

impl Drop for Pokit {
    fn drop(&mut self) {
        // A failing test prints what the page received, and whether it reloaded since launch.
        if let (true, Some(launched)) = (std::thread::panicking(), &self.launched) {
            let page = self.eval_quietly(
                "JSON.stringify({ origin: performance.timeOrigin, active: document.activeElement?.id, \
                 focus: document.hasFocus(), events: window.__events })",
            );
            eprintln!(
                "page at failure (home {}, time origin at launch {}): {page}",
                self.home.display(),
                launched["main"]["time_origin"]
            );
        }
        if self.launched.is_some() {
            close_home(&self.home);
        }
        if self.owns_home && !std::thread::panicking() {
            remove_home(&self.home);
        }
    }
}

fn close_home(home: &Path) {
    let _ = Command::new(env!("CARGO_BIN_EXE_pokit"))
        .arg("close")
        .env("POKIT_HOME", home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Removes a home, retrying for up to 3 s while the instance's WebView2 processes still hold its
/// profile.
pub fn remove_home(home: &Path) {
    for _ in 0..30 {
        if std::fs::remove_dir_all(home).is_ok() || !home.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// How long the home of a failing test is kept after it last changed.
const KEEP_FAILED_HOME: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Removes the `pokit-test-*` folders in `dir` last changed at least `older_than` ago whose
/// session is not running (no `session.json`, or its `pid` is not alive); the number removed.
pub fn sweep_homes(dir: &Path, older_than: std::time::Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let is_home = entry
            .file_name()
            .to_string_lossy()
            .starts_with("pokit-test-")
            && entry.file_type().is_ok_and(|t| t.is_dir());
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| t.elapsed().unwrap_or_default() >= older_than);
        if !is_home || !old {
            continue;
        }
        let running = std::fs::read_to_string(path.join("session.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| v["pid"].as_u64())
            .is_some_and(|pid| process_alive(pid as u32));
        if !running && std::fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// The fixture instance this test binary shares, ready for one test: the page reloaded and
/// scrolled to the top. It is launched again when the last test left the app in a state a reload
/// does not undo (another window, a dialog, a menu) or its session ended. It is closed, and its
/// home removed, when the test binary exits; its session also ends after `SHARED_IDLE` without
/// commands.
pub struct Shared {
    app: MutexGuard<'static, Option<Pokit>>,
    _turn: Turn,
}

/// How long the shared instance outlives its last command.
const SHARED_IDLE: &str = "20";

static SHARED: Mutex<Option<Pokit>> = Mutex::new(None);

pub fn shared_fixture() -> Shared {
    let turn = Turn::take();
    let mut app = SHARED.lock().unwrap_or_else(|e| e.into_inner());
    if !app.as_ref().is_some_and(reset) {
        *app = None;
        let mut p = Pokit::unturned("shared");
        let exe = if cfg!(target_os = "macos") {
            fixture_test_build_exe()
        } else {
            fixture_exe()
        };
        let r = p.run(&[
            "launch",
            exe.to_str().unwrap(),
            "--idle-timeout",
            SHARED_IDLE,
        ]);
        assert_eq!(r.code, 0, "the shared instance did not launch: {}", r.out);
        p.launched = Some(r.out);
        *app = Some(p);
        static AT_EXIT: OnceLock<()> = OnceLock::new();
        AT_EXIT.get_or_init(|| {
            extern "C" {
                fn atexit(f: extern "C" fn()) -> i32;
            }
            // SAFETY: registers a plain function to run when the process exits.
            unsafe { atexit(close_shared) };
        });
    }
    Shared { app, _turn: turn }
}

/// Closes the shared instance as the test binary exits.
extern "C" fn close_shared() {
    let app = match SHARED.try_lock() {
        Ok(app) => app,
        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return,
    };
    if let Some(p) = app.as_ref() {
        close_home(&p.home);
        remove_home(&p.home);
    }
}

/// Puts the shared instance back as launched, for the next test; whether it could.
fn reset(p: &Pokit) -> bool {
    let targets = p.run(&["targets"]);
    if targets.code != 0 || targets.out["targets"].as_array().map(Vec::len) != Some(1) {
        return false;
    }
    let native = p.run(&["native", "list"]);
    if native.code == 0
        && (native.out["dialogs"]
            .as_array()
            .is_some_and(|d| !d.is_empty())
            || !native.out["context_menu"].is_null())
    {
        return false;
    }
    for stop in [["measure", "stop"], ["trace", "stop"], ["profile", "stop"]] {
        p.run(&stop);
    }
    let origin = p.run(&["eval", "performance.timeOrigin"]).out["value"].clone();
    if p.run(&["eval", "location.reload(); true"]).code != 0 {
        return false;
    }
    eventually(10_000, || {
        let now = p.run(&["eval", "performance.timeOrigin"]);
        now.code == 0 && now.out["value"] != origin
    }) && p.run(&["eval", "window.scrollTo(0, 0); true"]).code == 0
}

impl std::ops::Deref for Shared {
    type Target = Pokit;

    fn deref(&self) -> &Pokit {
        self.app.as_ref().expect("a shared instance")
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        if std::thread::panicking() {
            if let Some(p) = self.app.as_ref() {
                let page = p.eval_quietly("JSON.stringify(window.__events)");
                eprintln!(
                    "shared instance at failure (home {}): {page}",
                    p.home.display()
                );
            }
        }
    }
}

/// Whether a process with this id is running.
#[cfg(windows)]
pub fn process_alive(pid: u32) -> bool {
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
}

/// Whether a process with this id is running.
#[cfg(not(windows))]
pub fn process_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Polls `cond` every 100 ms for up to `ms`.
pub fn eventually(ms: u64, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    loop {
        if cond() {
            return true;
        }
        if std::time::Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

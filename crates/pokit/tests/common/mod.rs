//! Drives the real `pokit` binary against the fixture app, each test in its own pokit home.

#![allow(dead_code)]

#[cfg(windows)]
pub mod window;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// The fixture app, built once per test run into its own target directory.
pub fn fixture_exe() -> &'static Path {
    static EXE: OnceLock<PathBuf> = OnceLock::new();
    EXE.get_or_init(|| {
        let root = workspace_root();
        let target = root.join("target").join("fixture");
        let status = Command::new(env!("CARGO"))
            .args(["build", "-q", "-p", "pokit-fixture", "--target-dir"])
            .arg(&target)
            .current_dir(&root)
            .status()
            .expect("cargo build of the fixture app did not start");
        assert!(status.success(), "fixture app failed to build");
        target.join("debug").join(if cfg!(windows) {
            "pokit-fixture.exe"
        } else {
            "pokit-fixture"
        })
    })
}

pub struct Run {
    pub code: i32,
    pub out: Value,
}

/// One pokit home with, once launched, one session on the fixture app; closed on drop.
pub struct Pokit {
    pub home: PathBuf,
    pub launched: Option<Value>,
}

impl Pokit {
    pub fn new(name: &str) -> Self {
        let home = std::env::temp_dir().join(format!("pokit-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        Pokit {
            home,
            launched: None,
        }
    }

    pub fn launch_fixture(name: &str) -> Self {
        let mut p = Pokit::new(name);
        let exe = fixture_exe().to_str().unwrap().to_string();
        let r = p.run(&["launch", &exe]);
        assert_eq!(r.code, 0, "launch failed: {}", r.out);
        p.launched = Some(r.out);
        p
    }

    pub fn run(&self, args: &[&str]) -> Run {
        self.run_with_stdin(args, None)
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
            let page = self
                .run(&[
                    "eval",
                    "JSON.stringify({ origin: performance.timeOrigin, active: document.activeElement?.id, \
                     focus: document.hasFocus(), events: window.__events })",
                ])
                .out["value"]
                .clone();
            eprintln!(
                "page at failure (home {}, time origin at launch {}): {page}",
                self.home.display(),
                launched["main"]["time_origin"]
            );
        }
        if self.launched.is_some() {
            let _ = Command::new(env!("CARGO_BIN_EXE_pokit"))
                .arg("close")
                .env("POKIT_HOME", &self.home)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        // A passing test's home is removed; it holds the instance's WebView2 profile until its processes exit.
        for _ in 0..30 {
            if std::thread::panicking() {
                break;
            }
            if std::fs::remove_dir_all(&self.home).is_ok() || !self.home.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

/// Whether a process with this id is running.
pub fn process_alive(pid: u32) -> bool {
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
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

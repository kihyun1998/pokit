//! Starting the app with its own WebView2 profile and debugging port, and waiting until its main
//! page is ready.

use super::commands::evaluate;
use super::{Config, State};
use crate::cdp::Cdp;
use crate::home;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Starts the app with its own WebView2 profile and the debugging port open.
pub(super) fn spawn_app(config: &Config) -> Result<(tokio::process::Child, PathBuf), String> {
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

/// While `still()` holds, gives the foreground back to the window that had it before whenever
/// the app takes it; `during` names what pokit was doing, for the log. Returns how many times it
/// gave it back, and how many times Windows refused.
#[cfg(windows)]
pub(super) async fn keep_foreground(
    state: Arc<State>,
    app_pid: u32,
    before: (isize, u32),
    during: &'static str,
    still: impl Fn() -> bool,
) -> (u32, u32) {
    let (mut gave_back, mut refused) = (0, 0);
    if before.0 == 0 || before.1 == app_pid {
        return (0, 0);
    }
    while still() {
        if crate::proc::foreground().1 == app_pid {
            let ok = crate::proc::set_foreground(before.0);
            if ok {
                gave_back += 1;
            } else {
                refused += 1;
            }
            state.log(
                "session",
                "info",
                "",
                format!(
                    "the app took the foreground {during}; {} it back to pid {}",
                    if ok { "gave" } else { "could not give" },
                    before.1
                ),
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    (gave_back, refused)
}

/// Removes the profiles under `instances/` left by sessions that have ended, retrying for up to
/// 30 s while their files are still held. Profiles are named `<stamp>-<session pid>`; the session
/// `own_pid` and any other running pokit process keep theirs.
pub(super) async fn sweep_profiles(own_pid: u32) {
    let me = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    let ended =
        |pid: u32| pid != own_pid && crate::proc::image_name(pid).map(|n| n.to_lowercase()) != me;
    let mut left: Vec<PathBuf> = std::fs::read_dir(home::home().join("instances"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|path| path.is_dir())
        .filter(|path| profile_session_pid(path).map(ended).unwrap_or(false))
        .collect();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !left.is_empty() && Instant::now() < deadline {
        left.retain(|dir| std::fs::remove_dir_all(dir).is_err() && dir.exists());
        if !left.is_empty() {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

/// The session pid in a profile directory named `<YYYYMMDD>-<HHMMSS>-<pid>`.
fn profile_session_pid(dir: &Path) -> Option<u32> {
    let name = dir.file_name()?.to_str()?;
    let mut parts = name.splitn(3, '-');
    let (date, time, pid) = (parts.next()?, parts.next()?, parts.next()?);
    let digits = |s: &str, n: usize| s.len() == n && s.chars().all(|c| c.is_ascii_digit());
    if !digits(date, 8) || !digits(time, 6) {
        return None;
    }
    pid.parse().ok()
}

/// Where the browser reports the debugging port it picked, under a WebView2 profile directory.
fn active_port_file(data_dir: &Path) -> PathBuf {
    data_dir.join("EBWebView").join("DevToolsActivePort")
}

/// Waits for the browser to report the debugging port it picked.
pub(super) async fn active_port(
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
pub(super) async fn wait_ready(
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
pub(super) async fn probe_ready(
    cdp: &Cdp,
    wanted: &str,
    timeout: Duration,
) -> Result<Value, Value> {
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

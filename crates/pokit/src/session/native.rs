//! `native list` / `native choose` / `native answer`: the launched app's menu bars and dialogs.

use super::State;
use crate::fields;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long after `native choose` or `native answer` the session gives the foreground back, for
/// a dialog or window the app brings forward in answer.
const NATIVE_FOCUS_GRACE: Duration = Duration::from_secs(2);
use crate::output::{Failure, Kind, Outcome};
use serde_json::json;

impl State {
    /// The launched app's process; an attached session does not know it.
    pub(super) fn native_pid(&self) -> Result<u32, Failure> {
        self.app_pid.ok_or_else(|| {
            Failure::new(
                Kind::Unsupported,
                "native needs the app's process, which only a launched session knows",
            )
        })
    }

    pub(super) async fn window_activate_cmd(&self) -> Outcome {
        let pid = self.native_pid()?;
        let previous = crate::proc::foreground().1;
        match blocking(move || crate::os_input::activate(pid)).await? {
            Some(how) => Ok(fields! {
                "frontmost" => true,
                "how" => format!("{how:?}"),
                "previous_pid" => previous,
            }),
            None => Err(Failure::new(
                Kind::GuardRefused,
                "Windows did not let the app come to the front",
            )
            .with("foreground_pid", crate::proc::foreground().1)),
        }
    }

    /// The launched app's process, for OS input; an attached session does not know it.
    pub(super) fn os_pid(&self) -> Result<u32, Failure> {
        self.native_pid()
    }

    pub(super) async fn native_list_cmd(&self) -> Outcome {
        let pid = self.native_pid()?;
        let (menus, context, tray, dialogs) = blocking(move || {
            let menus = crate::native::menus(pid);
            let context = crate::native::context_menu(pid);
            let tray = crate::native::tray_icons(pid);
            crate::native::dialogs(pid).map(|d| (menus, context, tray, d))
        })
        .await?;
        let mut text = String::new();
        let windows: Vec<_> = menus
            .iter()
            .map(|(title, menu, _)| {
                text.push_str(&format!("menu of \"{title}\":\n"));
                crate::native::render(menu, 1, &mut text);
                json!({ "title": title, "menu": menu })
            })
            .collect();
        if let Some(menu) = &context {
            text.push_str("context menu:\n");
            crate::native::render(menu, 1, &mut text);
        }
        for d in &dialogs {
            text.push_str(&format!("dialog \"{}\":\n", d.title));
            for line in &d.text {
                text.push_str(&format!("  text \"{line}\"\n"));
            }
            for b in &d.buttons {
                text.push_str(&format!("  button \"{b}\"\n"));
            }
        }
        Ok(fields! {
            "windows" => windows,
            "context_menu" => context,
            "tray_icons" => tray,
            "dialogs" => dialogs,
            "text" => text,
        })
    }

    /// Gives the foreground back, for a while, whenever the app takes it in answer to a command.
    /// Gives the foreground back whenever the app takes it, from now until `NATIVE_FOCUS_GRACE`
    /// after the returned deadline is last pushed back.
    fn keep_foreground_after(
        self: &Arc<Self>,
        pid: u32,
        before: (isize, u32),
        during: &'static str,
    ) -> Arc<std::sync::Mutex<Instant>> {
        let until = Arc::new(std::sync::Mutex::new(Instant::now() + NATIVE_FOCUS_GRACE));
        let deadline = until.clone();
        tokio::spawn(super::launch::keep_foreground(
            self.clone(),
            pid,
            before,
            during,
            move || Instant::now() < *deadline.lock().unwrap(),
        ));
        until
    }

    /// The window to give the foreground back to after a context menu: the one in front now, or,
    /// when the app is, the one in front at the click that opened the menu.
    fn foreground_before_menu(&self, pid: u32) -> (isize, u32) {
        let at_click = *self.foreground_at_click.lock().unwrap();
        let now = crate::proc::foreground();
        if now.1 == pid {
            at_click.unwrap_or(now)
        } else {
            now
        }
    }

    /// The window the command line is to give the foreground back to, while the app holds it.
    fn give_back_field(&self, pid: u32, before: (isize, u32)) -> Option<serde_json::Value> {
        (before.0 != 0 && before.1 != pid && crate::proc::foreground().1 == pid)
            .then(|| json!({ "hwnd": before.0, "pid": before.1, "app_pid": pid }))
    }

    pub(super) async fn native_choose_cmd(self: &Arc<Self>, path: &str) -> Outcome {
        let pid = self.native_pid()?;
        let parts = crate::native::parse_path(path);
        if parts.is_empty() {
            return Err(Failure::new(
                Kind::Usage,
                "native choose needs a path like `File > Open`",
            ));
        }
        let in_menu = blocking(move || Ok(crate::native::context_menu_open(pid))).await?;
        let before = if in_menu {
            self.foreground_before_menu(pid)
        } else {
            crate::proc::foreground()
        };
        let keeping =
            (!in_menu).then(|| self.keep_foreground_after(pid, before, "after a menu choice"));
        let chosen = tokio::task::spawn_blocking(move || crate::native::choose(pid, &parts))
            .await
            .map_err(|e| Failure::new(Kind::Error, format!("the native UI call failed: {e}")))?;
        match keeping {
            Some(k) => *k.lock().unwrap() = Instant::now() + NATIVE_FOCUS_GRACE,
            None if chosen.is_ok() => {
                *self.foreground_at_click.lock().unwrap() = None;
                self.keep_foreground_after(pid, before, "after a context menu choice");
            }
            None => {}
        }
        let window = chosen.map_err(|e| match e {
            crate::native::Refusal::Missing(m) => Failure::new(Kind::NotFound, m),
            crate::native::Refusal::Refused(m) => Failure::new(Kind::Error, m),
        })?;
        let mut f = fields! { "chosen" => path, "window" => window };
        if f["window"] == "context menu" {
            if let Some(g) = self.give_back_field(pid, before) {
                f.insert("give_back".into(), g);
            }
        }
        Ok(f)
    }

    /// `native tray`: clicks the app's tray icon. The window in front is kept for giving the
    /// foreground back after the menu a right click opens.
    pub(super) async fn native_tray_cmd(
        self: &Arc<Self>,
        index: usize,
        right: bool,
        double: bool,
    ) -> Outcome {
        let pid = self.native_pid()?;
        let front = crate::proc::foreground();
        if front.0 != 0 && front.1 != pid {
            *self.foreground_at_click.lock().unwrap() = Some(front);
        }
        let clicked = tokio::task::spawn_blocking(move || {
            crate::native::click_tray(pid, index, right, double)
        })
        .await
        .map_err(|e| Failure::new(Kind::Error, format!("the native UI call failed: {e}")))?;
        clicked.map_err(|e| match e {
            crate::native::Refusal::Missing(m) => Failure::new(Kind::NotFound, m),
            crate::native::Refusal::Refused(m) => Failure::new(Kind::Error, m),
        })?;
        let click = match (right, double) {
            (true, _) => "right",
            (_, true) => "double",
            _ => "left",
        };
        Ok(fields! { "tray_icon" => index, "click" => click })
    }

    /// `native dismiss`: closes the open context menu, and gives the foreground back.
    pub(super) async fn native_dismiss_cmd(self: &Arc<Self>) -> Outcome {
        let pid = self.native_pid()?;
        let before = self.foreground_before_menu(pid);
        let closed = tokio::task::spawn_blocking(move || crate::native::dismiss(pid))
            .await
            .map_err(|e| Failure::new(Kind::Error, format!("the native UI call failed: {e}")))?;
        if closed.is_ok() {
            *self.foreground_at_click.lock().unwrap() = None;
            self.keep_foreground_after(pid, before, "after a context menu closed");
        }
        let levels = closed.map_err(|e| match e {
            crate::native::Refusal::Missing(m) => Failure::new(Kind::NotFound, m),
            crate::native::Refusal::Refused(m) => Failure::new(Kind::Error, m),
        })?;
        let mut f = fields! { "dismissed" => "context menu", "levels" => levels };
        if let Some(g) = self.give_back_field(pid, before) {
            f.insert("give_back".into(), g);
        }
        Ok(f)
    }

    pub(super) async fn native_answer_cmd(
        self: &Arc<Self>,
        button: &str,
        dialog: Option<&str>,
    ) -> Outcome {
        let pid = self.native_pid()?;
        let keeping =
            self.keep_foreground_after(pid, crate::proc::foreground(), "after a dialog answer");
        let (button, dialog) = (button.to_string(), dialog.map(str::to_string));
        let answered = tokio::task::spawn_blocking(move || {
            crate::native::answer(pid, &button, dialog.as_deref())
        })
        .await
        .map_err(|e| Failure::new(Kind::Error, format!("the native UI call failed: {e}")))?;
        *keeping.lock().unwrap() = Instant::now() + NATIVE_FOCUS_GRACE;
        let answered = answered.map_err(|e| match e {
            crate::native::Refusal::Missing(m) => Failure::new(Kind::NotFound, m),
            crate::native::Refusal::Refused(m) => Failure::new(Kind::Error, m),
        })?;
        Ok(fields! {
            "dialog" => answered.dialog,
            "pressed" => answered.pressed,
            "closed" => answered.closed,
        })
    }
}

/// Runs a blocking Win32 or UI Automation call off the async runtime.
pub(super) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, Failure> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Failure::new(Kind::Error, format!("the native UI call failed: {e}")))?
        .map_err(Failure::from)
}

/// How long `window move` and `window resize` wait for the page to follow its window.
const PLACE_SETTLE: Duration = Duration::from_secs(3);

impl State {
    /// Where the current page is on the screen and how big, as it reports itself.
    async fn page_box(&self, cdp: &crate::cdp::Cdp) -> Result<crate::window::PageBox, Failure> {
        let v = super::commands::evaluate(
            cdp,
            "[screenX, screenY, innerWidth, innerHeight, devicePixelRatio]",
        )
        .await?;
        let n = |i: usize| {
            v[i].as_f64().ok_or_else(|| {
                Failure::new(
                    Kind::Error,
                    format!("the page reported no place or size: {v}"),
                )
            })
        };
        Ok(crate::window::PageBox {
            x: n(0)?,
            y: n(1)?,
            width: n(2)?,
            height: n(3)?,
            ratio: n(4)?,
        })
    }

    /// The app's window that holds the current page.
    async fn page_window(&self, pid: u32, page: crate::window::PageBox) -> Result<isize, Failure> {
        use crate::window::Unplaced;
        tokio::task::spawn_blocking(move || crate::window::page_window(pid, page))
            .await
            .map_err(|e| Failure::new(Kind::Error, format!("the window call failed: {e}")))?
            .map_err(|e| match e {
                Unplaced::Minimized => Failure::new(
                    Kind::Error,
                    "no visible window of the app holds this page, and one of its windows is \
                     minimized; restore it first",
                ),
                Unplaced::NotFound(m) => Failure::new(Kind::NotFound, m),
            })
    }

    /// `window move` / `window resize`: the window holding the current page, placed without
    /// activating it, once the page has followed. With `viewport`, `size` is the page's.
    pub(super) async fn window_place_cmd(
        &self,
        at: Option<(f64, f64)>,
        size: Option<(f64, f64)>,
        viewport: bool,
    ) -> Outcome {
        let pid = self.app_pid.ok_or_else(|| {
            Failure::new(
                Kind::Unsupported,
                "window needs the app's process, which only a launched session knows",
            )
        })?;
        let (tid, cdp) = self.current()?;
        self.ensure_ready(&tid, &cdp).await?;
        let page = self.page_box(&cdp).await?;
        let hwnd = self.page_window(pid, page).await?;
        let size = match (size, viewport) {
            (Some(want), true) => {
                let placed = blocking(move || crate::window::placed(hwnd)).await?;
                let area = crate::window::area(hwnd)
                    .ok_or_else(|| Failure::new(Kind::Error, "the window holds no page now"))?;
                Some(crate::window::outer_for_viewport(
                    want, page.ratio, placed, area,
                ))
            }
            (size, _) => size,
        };
        blocking(move || crate::window::set(hwnd, at, size)).await?;
        let deadline = Instant::now() + PLACE_SETTLE;
        let page = loop {
            let page = self.page_box(&cdp).await?;
            let followed = crate::window::area(hwnd)
                .is_some_and(|a| crate::window::matching(page, &[a]).is_ok());
            if followed {
                break page;
            }
            if Instant::now() >= deadline {
                return Err(Failure::new(
                    Kind::Timeout,
                    "the window was placed, but the page did not follow it within 3 s",
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        let placed = blocking(move || crate::window::placed(hwnd)).await?;
        Ok(fields! {
            "rect" => json!({ "x": placed.x, "y": placed.y, "width": placed.width, "height": placed.height }),
            "scale" => placed.scale,
            "viewport" => json!({ "width": page.width, "height": page.height }),
            "device_pixel_ratio" => page.ratio,
        })
    }

    /// `capture --window`: the window holding the current page with the app's own windows over
    /// it, as a PNG.
    pub(super) async fn capture_window_cmd(&self, out: Option<&str>) -> Outcome {
        let pid = self.app_pid.ok_or_else(|| {
            Failure::new(
                Kind::Unsupported,
                "capture --window needs the app's process, which only a launched session knows",
            )
        })?;
        let (tid, cdp) = self.current()?;
        self.ensure_ready(&tid, &cdp).await?;
        let page = self.page_box(&cdp).await?;
        let hwnd = self.page_window(pid, page).await?;
        let path = self.capture_path(out);
        let (shot, png) = blocking(move || {
            let shot = crate::window::capture(pid, hwnd)?;
            let png = crate::window::png(&shot)?;
            Ok((shot, png))
        })
        .await?;
        std::fs::write(&path, &png).map_err(|e| {
            Failure::new(
                Kind::Error,
                format!("could not write {}: {e}", path.display()),
            )
        })?;
        let windows: Vec<_> = shot
            .windows
            .iter()
            .map(|d| {
                let (x, y, width, height) = d.rect;
                json!({ "title": d.title, "class": d.class,
                        "rect": { "x": x, "y": y, "width": width, "height": height } })
            })
            .collect();
        Ok(fields! {
            "path" => path.display().to_string(),
            "bytes" => png.len(),
            "width" => shot.width,
            "height" => shot.height,
            "windows" => windows,
        })
    }
}

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
    fn native_pid(&self) -> Result<u32, Failure> {
        self.app_pid.ok_or_else(|| {
            Failure::new(
                Kind::Unsupported,
                "native needs the app's process, which only a launched session knows",
            )
        })
    }

    pub(super) async fn native_list_cmd(&self) -> Outcome {
        let pid = self.native_pid()?;
        let (menus, dialogs) = blocking(move || {
            let menus = crate::native::menus(pid);
            crate::native::dialogs(pid).map(|d| (menus, d))
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
        for d in &dialogs {
            text.push_str(&format!("dialog \"{}\":\n", d.title));
            for line in &d.text {
                text.push_str(&format!("  text \"{line}\"\n"));
            }
            for b in &d.buttons {
                text.push_str(&format!("  button \"{b}\"\n"));
            }
        }
        Ok(fields! { "windows" => windows, "dialogs" => dialogs, "text" => text })
    }

    /// Gives the foreground back, for a while, whenever the app takes it in answer to a command.
    fn keep_foreground_after(self: &Arc<Self>, pid: u32, during: &'static str) {
        let before = crate::proc::foreground();
        let until = Instant::now() + NATIVE_FOCUS_GRACE;
        tokio::spawn(super::launch::keep_foreground(
            self.clone(),
            pid,
            before,
            during,
            move || Instant::now() < until,
        ));
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
        self.keep_foreground_after(pid, "after a menu choice");
        let chosen = tokio::task::spawn_blocking(move || crate::native::choose(pid, &parts))
            .await
            .map_err(|e| Failure::new(Kind::Error, format!("the native UI call failed: {e}")))?;
        let window = chosen.map_err(|e| match e {
            crate::native::NotChosen::Missing(m) => Failure::new(Kind::NotFound, m),
            crate::native::NotChosen::Refused(m) => Failure::new(Kind::Error, m),
        })?;
        Ok(fields! { "chosen" => path, "window" => window })
    }

    pub(super) async fn native_answer_cmd(
        self: &Arc<Self>,
        button: &str,
        dialog: Option<&str>,
    ) -> Outcome {
        let pid = self.native_pid()?;
        self.keep_foreground_after(pid, "after a dialog answer");
        let (button, dialog) = (button.to_string(), dialog.map(str::to_string));
        let answered = blocking(move || crate::native::answer(pid, &button, dialog.as_deref()))
            .await
            .map_err(|f| Failure::new(Kind::NotFound, f.message))?;
        Ok(fields! {
            "dialog" => answered.dialog,
            "pressed" => answered.pressed,
            "closed" => answered.closed,
        })
    }
}

/// Runs a blocking Win32 or UI Automation call off the async runtime.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, Failure> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Failure::new(Kind::Error, format!("the native UI call failed: {e}")))?
        .map_err(Failure::from)
}

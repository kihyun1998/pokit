//! Input on `--route os`: `SendInput` into the app, which must be in front with the page that
//! takes the input.

use super::commands::{evaluate, ClickHow};
use super::State;
use crate::cdp::Cdp;
use crate::chord;
use crate::fields;
use crate::os_input::OsError;
use crate::output::{Failure, Kind, Outcome};
use std::time::Instant;

fn failure(e: OsError) -> Failure {
    match e {
        OsError::NotFront(m) => Failure::new(Kind::GuardRefused, m),
        OsError::Failed(m) => Failure::new(Kind::Error, m),
    }
}

/// Runs an OS input call off the async runtime.
async fn os<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, OsError> + Send + 'static,
) -> Result<T, Failure> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Failure::new(Kind::Error, format!("the OS input call failed: {e}")))?
        .map_err(failure)
}

impl State {
    /// Refuses OS input before anything happens on the page, unless the app is in front and the
    /// current page has the input focus: another of the app's windows, or a dialog of its own,
    /// in front would take the input instead.
    pub(super) async fn refuse_unless_front(&self) -> Result<u32, Failure> {
        let pid = self.os_pid()?;
        if !crate::os_input::is_frontmost(pid) {
            return Err(failure(OsError::NotFront(
                "OS input needs the app in front; run `window activate` first. Nothing was sent"
                    .into(),
            )));
        }
        let (_, cdp) = self.current()?;
        if evaluate(&cdp, "document.hasFocus()").await? != true {
            return Err(failure(OsError::NotFront(
                "the app is in front, but not with this page: another of its windows or a dialog \
                 has the focus. Nothing was sent"
                    .into(),
            )));
        }
        Ok(pid)
    }

    pub(super) async fn os_key_cmd(
        &self,
        chord: &str,
        into: Option<&str>,
        require_focus: Option<&str>,
    ) -> Outcome {
        let press = chord::parse_chord(chord).map_err(|e| Failure::new(Kind::Error, e))?;
        let pid = self.refuse_unless_front().await?;
        self.focus_for_input(into, require_focus).await?;
        let (code, modifiers) = (press.code.to_string(), press.modifiers);
        let sent = Instant::now();
        os(move || crate::os_input::key(pid, &code, modifiers)).await?;
        let f = fields! {
            "code" => press.code,
            "modifiers" => press.modifiers,
            "route" => "os",
        };
        Ok(self.stamp(f, &[sent]))
    }

    pub(super) async fn os_type_cmd(
        &self,
        text: &str,
        into: Option<&str>,
        require_focus: Option<&str>,
    ) -> Outcome {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let pid = self.refuse_unless_front().await?;
        self.focus_for_input(into, require_focus).await?;
        let chars = text.chars().count();
        let sent = Instant::now();
        os(move || crate::os_input::text(pid, &text)).await?;
        let f = fields! { "typed_chars" => chars, "route" => "os" };
        Ok(self.stamp(f, &[sent]))
    }

    /// Clicks at a page point; the caller has already refused it unless the app was in front.
    pub(super) async fn os_click(&self, cdp: &Cdp, x: f64, y: f64, how: &ClickHow) -> Outcome {
        let pid = self.os_pid()?;
        let ratio = evaluate(cdp, "devicePixelRatio")
            .await?
            .as_f64()
            .unwrap_or(1.0);
        let click = crate::os_input::Click {
            right: how.right,
            count: if how.double { 2 } else { 1 },
            hover: how.hover,
        };
        let sent = Instant::now();
        let at = os(move || crate::os_input::click(pid, (x, y), ratio, click)).await?;
        let action = match (how.hover, how.right, how.double) {
            (true, _, _) => "hover",
            (_, true, _) => "right_click",
            (_, _, true) => "double_click",
            _ => "click",
        };
        let f = fields! {
            "action" => action,
            "x" => x,
            "y" => y,
            "screen" => serde_json::json!({ "x": at.0, "y": at.1 }),
            "device_pixel_ratio" => ratio,
            "route" => "os",
        };
        Ok(self.stamp(f, &[sent]))
    }
}

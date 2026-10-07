//! Input on `--route os`: `SendInput` into the app, which must be in front.

use super::commands::{evaluate, ClickHow};
use super::native::blocking;
use super::State;
use crate::cdp::Cdp;
use crate::chord;
use crate::fields;
use crate::output::{Failure, Kind, Outcome};
use std::time::Instant;

/// OS input refused because the app is not in front: the guard code, and nothing sent.
fn guarded(message: String) -> Failure {
    if message.contains("needs the app in front") {
        Failure::new(Kind::GuardRefused, message)
    } else {
        Failure::new(Kind::Error, message)
    }
}

/// Refuses OS input before anything happens on the page, unless the app is in front.
fn refuse_unless_front(pid: u32) -> Result<(), Failure> {
    if crate::os_input::is_frontmost(pid) {
        return Ok(());
    }
    Err(guarded(
        "OS input needs the app in front; run `window activate` first. Nothing was sent".into(),
    ))
}

impl State {
    pub(super) async fn os_key_cmd(
        &self,
        chord: &str,
        into: Option<&str>,
        require_focus: Option<&str>,
    ) -> Outcome {
        let press = chord::parse_chord(chord).map_err(|e| Failure::new(Kind::Error, e))?;
        let pid = self.os_pid()?;
        refuse_unless_front(pid)?;
        self.focus_for_input(into, require_focus).await?;
        let (vk, modifiers) = (press.vk as u16, press.modifiers);
        let sent = Instant::now();
        blocking(move || crate::os_input::key(pid, vk, modifiers))
            .await
            .map_err(|f| guarded(f.message))?;
        let f = fields! {
            "key" => press.key,
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
        let pid = self.os_pid()?;
        refuse_unless_front(pid)?;
        self.focus_for_input(into, require_focus).await?;
        let chars = text.chars().count();
        let sent = Instant::now();
        blocking(move || crate::os_input::text(pid, &text))
            .await
            .map_err(|f| guarded(f.message))?;
        let f = fields! { "typed_chars" => chars, "route" => "os" };
        Ok(self.stamp(f, &[sent]))
    }

    pub(super) async fn os_click(&self, cdp: &Cdp, x: f64, y: f64, how: &ClickHow) -> Outcome {
        let pid = self.os_pid()?;
        refuse_unless_front(pid)?;
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
        let at = blocking(move || crate::os_input::click(pid, (x, y), ratio, click))
            .await
            .map_err(|f| guarded(f.message))?;
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

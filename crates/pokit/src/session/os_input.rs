//! Input on `--route os`: `SendInput` into the app, which must be in front with the page that
//! takes the input.

use super::commands::{check_delivery, evaluate, ClickHow, NOTE_KEYDOWNS, OS_KEYS_SETTLE_MS};
use super::State;
use crate::cdp::Cdp;
use crate::chord;
use crate::fields;
use crate::os_input::{OsError, PageWindow};
use crate::output::{Failure, Kind, Outcome};
use std::time::Instant;

fn failure(e: OsError) -> Failure {
    match e {
        OsError::NotFront(m) => Failure::new(Kind::GuardRefused, m),
        OsError::Failed(m) => Failure::new(Kind::Error, m),
    }
}

/// Counts, in `window.__pokitDrags`, the trusted HTML5 `dragstart`, `drag` and `dragend` events
/// the page raises, and each `dragstart` the page cancelled; replaces a previous count.
const NOTE_DRAGS: &str = "const old = window.__pokitDrags; \
    if (old) for (const [t, h] of old.hs) window.removeEventListener(t, h, true); \
    const c = { dragstart: 0, cancelled: 0, drag: 0, dragend: 0, hs: [] }; \
    for (const t of ['dragstart', 'drag', 'dragend']) { \
      const h = (e) => { if (!e.isTrusted) return; c[t] += 1; \
        if (t === 'dragstart') setTimeout(() => { if (e.defaultPrevented) c.cancelled += 1; }, 0); }; \
      window.addEventListener(t, h, true); c.hs.push([t, h]); } \
    window.__pokitDrags = c; true";

/// How long the count waits for a drag the page started to end once the button is up.
const DRAG_SETTLE_MS: u64 = 1000;

/// Reads and removes the count `NOTE_DRAGS` started, once a drag the page started has ended or
/// `DRAG_SETTLE_MS` has passed: `{dragstart, cancelled, drag, dragend}`, or `None` when the
/// count went with its document or the page could not be read.
async fn read_drags(cdp: &Cdp) -> Option<serde_json::Value> {
    let read = format!(
        "new Promise((resolve) => {{ const c = window.__pokitDrags; \
         if (!c) {{ resolve(null); return; }} \
         const until = performance.now() + {DRAG_SETTLE_MS}; \
         const done = () => {{ for (const [t, h] of c.hs) window.removeEventListener(t, h, true); \
           delete window.__pokitDrags; \
           resolve({{ dragstart: c.dragstart, cancelled: c.cancelled, drag: c.drag, dragend: c.dragend }}); }}; \
         const poll = () => (c.dragstart === c.cancelled || c.dragend > 0 || performance.now() >= until) \
           ? setTimeout(done, 0) : setTimeout(poll, 5); \
         setTimeout(poll, 0); }})"
    );
    evaluate(cdp, &read).await.ok().filter(|v| !v.is_null())
}

/// The result's `drag_events` and `delivery` for an OS drag, or not delivered when the page
/// started an HTML5 drag, dropped inside it, that never reached it.
fn drag_delivery(
    drags: Option<serde_json::Value>,
    drops_inside: bool,
) -> Result<crate::output::Fields, Failure> {
    let Some(v) = drags else {
        return Ok(fields! {
            "drag_events" => serde_json::Value::Null,
            "delivery" => "unverified: the page changed while dragging",
        });
    };
    let n = |k: &str| v[k].as_u64().unwrap_or(0);
    if n("dragstart") <= n("cancelled") {
        return Ok(fields! { "drag_events" => serde_json::Value::Null });
    }
    let events = serde_json::json!({
        "dragstart": n("dragstart"),
        "drag": n("drag"),
        "dragend": n("dragend"),
    });
    if n("dragend") == 0 {
        return Ok(fields! {
            "drag_events" => events,
            "delivery" => format!("unverified: the page's drag had not ended {DRAG_SETTLE_MS} ms after the button was let go"),
        });
    }
    if drops_inside && n("drag") == 0 {
        return Err(Failure::new(
            Kind::NotDelivered,
            "the page started an HTML5 drag and saw none of it: no `drag` event came between its \
             dragstart and dragend, so nothing was dropped. Either the window's drop target \
             ignores drags that carry no files (Tauri's `dragDropEnabled`, on by default, does; \
             turning it off loses the app's own file-drop events), or the path was too short \
             (raise --steps). `--route cdp` reaches HTML5 drag and drop either way",
        )
        .with("drag_events", events));
    }
    Ok(fields! { "drag_events" => events, "delivery" => "confirmed" })
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
    /// Refuses OS input before anything happens on the page, unless the window that holds the
    /// current page is the foreground window and the page has the input focus: another of the
    /// app's windows, or a dialog of its own, in front would take the input instead. The page's
    /// window, for the input to be checked against again as it is sent.
    pub(super) async fn refuse_unless_front(&self) -> Result<PageWindow, Failure> {
        let pid = self.os_pid()?;
        if !crate::os_input::is_frontmost(pid) {
            return Err(failure(OsError::NotFront(
                "OS input needs the app in front; run `window activate` first. Nothing was sent"
                    .into(),
            )));
        }
        let hwnd = self.current_page_window(pid).await.map_err(|why| {
            failure(OsError::NotFront(format!(
                "the window that holds this page could not be found ({}). Nothing was sent",
                why.message
            )))
        })?;
        let page = PageWindow { pid, hwnd };
        os(move || crate::os_input::front_page(page).map(|_| ())).await?;
        let (_, cdp) = self.current()?;
        if evaluate(&cdp, "document.hasFocus()").await? != true {
            return Err(failure(OsError::NotFront(
                "the app is in front, but not with this page: another of its windows or a dialog \
                 has the focus. Nothing was sent"
                    .into(),
            )));
        }
        Ok(page)
    }

    pub(super) async fn os_key_cmd(
        &self,
        chord: &str,
        into: Option<&str>,
        require_focus: Option<&str>,
    ) -> Outcome {
        let press = chord::parse_chord(chord).map_err(|e| Failure::new(Kind::Error, e))?;
        let page = self.refuse_unless_front().await?;
        let cdp = self.focus_then(into, require_focus, NOTE_KEYDOWNS).await?;
        let (code, modifiers) = (press.code.to_string(), press.modifiers);
        let downs = press.down_codes();
        let sent = Instant::now();
        let pressed = os(move || crate::os_input::key(page, &code, modifiers)).await;
        let delivery = check_delivery(&cdp, &downs, pressed.is_ok(), OS_KEYS_SETTLE_MS).await;
        pressed?;
        let mut f = fields! {
            "code" => press.code,
            "modifiers" => press.modifiers,
            "route" => "os",
        };
        f.extend(delivery?);
        Ok(self.stamp(f, &[sent]))
    }

    pub(super) async fn os_type_cmd(
        &self,
        text: &str,
        into: Option<&str>,
        require_focus: Option<&str>,
    ) -> Outcome {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let page = self.refuse_unless_front().await?;
        let cdp = self.focus_then(into, require_focus, NOTE_KEYDOWNS).await?;
        let chars = text.chars().count();
        let downs = crate::os_input::text_down_codes(&text);
        let sent = Instant::now();
        let typed = os(move || crate::os_input::text(page, &text)).await;
        let delivery = check_delivery(&cdp, &downs, typed.is_ok(), OS_KEYS_SETTLE_MS).await;
        typed?;
        let mut f = fields! { "typed_chars" => chars, "route" => "os" };
        f.extend(delivery?);
        Ok(self.stamp(f, &[sent]))
    }

    /// Clicks at a page point in `page`'s window, which the caller has already checked was in
    /// front.
    pub(super) async fn os_click(
        &self,
        page: PageWindow,
        cdp: &Cdp,
        x: f64,
        y: f64,
        how: &ClickHow,
    ) -> Outcome {
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
        let at = os(move || crate::os_input::click(page, (x, y), ratio, click)).await?;
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

    /// The OS side of `hold`: the modifiers down, `count` key-downs on schedule, then the key and
    /// the modifiers up; when each key-down and the key-up were sent. Whatever goes wrong, every
    /// key pressed is released again, whichever app is in front by then.
    pub(super) async fn os_hold(
        &self,
        page: PageWindow,
        press: &chord::KeyPress,
        count: u32,
        interval: std::time::Duration,
    ) -> Result<Vec<Instant>, Failure> {
        let strokes = crate::os_input::chord_strokes(press.code, press.modifiers);
        let held = (strokes.len() - 2) / 2;
        let modifiers_down = strokes[..held].to_vec();
        let key_down = strokes[held].clone();
        let release: Vec<String> = strokes[held + 1..].iter().map(|(c, _)| c.clone()).collect();
        *self.keys_down.lock().unwrap() = release.clone();
        let mut sent = Vec::with_capacity(count as usize + 1);
        let pressed = async {
            if !modifiers_down.is_empty() {
                os(move || crate::os_input::strokes(page, &modifiers_down)).await?;
            }
            let start = tokio::time::Instant::now();
            for n in 0..count {
                tokio::time::sleep_until(start + interval * n).await;
                sent.push(Instant::now());
                let down = vec![key_down.clone()];
                os(move || crate::os_input::strokes(page, &down)).await?;
            }
            tokio::time::sleep_until(start + interval * count).await;
            Ok::<_, Failure>(())
        }
        .await;
        sent.push(Instant::now());
        let up = release.clone();
        let _ = tokio::task::spawn_blocking(move || crate::os_input::release(&up)).await;
        self.keys_down.lock().unwrap().clear();
        // `sent` also holds the key-down that was refused and the release.
        let downs = sent.len().saturating_sub(2);
        pressed.map(|_| sent).map_err(|mut f| {
            if downs > 0 {
                let why = f.message.trim_end_matches(". Nothing was sent");
                let why = why.trim_end_matches("; nothing was sent");
                f.message = format!(
                    "{why}, after {downs} of {count} key-downs; every key pokit pressed was released"
                );
            }
            f
        })
    }

    /// Drags through the OS along a page path from `page`'s window, which the caller has already
    /// checked was in front. The button is held until the end, and released when the session
    /// ends. The HTML5 drag events the page raised are counted: a drag the page started and
    /// dropped inside it (`drops_inside`) with no `drag` event between its start and end never
    /// reached the page, and fails as not delivered.
    pub(super) async fn os_drag(
        &self,
        page: PageWindow,
        cdp: &Cdp,
        path: Vec<(f64, f64)>,
        drops_inside: bool,
    ) -> Outcome {
        evaluate(cdp, NOTE_DRAGS).await?;
        let ratio = evaluate(cdp, "devicePixelRatio")
            .await?
            .as_f64()
            .unwrap_or(1.0);
        *self.keys_down.lock().unwrap() = vec![crate::os_input::LEFT_BUTTON.to_string()];
        let sent = Instant::now();
        let step = super::commands::DRAG_STEP;
        let dropped = os(move || crate::os_input::drag(page, &path, ratio, step)).await;
        self.keys_down.lock().unwrap().clear();
        let drags = read_drags(cdp).await;
        let at = dropped?;
        let mut f = fields! {
            "screen" => serde_json::json!({ "x": at.0, "y": at.1 }),
            "device_pixel_ratio" => ratio,
        };
        f.extend(drag_delivery(drags, drops_inside)?);
        Ok(self.stamp(f, &[sent]))
    }

    /// Turns the wheel through the OS over a page point in `page`'s window, which the caller has
    /// already checked was in front.
    pub(super) async fn os_wheel(
        &self,
        page: PageWindow,
        cdp: &Cdp,
        x: f64,
        y: f64,
        notches: i32,
    ) -> Outcome {
        let ratio = evaluate(cdp, "devicePixelRatio")
            .await?
            .as_f64()
            .unwrap_or(1.0);
        let sent = Instant::now();
        let at = os(move || crate::os_input::wheel(page, (x, y), ratio, notches)).await?;
        let f = fields! {
            "notches" => notches,
            "x" => x,
            "y" => y,
            "screen" => serde_json::json!({ "x": at.0, "y": at.1 }),
            "route" => "os",
        };
        Ok(self.stamp(f, &[sent]))
    }
}

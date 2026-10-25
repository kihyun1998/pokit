//! Input on the plugin route (macOS): the app's plugin delivers key events to its own window, text
//! the US layout has no key for to the webview as an input method commits it, and mouse events to
//! the view under the point.

use super::commands::{
    delivery_js, delivery_verdict, focus_holds_js, focus_verdict, invalid_selector, no_match,
    refs_unsupported, target_verdict, ClickAt, ClickHow, EVALUATE_TIMEOUT, NOTE_KEYDOWNS,
    PLUGIN_KEYS_SETTLE_MS,
};
use super::page::{Page, PluginPage};
use super::State;
use crate::chord::{self, KeyPress};
use crate::fields;
use crate::output::{Failure, Fields, Kind, Outcome};
use crate::snapshot;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

impl State {
    pub(super) async fn plugin_type_cmd(
        &self,
        text: &str,
        into: Option<&str>,
        require_focus: Option<&str>,
    ) -> Outcome {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let page = self.plugin_focus(into, require_focus).await?;
        let (mut keyed, mut inserted) = (0, 0);
        let mut downs = Vec::new();
        let mut sent = Vec::new();
        let mut typed = Ok(());
        for c in text.chars() {
            let press = if c == '\n' {
                chord::parse_chord("Enter").ok()
            } else {
                chord::key_for_char(c)
            };
            typed = match &press {
                Some(p) => send_press(page, p, &mut sent).await,
                None => {
                    keys_handled(page, downs.len()).await;
                    let insert = json!([{ "type": "insert", "text": c.to_string() }]);
                    sent.push(Instant::now());
                    page.plugin.input(&page.webview, insert).await.map(drop)
                }
            };
            if typed.is_err() {
                break;
            }
            match press {
                Some(p) => {
                    downs.extend(p.mac_down_codes());
                    keyed += 1;
                }
                None => inserted += 1,
            }
        }
        let delivery = delivery(page, &downs, typed.is_ok()).await;
        typed?;
        let mut f = fields! {
            "typed_chars" => keyed + inserted,
            "key_events" => keyed,
            "inserted_chars" => inserted,
            "composed_chars" => 0,
        };
        f.extend(delivery?);
        Ok(self.stamp(f, &sent))
    }

    pub(super) async fn plugin_key_cmd(
        &self,
        chord: &str,
        into: Option<&str>,
        require_focus: Option<&str>,
    ) -> Outcome {
        let press = chord::parse_chord(chord).map_err(|e| Failure::new(Kind::Error, e))?;
        let page = self.plugin_focus(into, require_focus).await?;
        let mut sent = Vec::new();
        let pressed = send_press(page, &press, &mut sent).await;
        let downs = if pressed.is_ok() {
            press.mac_down_codes()
        } else {
            Vec::new()
        };
        let delivery = delivery(page, &downs, pressed.is_ok()).await;
        pressed?;
        let mut f =
            fields! { "key" => press.key, "code" => press.code, "modifiers" => press.modifiers };
        f.extend(delivery?);
        Ok(self.stamp(f, &sent))
    }

    pub(super) async fn plugin_click_cmd(
        &self,
        at: Option<ClickAt<'_>>,
        how: ClickHow,
        require_focus: Option<&str>,
    ) -> Outcome {
        let p = self
            .plugin
            .as_ref()
            .ok_or_else(|| Failure::new(Kind::Error, "this session has no plugin"))?;
        let page = Page::Plugin(p);
        let right_click = how.right && !how.hover;
        let note = if right_click {
            format!("{NOTE_MOUSE} {NOTE_CONTEXTMENU}")
        } else {
            NOTE_MOUSE.to_string()
        };
        let (x, y) = match at {
            Some(ClickAt::Element(t)) if snapshot::is_ref(t) => return Err(refs_unsupported(t)),
            Some(ClickAt::Element(t)) => {
                self.plugin_guard_focus(&page, require_focus).await?;
                let js = format!(
                    "(() => {{
                        let el;
                        try {{ el = document.querySelector({t}); }}
                        catch (e) {{ return {{ invalid: String(e) }}; }}
                        if (!el) return {{}};
                        el.scrollIntoView({{ block: 'center', inline: 'center' }});
                        const r = el.getBoundingClientRect();
                        {note}
                        return {{ left: r.left, top: r.top, width: r.width, height: r.height }};
                    }})()",
                    t = json!(t),
                );
                let r = page.eval(&js, EVALUATE_TIMEOUT).await?;
                if let Some(e) = r["invalid"].as_str() {
                    return Err(invalid_selector(t, e));
                }
                let (Some(left), Some(top)) = (r["left"].as_f64(), r["top"].as_f64()) else {
                    return Err(no_match(t));
                };
                let (width, height) = (
                    r["width"].as_f64().unwrap_or(0.0),
                    r["height"].as_f64().unwrap_or(0.0),
                );
                if width == 0.0 && height == 0.0 {
                    return Err(Failure::new(
                        Kind::NotFound,
                        format!("`{t}` has no size on screen"),
                    ));
                }
                (left + width / 2.0, top + height / 2.0)
            }
            Some(ClickAt::Point(x, y)) => {
                self.plugin_guard_focus(&page, require_focus).await?;
                page.eval(
                    &format!("(() => {{ {note} return true; }})()"),
                    EVALUATE_TIMEOUT,
                )
                .await?;
                (x, y)
            }
            None => {
                return Err(Failure::new(
                    Kind::Error,
                    "click needs a ref, a selector, or --x and --y",
                ))
            }
        };
        let mouse = |kind: &str, clicks: i64| {
            let button = if how.right { "right" } else { "left" };
            json!([{ "type": kind, "x": x, "y": y, "button": button, "clicks": clicks }])
        };
        let mut sent = Vec::new();
        let mut watch = None;
        let pressed = async {
            sent.push(Instant::now());
            p.plugin.input(&p.webview, mouse("mousemove", 0)).await?;
            if how.hover {
                return Ok("hover");
            }
            let count = if how.double { 2 } else { 1 };
            for n in 1..=count {
                sent.push(Instant::now());
                let reply = p.plugin.input(&p.webview, mouse("mousedown", n)).await?;
                watch = watch.or(reply["menu_watch"].as_u64());
                sent.push(Instant::now());
                p.plugin.input(&p.webview, mouse("mouseup", n)).await?;
            }
            Ok::<_, Failure>(match (how.right, count) {
                (true, _) => "right_click",
                (_, 2) => "double_click",
                _ => "click",
            })
        }
        .await;
        if let Ok(action) = pressed {
            mouse_handled(p, action, if how.double { 2 } else { 1 }).await;
        }
        let menu_closed = match right_click {
            true => Some(close_menu(p, watch).await),
            false => None,
        };
        let mut f = fields! { "action" => pressed?, "x" => x, "y" => y };
        if let Some(closed) = menu_closed {
            f.insert("menu_closed".into(), json!(closed?));
        }
        Ok(self.stamp(f, &sent))
    }

    /// Focuses `into` when given, checking the focus guard before anything changes; without
    /// `into`, checks that the current focus holds. Then notes the page's keydowns
    /// (`NOTE_KEYDOWNS`). Returns the plugin's page.
    async fn plugin_focus(
        &self,
        into: Option<&str>,
        require_focus: Option<&str>,
    ) -> Result<&PluginPage, Failure> {
        let p = self
            .plugin
            .as_ref()
            .ok_or_else(|| Failure::new(Kind::Error, "this session has no plugin"))?;
        let page = Page::Plugin(p);
        match into {
            Some(into) if snapshot::is_ref(into) => return Err(refs_unsupported(into)),
            Some(into) => {
                let js = format!(
                    "(() => {{
                        let el;
                        try {{ el = document.querySelector({into}); }}
                        catch (e) {{ return {{ invalid: String(e) }}; }}
                        if (!el) return {{}};
                        const sel = {sel};
                        if (sel !== null) {{
                            const r = document.querySelector(sel);
                            if (!(r && (r === el || r.contains(el))))
                                return {{ guard: {{ matched: !!r, inside: false }} }};
                        }}
                        el.focus();
                        {NOTE_KEYDOWNS}
                        return {{ focused: true }};
                    }})()",
                    into = json!(into),
                    sel = json!(require_focus),
                );
                let v = page.eval(&js, EVALUATE_TIMEOUT).await?;
                if let Some(e) = v["invalid"].as_str() {
                    return Err(invalid_selector(into, e));
                }
                if let (Some(guard), Some(sel)) = (v.get("guard"), require_focus) {
                    target_verdict(guard, into, sel)?;
                }
                if v["focused"] != true {
                    return Err(no_match(into));
                }
                self.plugin_guard_focus(&page, require_focus).await?;
            }
            None => {
                self.plugin_guard_focus(&page, require_focus).await?;
                let js = format!("(() => {{ {NOTE_KEYDOWNS} return true; }})()");
                page.eval(&js, EVALUATE_TIMEOUT).await?;
            }
        }
        Ok(p)
    }

    async fn plugin_guard_focus(
        &self,
        page: &Page<'_>,
        require_focus: Option<&str>,
    ) -> Result<(), Failure> {
        let Some(sel) = require_focus else {
            return Ok(());
        };
        focus_verdict(
            &page.eval(&focus_holds_js(sel), EVALUATE_TIMEOUT).await?,
            sel,
        )
    }
}

/// Delivers `p`'s events one at a time, noting when each was sent.
async fn send_press(
    page: &PluginPage,
    p: &KeyPress,
    sent: &mut Vec<Instant>,
) -> Result<(), Failure> {
    for event in p.mac_events() {
        sent.push(Instant::now());
        page.plugin
            .input(&page.webview, Value::Array(vec![event]))
            .await?;
    }
    Ok(())
}

/// Waits, up to `PLUGIN_KEYS_SETTLE_MS`, until the page has noted `n` keydowns.
async fn keys_handled(page: &PluginPage, n: usize) {
    if n == 0 {
        return;
    }
    let js = format!(
        "new Promise((resolve) => {{ const until = performance.now() + {PLUGIN_KEYS_SETTLE_MS}; \
         const poll = () => {{ const c = window.__pokitKeydowns; \
         (!c || c.codes.length >= {n} || performance.now() >= until) ? resolve(true) : setTimeout(poll, 2); }}; \
         poll(); }})"
    );
    let _ = Page::Plugin(page).eval(&js, EVALUATE_TIMEOUT).await;
}

/// The delivery check over the plugin: the plugin's events reach the page after the request
/// that sent them returned, so it waits up to `PLUGIN_KEYS_SETTLE_MS` for them.
async fn delivery(page: &PluginPage, downs: &[String], sent_all: bool) -> Result<Fields, Failure> {
    let read = Page::Plugin(page)
        .eval(&delivery_js(downs, PLUGIN_KEYS_SETTLE_MS), EVALUATE_TIMEOUT)
        .await;
    delivery_verdict(read, downs, sent_all)
}

/// Counts the trusted mouse events that end each kind of click, in a capture listener on
/// `window`, for `mouse_handled` to wait on.
const NOTE_MOUSE: &str = "const old = window.__pokitMouse; \
    if (old) for (const t of ['mousemove', 'mouseup', 'contextmenu']) window.removeEventListener(t, old.h, true); \
    const m = { mousemove: 0, mouseup: 0, contextmenu: 0 }; m.h = (e) => { if (e.isTrusted) m[e.type] += 1; }; \
    for (const t of ['mousemove', 'mouseup', 'contextmenu']) window.addEventListener(t, m.h, true); \
    window.__pokitMouse = m;";

/// Waits, up to `PLUGIN_KEYS_SETTLE_MS`, until the page has handled the events that end
/// `action`: the move of a hover, the `n`th release of a click, the `contextmenu` of a
/// right-click.
async fn mouse_handled(page: &PluginPage, action: &str, n: usize) {
    let (kind, n) = match action {
        "hover" => ("mousemove", 1),
        "right_click" => ("contextmenu", 1),
        _ => ("mouseup", n),
    };
    let js = format!(
        "new Promise((resolve) => {{ const until = performance.now() + {PLUGIN_KEYS_SETTLE_MS}; \
         const poll = () => {{ const m = window.__pokitMouse; \
         (!m || m.{kind} >= {n} || performance.now() >= until) ? resolve(true) : setTimeout(poll, 2); }}; \
         poll(); }})"
    );
    let _ = Page::Plugin(page).eval(&js, EVALUATE_TIMEOUT).await;
}

/// Notes the next trusted `contextmenu` on the page, to read whether the page prevented its menu.
const NOTE_CONTEXTMENU: &str = "window.__pokitContextmenu = null; \
    window.addEventListener('contextmenu', (e) => { if (e.isTrusted) window.__pokitContextmenu = e; }, { capture: true, once: true });";

/// Closes the menu WebKit opens for a right-click whose `contextmenu` the page did not prevent,
/// and says whether it did. Waits up to `PLUGIN_KEYS_SETTLE_MS` for the page's event, then as
/// long again for the menu.
async fn close_menu(page: &PluginPage, watch: Option<u64>) -> Result<bool, Failure> {
    let Some(watch) = watch else {
        return Ok(false);
    };
    let js = format!(
        "new Promise((resolve) => {{ const until = performance.now() + {PLUGIN_KEYS_SETTLE_MS}; \
         const poll = () => {{ const e = window.__pokitContextmenu; \
         e ? resolve({{ prevented: e.defaultPrevented }}) : performance.now() >= until ? resolve({{}}) : setTimeout(poll, 2); }}; \
         poll(); }})"
    );
    let seen = Page::Plugin(page).eval(&js, EVALUATE_TIMEOUT).await?;
    if seen["prevented"] != false {
        return Ok(false);
    }
    page.plugin
        .close_menu(watch, Duration::from_millis(PLUGIN_KEYS_SETTLE_MS))
        .await
}

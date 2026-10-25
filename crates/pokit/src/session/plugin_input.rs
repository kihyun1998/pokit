//! Input on the plugin route (macOS): the app's plugin delivers key events to its own window, and
//! text the US layout has no key for to the webview as an input method commits it.

use super::commands::{
    delivery_js, delivery_verdict, focus_holds_js, focus_verdict, invalid_selector, no_match,
    refs_unsupported, target_verdict, EVALUATE_TIMEOUT, NOTE_KEYDOWNS, PLUGIN_KEYS_SETTLE_MS,
};
use super::page::{Page, PluginPage};
use super::State;
use crate::chord::{self, KeyPress};
use crate::fields;
use crate::output::{Failure, Fields, Kind, Outcome};
use crate::snapshot;
use serde_json::{json, Value};
use std::time::Instant;

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
                    page.plugin.input(&page.webview, insert).await
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

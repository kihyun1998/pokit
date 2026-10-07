//! Answering one request: the command handlers, the focus guards, and the page helpers they share.

use super::record::screenshot_to;
use super::State;
use crate::cdp::Cdp;
use crate::chord::{self, KeyPress};
use crate::fields;
use crate::hangul;
use crate::home::Mode;
use crate::output::{self, Failure, Fields, Kind, Outcome};
use crate::redact::redact;
use crate::request::{Request, Route};
use crate::snapshot;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The object group every remote object pokit resolves belongs to, released after each command.
const OBJECT_GROUP: &str = "pokit";

impl State {
    pub(super) async fn handle(self: &Arc<Self>, request: Request) -> (i32, Value) {
        let kind = request.kind();
        if let Some(text) = request.secret() {
            let mut secrets = self.secrets.lock().unwrap();
            if !secrets.iter().any(|s| s == text) {
                secrets.push(text.to_string());
            }
        }
        let _one_at_a_time = if request.sends_os_input() {
            Some(self.os_input.lock().await)
        } else {
            None
        };
        let outcome = match &request {
            Request::Ping => Ok(Fields::new()),
            Request::Status => Ok(self.status()),
            Request::Close => {
                if self.config.mode == Mode::Attach {
                    self.remove_probes().await;
                }
                self.restore_clipboard().await;
                Ok(fields! { "closed" => self.config.mode == Mode::Launch })
            }
            Request::Targets { select } => self.targets_cmd(select.as_deref()).await,
            Request::Eval {
                expression,
                timeout_ms,
            } => self.eval_cmd(expression, *timeout_ms).await,
            Request::Snapshot => self.snapshot_cmd().await,
            Request::Read { target } => self.read_cmd(target).await,
            Request::Click {
                target,
                x,
                y,
                double,
                right,
                hover,
                require_focus,
                route,
            } => {
                let at = match (target, x, y) {
                    (Some(t), _, _) => Some(ClickAt::Element(t)),
                    (None, Some(x), Some(y)) => Some(ClickAt::Point(*x, *y)),
                    _ => None,
                };
                let how = ClickHow {
                    double: *double,
                    right: *right,
                    hover: *hover,
                    route: *route,
                };
                self.click_cmd(at, how, require_focus.as_deref()).await
            }
            Request::Type {
                text,
                into,
                require_focus,
                route,
                ..
            } => match route {
                Route::Cdp => {
                    self.type_cmd(text, into.as_deref(), require_focus.as_deref())
                        .await
                }
                #[cfg(windows)]
                Route::Os => {
                    self.os_type_cmd(text, into.as_deref(), require_focus.as_deref())
                        .await
                }
                #[cfg(not(windows))]
                Route::Os => Err(os_route_unsupported()),
            },
            Request::Key {
                chord,
                into,
                require_focus,
                route,
            } => match route {
                Route::Cdp => {
                    self.key_cmd(chord, into.as_deref(), require_focus.as_deref())
                        .await
                }
                #[cfg(windows)]
                Route::Os => {
                    self.os_key_cmd(chord, into.as_deref(), require_focus.as_deref())
                        .await
                }
                #[cfg(not(windows))]
                Route::Os => Err(os_route_unsupported()),
            },
            Request::Hold {
                chord,
                count,
                interval_ms,
                into,
                require_focus,
                route,
                compare,
            } => {
                let hold = super::measure::Hold {
                    chord,
                    count: *count,
                    interval_ms: *interval_ms,
                    into: into.as_deref(),
                    require_focus: require_focus.as_deref(),
                    route: *route,
                };
                if *compare {
                    self.hold_compare_cmd(hold).await
                } else {
                    self.hold_cmd(hold).await
                }
            }
            Request::Drag {
                from,
                to,
                to_x,
                to_y,
                via,
                steps,
                route,
            } => {
                let to = match (to, to_x, to_y) {
                    (Some(t), _, _) => Some(ClickAt::Element(t)),
                    (None, Some(x), Some(y)) => Some(ClickAt::Point(*x, *y)),
                    _ => None,
                };
                self.drag_cmd(from, to, via, *steps, *route).await
            }
            Request::Wheel {
                target,
                x,
                y,
                notches,
                route,
            } => {
                self.wheel_cmd(target.as_deref(), *x, *y, *notches, *route)
                    .await
            }
            Request::MeasureStart {
                watch,
                watch_attr,
                long_frame_ms,
                over_ms,
            } => {
                self.measure_start_cmd(
                    watch.as_deref(),
                    watch_attr.as_deref(),
                    *long_frame_ms,
                    *over_ms,
                )
                .await
            }
            #[cfg(windows)]
            Request::WindowActivate => self.window_activate_cmd().await,
            #[cfg(windows)]
            Request::WindowMove { x, y } => {
                self.window_place_cmd(Some((*x, *y)), None, false).await
            }
            #[cfg(windows)]
            Request::WindowResize {
                width,
                height,
                viewport,
            } => {
                self.window_place_cmd(None, Some((*width, *height)), *viewport)
                    .await
            }
            #[cfg(not(windows))]
            Request::WindowMove { .. } | Request::WindowResize { .. } => Err(Failure::new(
                Kind::Unsupported,
                "moving and resizing windows is not built on this platform yet",
            )),
            #[cfg(not(windows))]
            Request::WindowActivate => Err(Failure::new(
                Kind::Unsupported,
                "window activation is not built on this platform yet",
            )),
            #[cfg(windows)]
            Request::NativeList => self.native_list_cmd().await,
            #[cfg(windows)]
            Request::NativeChoose { path } => self.native_choose_cmd(path).await,
            #[cfg(windows)]
            Request::NativeAnswer { button, dialog } => {
                self.native_answer_cmd(button, dialog.as_deref()).await
            }
            #[cfg(windows)]
            Request::NativeDismiss => self.native_dismiss_cmd().await,
            #[cfg(windows)]
            Request::NativeTray {
                index,
                right,
                double,
            } => self.native_tray_cmd(*index, *right, *double).await,
            #[cfg(not(windows))]
            Request::NativeList
            | Request::NativeChoose { .. }
            | Request::NativeAnswer { .. }
            | Request::NativeDismiss
            | Request::NativeTray { .. } => Err(Failure::new(
                Kind::Unsupported,
                "native UI is not built on this platform yet",
            )),
            #[cfg(windows)]
            Request::ClipboardRead => self.clipboard_read_cmd().await,
            #[cfg(windows)]
            Request::ClipboardWrite { text, .. } => self.clipboard_write_cmd(text).await,
            #[cfg(not(windows))]
            Request::ClipboardRead | Request::ClipboardWrite { .. } => Err(Failure::new(
                Kind::Unsupported,
                "the clipboard is not built on this platform yet",
            )),
            Request::TraceStart => self.trace_start_cmd().await,
            Request::TraceStop => self.trace_stop_cmd().await,
            Request::ProfileStart { interval_us } => self.profile_start_cmd(*interval_us).await,
            Request::ProfileStop { top } => self.profile_stop_cmd(*top).await,
            Request::MeasureStop {
                quiet_ms,
                ceiling_ms,
            } => self.measure_stop_cmd(*quiet_ms, *ceiling_ms).await,
            Request::Wait {
                selector,
                text,
                expr,
                timeout_ms,
            } => {
                let condition = match (selector, text, expr) {
                    (Some(s), _, _) => Some(Condition::Selector(s)),
                    (None, Some(t), _) => Some(Condition::Text(t)),
                    (None, None, Some(e)) => Some(Condition::Expr(e)),
                    _ => None,
                };
                self.wait_cmd(condition, *timeout_ms).await
            }
            #[cfg(windows)]
            Request::Capture {
                out, window: true, ..
            } => self.capture_window_cmd(out.as_deref()).await,
            #[cfg(not(windows))]
            Request::Capture { window: true, .. } => Err(Failure::new(
                Kind::Unsupported,
                "capturing a window with its native UI is not built on this platform yet",
            )),
            Request::Capture { target, out, .. } => {
                self.capture_cmd(target.as_deref(), out.as_deref()).await
            }
            Request::Logs { since } => {
                let (entries, next) = self.logs_since(since.unwrap_or(0));
                Ok(fields! { "entries" => entries, "next" => next })
            }
            Request::Doctor => {
                crate::doctor::outcome(crate::doctor::checks(self.config.cdp_port).await)
            }
        };
        if kind.resolves_elements() {
            self.release_objects().await;
        }
        let screenshot = match &outcome {
            Err(_) if kind.screenshots_failure() && request.secret().is_none() => {
                self.failure_screenshot().await
            }
            _ => None,
        };
        let outcome = match (outcome, screenshot) {
            (Err(f), Some(path)) => Err(f.with("screenshot", path)),
            (o, _) => o,
        };
        let (code, mut out) = output::render(kind.name(), &outcome);
        let secrets = self.secrets.lock().unwrap().clone();
        redact(&mut out, &secrets);
        if kind.is_public() {
            let mut args = serde_json::to_value(&request)
                .ok()
                .and_then(|v| v.get("args").cloned())
                .unwrap_or_else(|| json!({}));
            if request.secret().is_some() {
                args["text"] = json!(crate::redact::MASK);
            }
            redact(&mut args, &secrets);
            let mut recorded = out.clone();
            if let Some(entries) = recorded
                .get("entries")
                .and_then(Value::as_array)
                .map(Vec::len)
            {
                recorded["entries"] = json!(format!("{entries} entries, not copied"));
            }
            self.record(kind.name(), &args, code, &recorded);
        }
        (code, out)
    }

    /// Releases every remote object pokit resolved during a command, on every page.
    async fn release_objects(&self) {
        let cdps: Vec<Arc<Cdp>> = self
            .targets
            .lock()
            .unwrap()
            .values()
            .map(|t| t.cdp.clone())
            .collect();
        for cdp in cdps {
            let _ = cdp
                .call(
                    "Runtime.releaseObjectGroup",
                    json!({ "objectGroup": OBJECT_GROUP }),
                )
                .await;
        }
    }

    async fn targets_cmd(&self, select: Option<&str>) -> Outcome {
        if let Ok(pages) = crate::devtools::pages(self.config.cdp_port).await {
            self.sync_targets(&pages).await;
        }
        if let Some(sel) = select {
            let order = self.order.lock().unwrap().clone();
            let found = {
                let targets = self.targets.lock().unwrap();
                let by_id = order.iter().find(|id| id.as_str() == sel);
                let by_index = sel.parse::<usize>().ok().and_then(|i| order.get(i));
                let by_text = order.iter().find(|id| {
                    targets
                        .get(*id)
                        .map(|t| t.url.contains(sel) || t.title.contains(sel))
                        .unwrap_or(false)
                });
                by_id.or(by_index).or(by_text).cloned()
            };
            match found {
                Some(id) => *self.current.lock().unwrap() = id,
                None => {
                    return Err(Failure::new(
                        Kind::NotFound,
                        format!("no target matches `{sel}`"),
                    ))
                }
            }
        }
        let (main, current) = (
            self.main.lock().unwrap().clone(),
            self.current.lock().unwrap().clone(),
        );
        let order = self.order.lock().unwrap().clone();
        let targets = self.targets.lock().unwrap();
        let list: Vec<Value> = order
            .iter()
            .enumerate()
            .filter_map(|(i, id)| {
                targets.get(id).map(|t| {
                    json!({ "index": i, "id": id, "title": t.title, "url": t.url, "main": *id == main, "current": *id == current })
                })
            })
            .collect();
        Ok(fields! { "targets" => list })
    }

    async fn eval_cmd(&self, expression: &str, timeout_ms: u64) -> Outcome {
        let (_, cdp) = self.current()?;
        let value = evaluate_within(&cdp, expression, Duration::from_millis(timeout_ms)).await?;
        Ok(fields! { "value" => value })
    }

    async fn snapshot_cmd(&self) -> Outcome {
        let (target, cdp) = self.current()?;
        cdp.call("Accessibility.enable", json!({})).await?;
        let tree = cdp.call("Accessibility.getFullAXTree", json!({})).await;
        let _ = cdp.call("Accessibility.disable", json!({})).await;
        let tree = tree?;
        let mut refs = self.refs.lock().unwrap();
        let snap = snapshot::render(
            tree["nodes"].as_array().map(Vec::as_slice).unwrap_or(&[]),
            refs.next,
        );
        refs.next = snap.next_ref;
        refs.map = snap
            .refs
            .iter()
            .map(|(r, node)| (r.clone(), (target.clone(), *node)))
            .collect();
        Ok(fields! { "snapshot" => snap.text, "refs" => snap.refs.len() })
    }

    /// Resolves a ref (`e12`) or CSS selector to a remote object: (target id, its page, object id).
    async fn resolve(&self, target: &str) -> Result<(String, Arc<Cdp>, String), Failure> {
        if snapshot::is_ref(target) {
            let entry = self.refs.lock().unwrap().map.get(target).cloned();
            let Some((tid, node)) = entry else {
                return Err(Failure::new(
                    Kind::StaleRef,
                    format!("ref {target} is not in the latest snapshot; take a new snapshot"),
                )
                .with("ref", target));
            };
            let cdp = self.cdp(&tid)?;
            let obj = cdp
                .call(
                    "DOM.resolveNode",
                    json!({ "backendNodeId": node, "objectGroup": OBJECT_GROUP }),
                )
                .await
                .map_err(|_| {
                    Failure::new(
                        Kind::StaleRef,
                        format!("the element for ref {target} is no longer in the page"),
                    )
                    .with("ref", target)
                })?;
            let id = obj["object"]["objectId"].as_str().unwrap_or("").to_string();
            return Ok((tid, cdp, id));
        }
        let (tid, cdp) = self.current()?;
        let expr = format!("document.querySelector({})", json!(target));
        let r = cdp
            .call(
                "Runtime.evaluate",
                json!({ "expression": expr, "objectGroup": OBJECT_GROUP }),
            )
            .await?;
        if let Some(e) = r.get("exceptionDetails") {
            return Err(Failure::new(
                Kind::Error,
                format!("invalid selector `{target}`: {}", exception_text(e)),
            ));
        }
        match r["result"]["objectId"].as_str() {
            Some(id) => Ok((tid, cdp, id.to_string())),
            None => Err(
                Failure::new(Kind::NotFound, format!("no element matches `{target}`"))
                    .with("selector", target),
            ),
        }
    }

    async fn read_cmd(&self, target: &str) -> Outcome {
        let (_, cdp, obj) = self.resolve(target).await?;
        let v = call_on(
            &cdp,
            &obj,
            "function() {
                const r = this.getBoundingClientRect();
                return {
                    tag: this.tagName.toLowerCase(),
                    text: (this.innerText ?? this.textContent ?? '').trim(),
                    value: 'value' in this ? String(this.value) : null,
                    state: {
                        focused: document.activeElement === this,
                        disabled: !!this.disabled,
                        checked: 'checked' in this ? !!this.checked : null,
                        visible: r.width > 0 && r.height > 0,
                    },
                };
            }",
            &[],
        )
        .await?;
        let mut f = Fields::new();
        if let Value::Object(m) = v {
            f.extend(m);
        }
        Ok(f)
    }

    /// Refuses input unless the focused element lies inside the required focus selector.
    async fn guard_focus(&self, cdp: &Cdp, require_focus: Option<&str>) -> Result<(), Failure> {
        let Some(sel) = require_focus else {
            return Ok(());
        };
        let expr = format!(
            "(() => {{
                const r = document.querySelector({sel});
                const a = document.activeElement;
                const d = a ? a.tagName.toLowerCase() + (a.id ? '#' + a.id : '') : null;
                return {{ matched: !!r, holds: !!(r && a && (r === a || r.contains(a))), active: d }};
            }})()",
            sel = json!(sel)
        );
        let v = evaluate(cdp, &expr).await?;
        if v["holds"] == true {
            return Ok(());
        }
        let message = if v["matched"] == true {
            format!("focus is outside `{sel}`; nothing was sent")
        } else {
            format!("no element matches the required focus `{sel}`; nothing was sent")
        };
        Err(Failure::new(Kind::GuardRefused, message)
            .with("expected", sel)
            .with("found", v["active"].clone()))
    }

    /// Refuses input into `obj` unless it lies inside the required focus selector; nothing on the page changes.
    async fn guard_target(
        &self,
        cdp: &Cdp,
        obj: &str,
        into: &str,
        require_focus: Option<&str>,
    ) -> Result<(), Failure> {
        let Some(sel) = require_focus else {
            return Ok(());
        };
        let v = call_on(
            cdp,
            obj,
            "function(sel) {
                const r = document.querySelector(sel);
                return { matched: !!r, inside: !!(r && (r === this || r.contains(this))) };
            }",
            &[json!(sel)],
        )
        .await?;
        if v["inside"] == true {
            return Ok(());
        }
        let message = if v["matched"] == true {
            format!("`{into}` is outside `{sel}`; nothing was sent")
        } else {
            format!("no element matches the required focus `{sel}`; nothing was sent")
        };
        Err(Failure::new(Kind::GuardRefused, message)
            .with("expected", sel)
            .with("found", into))
    }

    /// Resolves `into` when given, checks the focus guard before anything changes, then focuses it;
    /// without `into`, checks that the current focus holds. Returns the page to send input to.
    pub(super) async fn focus_for_input(
        &self,
        into: Option<&str>,
        require_focus: Option<&str>,
    ) -> Result<Arc<Cdp>, Failure> {
        match into {
            Some(into) => {
                let (tid, cdp, obj) = self.resolve(into).await?;
                self.guard_target(&cdp, &obj, into, require_focus).await?;
                self.ensure_ready(&tid, &cdp).await?;
                call_on(&cdp, &obj, "function() { this.focus(); return true; }", &[]).await?;
                self.guard_focus(&cdp, require_focus).await?;
                Ok(cdp)
            }
            None => {
                let (tid, cdp) = self.current()?;
                self.guard_focus(&cdp, require_focus).await?;
                self.ensure_ready(&tid, &cdp).await?;
                Ok(cdp)
            }
        }
    }

    async fn click_cmd(
        &self,
        at: Option<ClickAt<'_>>,
        how: ClickHow,
        require_focus: Option<&str>,
    ) -> Outcome {
        #[cfg(windows)]
        if how.route == Route::Os {
            self.refuse_unless_front().await?;
        }
        let (cdp, x, y) = match at {
            Some(ClickAt::Element(t)) => {
                let (tid, cdp, obj) = self.resolve(t).await?;
                self.guard_focus(&cdp, require_focus).await?;
                self.ensure_ready(&tid, &cdp).await?;
                let r = scroll_and_measure(&cdp, &obj).await?;
                if r["width"].as_f64() == Some(0.0) && r["height"].as_f64() == Some(0.0) {
                    return Err(Failure::new(
                        Kind::NotFound,
                        format!("`{t}` has no size on screen"),
                    ));
                }
                let x =
                    r["left"].as_f64().unwrap_or(0.0) + r["width"].as_f64().unwrap_or(0.0) / 2.0;
                let y =
                    r["top"].as_f64().unwrap_or(0.0) + r["height"].as_f64().unwrap_or(0.0) / 2.0;
                (cdp, x, y)
            }
            Some(ClickAt::Point(x, y)) => {
                let (tid, cdp) = self.current()?;
                self.guard_focus(&cdp, require_focus).await?;
                self.ensure_ready(&tid, &cdp).await?;
                (cdp, x, y)
            }
            None => {
                self.current()?;
                return Err(Failure::new(
                    Kind::Error,
                    "click needs a ref, a selector, or --x and --y",
                ));
            }
        };
        if how.route == Route::Os {
            #[cfg(windows)]
            return self.os_click(&cdp, x, y, &how).await;
            #[cfg(not(windows))]
            return Err(os_route_unsupported());
        }
        #[cfg(windows)]
        if let Some(pid) = self.app_pid {
            let front = crate::proc::foreground();
            if front.0 != 0 && front.1 != pid {
                *self.foreground_at_click.lock().unwrap() = Some(front);
            }
        }
        let mouse = |kind: &str, button: &str, count: i64| json!({ "type": kind, "x": x, "y": y, "button": button, "clickCount": count });
        let mut sent = vec![Instant::now()];
        cdp.call("Input.dispatchMouseEvent", mouse("mouseMoved", "none", 0))
            .await?;
        let action = if how.hover {
            "hover"
        } else {
            let button = if how.right { "right" } else { "left" };
            let count = if how.double { 2 } else { 1 };
            for n in 1..=count {
                sent.push(Instant::now());
                cdp.call("Input.dispatchMouseEvent", mouse("mousePressed", button, n))
                    .await?;
                sent.push(Instant::now());
                cdp.call(
                    "Input.dispatchMouseEvent",
                    mouse("mouseReleased", button, n),
                )
                .await?;
            }
            match (button, count) {
                ("right", _) => "right_click",
                (_, 2) => "double_click",
                _ => "click",
            }
        };
        Ok(self.stamp(fields! { "action" => action, "x" => x, "y" => y }, &sent))
    }

    async fn type_cmd(
        &self,
        text: &str,
        into: Option<&str>,
        require_focus: Option<&str>,
    ) -> Outcome {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let cdp = self.focus_for_input(into, require_focus).await?;
        let mut keyed = 0;
        let mut inserted = 0;
        let mut composed = 0;
        let mut sent = Vec::new();
        let mut ime = hangul::Composer::default();
        for c in text.chars() {
            if hangul::is_composed(c) {
                if !hangul::is_syllable(c) {
                    if let Some(rest) = ime.finish() {
                        commit(&cdp, &rest, &mut sent).await?;
                    }
                }
                for jamo in hangul::keys_for(c) {
                    let step = ime.press(jamo);
                    if let Err(e) = send_ime_key(&cdp, jamo, step, &mut sent).await {
                        let _ = cdp
                            .call(
                                "Input.imeSetComposition",
                                json!({ "text": "", "selectionStart": 0, "selectionEnd": 0 }),
                            )
                            .await;
                        return Err(e);
                    }
                }
                if !hangul::is_syllable(c) {
                    if let Some(rest) = ime.finish() {
                        commit(&cdp, &rest, &mut sent).await?;
                    }
                }
                composed += 1;
                continue;
            }
            if let Some(rest) = ime.finish() {
                commit(&cdp, &rest, &mut sent).await?;
            }
            let press = if c == '\n' {
                chord::parse_chord("Enter").ok()
            } else {
                chord::key_for_char(c)
            };
            match press {
                Some(p) => {
                    send_key(&cdp, &p, &mut sent).await?;
                    keyed += 1;
                }
                None => {
                    sent.push(Instant::now());
                    cdp.call("Input.insertText", json!({ "text": c.to_string() }))
                        .await?;
                    inserted += 1;
                }
            }
        }
        if let Some(rest) = ime.finish() {
            commit(&cdp, &rest, &mut sent).await?;
        }
        let f = fields! {
            "typed_chars" => keyed + inserted + composed,
            "key_events" => keyed,
            "inserted_chars" => inserted,
            "composed_chars" => composed,
        };
        Ok(self.stamp(f, &sent))
    }

    async fn key_cmd(
        &self,
        chord: &str,
        into: Option<&str>,
        require_focus: Option<&str>,
    ) -> Outcome {
        let press = chord::parse_chord(chord).map_err(|e| Failure::new(Kind::Error, e))?;
        let cdp = self.focus_for_input(into, require_focus).await?;
        let mut sent = Vec::new();
        send_key(&cdp, &press, &mut sent).await?;
        let f =
            fields! { "key" => press.key, "code" => press.code, "modifiers" => press.modifiers };
        Ok(self.stamp(f, &sent))
    }

    async fn wait_cmd(&self, condition: Option<Condition<'_>>, timeout_ms: u64) -> Outcome {
        let timeout = Duration::from_millis(timeout_ms);
        let (expr, expected, absent) = match condition {
            Some(Condition::Selector(s)) => (
                format!("!!document.querySelector({})", json!(s)),
                format!("an element matching `{s}`"),
                "no element matches".to_string(),
            ),
            Some(Condition::Text(t)) => (
                format!("(document.body?.innerText ?? '').includes({})", json!(t)),
                format!("the text {}", json!(t)),
                "not in the page's text".to_string(),
            ),
            Some(Condition::Expr(e)) => (
                format!("!!({e})"),
                format!("`{e}` to be true"),
                "false".to_string(),
            ),
            None => {
                return Err(Failure::new(
                    Kind::Error,
                    "wait needs --selector, --text or --expr",
                ))
            }
        };
        let start = Instant::now();
        let mut found = absent;
        loop {
            let (_, cdp) = self.current()?;
            match evaluate(&cdp, &expr).await {
                Ok(Value::Bool(true)) => {
                    return Ok(fields! { "waited_ms" => start.elapsed().as_millis() as u64 });
                }
                Ok(_) => {}
                Err(f) if f.kind == Kind::JsError => return Err(f.with("expected", expected)),
                Err(f) => found = f.message,
            }
            if start.elapsed() >= timeout {
                return Err(Failure::new(
                    Kind::Timeout,
                    format!("waited {}ms for {expected}", timeout.as_millis()),
                )
                .with("expected", expected)
                .with("found", found));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Where a capture goes: `out`, or the next numbered file in the run record.
    pub(super) fn capture_path(&self, out: Option<&str>) -> PathBuf {
        match out {
            Some(p) => PathBuf::from(p),
            None => self.record_dir.join(format!(
                "capture-{}.png",
                *self.record_seq.lock().unwrap() + 1
            )),
        }
    }

    async fn capture_cmd(&self, target: Option<&str>, out: Option<&str>) -> Outcome {
        let mut params = json!({ "format": "png" });
        let cdp = match target {
            Some(t) => {
                let (_, cdp, obj) = self.resolve(t).await?;
                let r = scroll_and_measure(&cdp, &obj).await?;
                let x = r["left"].as_f64().unwrap_or(0.0) + r["scroll_x"].as_f64().unwrap_or(0.0);
                let y = r["top"].as_f64().unwrap_or(0.0) + r["scroll_y"].as_f64().unwrap_or(0.0);
                params["clip"] = json!({ "x": x, "y": y, "width": r["width"], "height": r["height"], "scale": 1 });
                cdp
            }
            None => self.current()?.1,
        };
        let path = self.capture_path(out);
        let bytes = screenshot_to(&cdp, params, &path).await?;
        Ok(fields! { "path" => path.display().to_string(), "bytes" => bytes })
    }
}

/// How long a drag waits between pointer moves, about one frame.
pub(super) const DRAG_STEP: Duration = Duration::from_millis(16);

/// The longest a drag may take, well inside a command's default time limit.
const DRAG_LONGEST: Duration = Duration::from_secs(10);

/// Scrolls the element into view only if it is not already, and returns its rectangle.
const SCROLL_INTO_VIEW_IF_NEEDED: &str = "function() {
    this.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    const r = this.getBoundingClientRect();
    return { left: r.left, top: r.top, width: r.width, height: r.height }; }";

/// The element's rectangle in the viewport.
const RECT: &str = "function() { const r = this.getBoundingClientRect();
    return { left: r.left, top: r.top, width: r.width, height: r.height }; }";

impl State {
    /// Drags from element `from` to `to` (an element or a page point) through `via`, with the
    /// left button held: on CDP within the page, through the OS wherever the path leads.
    async fn drag_cmd(
        &self,
        from: &str,
        to: Option<ClickAt<'_>>,
        via: &[(f64, f64)],
        steps: u32,
        route: Route,
    ) -> Outcome {
        let Some(to) = to else {
            return Err(Failure::new(
                Kind::Usage,
                "drag needs --to, or --to-x and --to-y",
            ));
        };
        let moves = u64::from(steps) * (via.len() as u64 + 1);
        if Duration::from_millis(moves * DRAG_STEP.as_millis() as u64) > DRAG_LONGEST {
            return Err(Failure::new(
                Kind::Usage,
                format!(
                    "{moves} moves would take over {} s; use fewer --steps or --via points",
                    DRAG_LONGEST.as_secs()
                ),
            ));
        }
        #[cfg(windows)]
        if route == Route::Os {
            self.refuse_unless_front().await?;
        }
        let (tid, cdp, obj) = self.resolve(from).await?;
        self.ensure_ready(&tid, &cdp).await?;
        let size = evaluate(&cdp, "[innerWidth, innerHeight]").await?;
        let (Some(w), Some(h)) = (size[0].as_f64(), size[1].as_f64()) else {
            return Err(Failure::new(
                Kind::Error,
                format!("the page reported no size: {size}"),
            ));
        };
        let inside = |(x, y): (f64, f64)| (0.0..w).contains(&x) && (0.0..h).contains(&y);
        let center = |r: &Value| {
            (
                r["left"].as_f64().unwrap_or(0.0) + r["width"].as_f64().unwrap_or(0.0) / 2.0,
                r["top"].as_f64().unwrap_or(0.0) + r["height"].as_f64().unwrap_or(0.0) / 2.0,
            )
        };
        let drop_on = match to {
            ClickAt::Element(t) => {
                let (to_tid, _, to_obj) = self.resolve(t).await?;
                if to_tid != tid {
                    return Err(Failure::new(
                        Kind::Usage,
                        format!(
                            "`{t}` is in another page than `{from}`; drop at a point                              (--to-x, --to-y) with --route os to cross into another window"
                        ),
                    ));
                }
                Some((t, to_obj))
            }
            ClickAt::Point(..) => None,
        };
        let start = center(&call_on(&cdp, &obj, SCROLL_INTO_VIEW_IF_NEEDED, &[]).await?);
        let end = match (drop_on, to) {
            (Some((t, to_obj)), _) => {
                let at = center(&call_on(&cdp, &to_obj, RECT, &[]).await?);
                if !inside(at) {
                    return Err(Failure::new(
                        Kind::NotFound,
                        format!("`{t}` is not in view, so nothing can be dropped on it"),
                    ));
                }
                at
            }
            (None, ClickAt::Point(x, y)) => (x, y),
            (None, ClickAt::Element(t)) => {
                return Err(Failure::new(Kind::Error, format!("`{t}` was not resolved")))
            }
        };
        let points: Vec<(f64, f64)> = std::iter::once(start)
            .chain(via.iter().copied())
            .chain(std::iter::once(end))
            .collect();
        let path = crate::os_input::drag_path(&points, steps);
        let moves = path.len() - 1;
        let fields = |route: &str| {
            fields! {
                "from" => json!({ "x": start.0, "y": start.1 }),
                "to" => json!({ "x": end.0, "y": end.1 }),
                "moves" => moves,
                "route" => route,
            }
        };
        match route {
            Route::Cdp => {
                if let Some(&(x, y)) = path.iter().find(|&&p| !inside(p)) {
                    return Err(Failure::new(
                        Kind::NotFound,
                        format!(
                            "the path leaves the page at ({x}, {y}); only `--route os` can drag \
                             outside it. Nothing was sent"
                        ),
                    ));
                }
                let mouse = |kind: &str, (x, y): (f64, f64), buttons: i64| {
                    json!({ "type": kind, "x": x, "y": y, "button": "left",
                            "buttons": buttons, "clickCount": 1 })
                };
                let sent = vec![Instant::now()];
                cdp.call("Input.dispatchMouseEvent", mouse("mouseMoved", start, 0))
                    .await?;
                let mut at = start;
                let mut moved = cdp
                    .call("Input.dispatchMouseEvent", mouse("mousePressed", start, 1))
                    .await
                    .map(|_| ());
                for &p in &path[1..] {
                    if moved.is_err() {
                        break;
                    }
                    tokio::time::sleep(DRAG_STEP).await;
                    moved = cdp
                        .call("Input.dispatchMouseEvent", mouse("mouseMoved", p, 1))
                        .await
                        .map(|_| ());
                    if moved.is_ok() {
                        at = p;
                    }
                }
                let released = cdp
                    .call("Input.dispatchMouseEvent", mouse("mouseReleased", at, 0))
                    .await;
                moved?;
                released?;
                Ok(self.stamp(fields("cdp"), &sent))
            }
            #[cfg(windows)]
            Route::Os => {
                let mut f = self.os_drag(&cdp, path).await?;
                f.extend(fields("os"));
                Ok(f)
            }
            #[cfg(not(windows))]
            Route::Os => Err(os_route_unsupported()),
        }
    }
}

/// How far CDP turns the wheel for one notch, in CSS pixels; what the OS route scrolls a page
/// for one notch at 100% (docs/map/sending-os-input.md).
const PIXELS_PER_NOTCH: f64 = 100.0;

impl State {
    /// Turns the mouse wheel over an element, a point, or the middle of the page, without
    /// scrolling anything into view first.
    pub(super) async fn wheel_cmd(
        &self,
        target: Option<&str>,
        x: Option<f64>,
        y: Option<f64>,
        notches: i32,
        route: Route,
    ) -> Outcome {
        #[cfg(windows)]
        if route == Route::Os {
            self.refuse_unless_front().await?;
        }
        let (cdp, x, y) = match (target, x, y) {
            (Some(t), _, _) => {
                let (tid, cdp, obj) = self.resolve(t).await?;
                self.ensure_ready(&tid, &cdp).await?;
                let r = call_on(
                    &cdp,
                    &obj,
                    "function() { const r = this.getBoundingClientRect();
                        const left = Math.max(r.left, 0), right = Math.min(r.right, innerWidth);
                        const top = Math.max(r.top, 0), bottom = Math.min(r.bottom, innerHeight);
                        return { x: (left + right) / 2, y: (top + bottom) / 2,
                                 inside: right > left && bottom > top }; }",
                    &[],
                )
                .await?;
                if r["inside"] != true {
                    return Err(Failure::new(
                        Kind::NotFound,
                        format!("`{t}` is not in view; the wheel turns where the cursor is"),
                    ));
                }
                (
                    cdp,
                    r["x"].as_f64().unwrap_or(0.0),
                    r["y"].as_f64().unwrap_or(0.0),
                )
            }
            (None, Some(x), Some(y)) => {
                let (tid, cdp) = self.current()?;
                self.ensure_ready(&tid, &cdp).await?;
                let size = evaluate(&cdp, "[innerWidth, innerHeight]").await?;
                let (w, h) = (
                    size[0].as_f64().unwrap_or(0.0),
                    size[1].as_f64().unwrap_or(0.0),
                );
                if !(0.0..w).contains(&x) || !(0.0..h).contains(&y) {
                    return Err(Failure::new(
                        Kind::NotFound,
                        format!("({x}, {y}) is outside the page's {w}x{h} viewport"),
                    ));
                }
                (cdp, x, y)
            }
            _ => {
                let (tid, cdp) = self.current()?;
                self.ensure_ready(&tid, &cdp).await?;
                let v = evaluate(&cdp, "({ x: innerWidth / 2, y: innerHeight / 2 })").await?;
                (
                    cdp,
                    v["x"].as_f64().unwrap_or(0.0),
                    v["y"].as_f64().unwrap_or(0.0),
                )
            }
        };
        match route {
            Route::Cdp => {
                let sent = Instant::now();
                cdp.call(
                    "Input.dispatchMouseEvent",
                    json!({ "type": "mouseWheel", "x": x, "y": y, "deltaX": 0,
                            "deltaY": f64::from(notches) * PIXELS_PER_NOTCH }),
                )
                .await?;
                let f = fields! { "notches" => notches, "x" => x, "y" => y, "route" => "cdp" };
                Ok(self.stamp(f, &[sent]))
            }
            #[cfg(windows)]
            Route::Os => self.os_wheel(&cdp, x, y, notches).await,
            #[cfg(not(windows))]
            Route::Os => Err(os_route_unsupported()),
        }
    }
}

/// Where a click lands: an element (a ref or selector), or a point in the page.
enum ClickAt<'a> {
    Element(&'a str),
    Point(f64, f64),
}

/// Which click: a hover, a right click, or a single or double left click.
pub(super) struct ClickHow {
    pub(super) double: bool,
    pub(super) right: bool,
    pub(super) hover: bool,
    pub(super) route: Route,
}

/// What `wait` waits for.
enum Condition<'a> {
    Selector(&'a str),
    Text(&'a str),
    Expr(&'a str),
}

/// One key of a 2-Set IME: a `Process` key-down, the text the IME commits, the composition it
/// leaves, then the key-up, noting when each event was sent.
async fn send_ime_key(
    cdp: &Cdp,
    jamo: char,
    step: hangul::Step,
    sent: &mut Vec<Instant>,
) -> Result<(), Failure> {
    let (code, shift) = hangul::key_for(jamo).unwrap_or(("", false));
    let vk = chord::parse_chord(code).map(|p| p.vk).unwrap_or(0);
    let modifiers = if shift { chord::SHIFT } else { 0 };
    sent.push(Instant::now());
    cdp.call(
        "Input.dispatchKeyEvent",
        json!({ "type": "rawKeyDown", "key": "Process", "code": code,
                "windowsVirtualKeyCode": 229, "modifiers": modifiers }),
    )
    .await?;
    if let Some(text) = &step.committed {
        commit(cdp, text, sent).await?;
    }
    if !step.composing.is_empty() {
        let end = step.composing.encode_utf16().count();
        sent.push(Instant::now());
        cdp.call(
            "Input.imeSetComposition",
            json!({ "text": step.composing, "selectionStart": end, "selectionEnd": end }),
        )
        .await?;
    }
    sent.push(Instant::now());
    cdp.call(
        "Input.dispatchKeyEvent",
        json!({ "type": "keyUp", "key": jamo.to_string(), "code": code,
                "windowsVirtualKeyCode": vk, "modifiers": modifiers }),
    )
    .await?;
    Ok(())
}

/// Commits `text`, ending the composition in progress.
async fn commit(cdp: &Cdp, text: &str, sent: &mut Vec<Instant>) -> Result<(), Failure> {
    sent.push(Instant::now());
    cdp.call("Input.insertText", json!({ "text": text }))
        .await?;
    Ok(())
}

/// `--route os` where OS input is not built yet.
#[cfg(not(windows))]
fn os_route_unsupported() -> Failure {
    Failure::new(
        Kind::Unsupported,
        "OS input is not built on this platform yet",
    )
}

/// Presses and releases `p` as a keyboard would, its modifiers as keys of their own around it,
/// noting when each event was sent.
async fn send_key(cdp: &Cdp, p: &KeyPress, sent: &mut Vec<Instant>) -> Result<(), Failure> {
    let (downs, ups) = p.modifier_events();
    let events = downs
        .into_iter()
        .chain([p.down_event(false), p.up_event()])
        .chain(ups);
    for event in events {
        sent.push(Instant::now());
        cdp.call("Input.dispatchKeyEvent", event).await?;
    }
    Ok(())
}

/// Scrolls the element into view and returns its viewport rectangle and the page's scroll offset.
async fn scroll_and_measure(cdp: &Cdp, obj: &str) -> Result<Value, Failure> {
    call_on(
        cdp,
        obj,
        "function() {
            this.scrollIntoView({ block: 'center', inline: 'center' });
            const r = this.getBoundingClientRect();
            return { left: r.left, top: r.top, width: r.width, height: r.height,
                     scroll_x: window.scrollX, scroll_y: window.scrollY };
        }",
        &[],
    )
    .await
}

pub(super) fn exception_text(details: &Value) -> String {
    details["exception"]["description"]
        .as_str()
        .or(details["text"].as_str())
        .unwrap_or("exception")
        .to_string()
}

pub(super) async fn evaluate(cdp: &Cdp, expr: &str) -> Result<Value, Failure> {
    evaluate_within(cdp, expr, Duration::from_secs(15)).await
}

pub(super) async fn evaluate_within(
    cdp: &Cdp,
    expr: &str,
    timeout: Duration,
) -> Result<Value, Failure> {
    let params = json!({ "expression": expr, "returnByValue": true, "awaitPromise": true });
    let r = cdp
        .call_timeout("Runtime.evaluate", params, timeout)
        .await?;
    if let Some(e) = r.get("exceptionDetails") {
        return Err(Failure::new(Kind::JsError, exception_text(e)));
    }
    Ok(r["result"]["value"].clone())
}

async fn call_on(
    cdp: &Cdp,
    object_id: &str,
    function: &str,
    arguments: &[Value],
) -> Result<Value, Failure> {
    let arguments: Vec<Value> = arguments.iter().map(|v| json!({ "value": v })).collect();
    let r = cdp
        .call(
            "Runtime.callFunctionOn",
            json!({ "objectId": object_id, "functionDeclaration": function, "arguments": arguments,
                    "returnByValue": true, "awaitPromise": true }),
        )
        .await?;
    if let Some(e) = r.get("exceptionDetails") {
        return Err(Failure::new(Kind::JsError, exception_text(e)));
    }
    Ok(r["result"]["value"].clone())
}

//! Delivering key events and text to a webview's own window, and mouse events to the view under
//! a point, from inside the app.

use crate::channel::failure;
use objc2::msg_send;
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType, NSView};
use objc2_foundation::{MainThreadMarker, NSNotFound, NSPoint, NSProcessInfo, NSRange, NSString};
use objc2_web_kit::WKWebView;
use serde_json::{json, Value};
use std::sync::mpsc;
use std::time::Duration;
use tauri::{Runtime, WebviewWindow};

/// How long the main thread may take to deliver the events.
const DELIVER_TIMEOUT: Duration = Duration::from_secs(10);

/// One event to deliver, as the request names it.
enum Event {
    /// A key-down, key-up or modifier change, given to the window's `sendEvent:`.
    Key {
        kind: NSEventType,
        key_code: u16,
        characters: String,
        unmodified: String,
        flags: usize,
        repeat: bool,
    },
    /// Text given to the webview's `insertText:replacementRange:`, as an input method commits it.
    Insert(String),
    /// A mouse event at a point of the page (CSS pixels from its top left), given to the view under
    /// it.
    Mouse {
        kind: NSEventType,
        x: f64,
        y: f64,
        clicks: isize,
    },
}

fn parse(event: &Value) -> Result<Event, String> {
    let kind = match event["type"].as_str() {
        Some("insert") => {
            let text = event["text"].as_str().ok_or("insert needs `text`")?;
            return Ok(Event::Insert(text.to_string()));
        }
        Some(mouse @ ("mousedown" | "mouseup" | "mousemove")) => {
            let right = match event["button"].as_str() {
                None | Some("left") => false,
                Some("right") => true,
                Some(other) => return Err(format!("unknown button {other:?}")),
            };
            let kind = match (mouse, right) {
                ("mousedown", false) => NSEventType::LeftMouseDown,
                ("mousedown", true) => NSEventType::RightMouseDown,
                ("mouseup", false) => NSEventType::LeftMouseUp,
                ("mouseup", true) => NSEventType::RightMouseUp,
                _ => NSEventType::LeftMouseDragged,
            };
            let at = |name: &str| {
                event[name]
                    .as_f64()
                    .ok_or(format!("a mouse event needs `{name}`"))
            };
            return Ok(Event::Mouse {
                kind,
                x: at("x")?,
                y: at("y")?,
                clicks: event["clicks"].as_i64().unwrap_or(0) as isize,
            });
        }
        Some("keydown") => NSEventType::KeyDown,
        Some("keyup") => NSEventType::KeyUp,
        Some("flags") => NSEventType::FlagsChanged,
        other => return Err(format!("unknown event type {other:?}")),
    };
    let key_code = event["key_code"]
        .as_u64()
        .and_then(|c| u16::try_from(c).ok())
        .ok_or("a key event needs `key_code`")?;
    let text = |name: &str| event[name].as_str().unwrap_or("").to_string();
    Ok(Event::Key {
        kind,
        key_code,
        characters: text("characters"),
        unmodified: text("unmodified"),
        flags: event["flags"].as_u64().unwrap_or(0) as usize,
        repeat: event["repeat"] == true,
    })
}

/// The window point under page point (`x`, `y`), in CSS pixels from the page's top left.
fn window_point(view: &WKWebView, x: f64, y: f64) -> NSPoint {
    // SAFETY: plain property reads on the main thread.
    let scale = unsafe { view.pageZoom() * view.magnification() };
    let inset = view.safeAreaInsets();
    view.convertPoint_toView(
        NSPoint::new(inset.left + x * scale, inset.top + y * scale),
        None,
    )
}

/// Gives `event` to `view`'s handler for its kind, as AppKit does for a click on the view.
fn give(view: &NSView, kind: NSEventType, event: &NSEvent) {
    match kind {
        NSEventType::LeftMouseDown => view.mouseDown(event),
        NSEventType::LeftMouseUp => view.mouseUp(event),
        NSEventType::RightMouseDown => view.rightMouseDown(event),
        NSEventType::RightMouseUp => view.rightMouseUp(event),
        _ => view.mouseDragged(event),
    }
}

/// Delivers `events` in order to `webview`'s window, on the main thread.
pub fn deliver<R: Runtime>(webview: &WebviewWindow<R>, events: &[Value]) -> Value {
    let events = match events.iter().map(parse).collect::<Result<Vec<_>, _>>() {
        Ok(e) => e,
        Err(e) => return failure("bad_request", e),
    };
    let (tx, rx) = mpsc::channel::<Value>();
    let dispatched = webview.with_webview(move |platform| {
        let reply = (|| {
            MainThreadMarker::new().ok_or("with_webview ran off the main thread")?;
            // SAFETY: on macOS `inner` is this webview's live WKWebView, and we are on the main thread.
            let view: &WKWebView = unsafe { &*(platform.inner() as *const WKWebView) };
            let window = view.window().ok_or("the webview is not in a window")?;
            let mut menu_watch = None;
            for event in &events {
                match event {
                    Event::Key {
                        kind,
                        key_code,
                        characters,
                        unmodified,
                        flags,
                        repeat,
                    } => {
                        let time = NSProcessInfo::processInfo().systemUptime();
                        let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                            *kind,
                            NSPoint::new(0.0, 0.0),
                            NSEventModifierFlags(*flags),
                            time,
                            window.windowNumber(),
                            None,
                            &NSString::from_str(characters),
                            &NSString::from_str(unmodified),
                            *repeat,
                            *key_code,
                        )
                        .ok_or("AppKit made no key event")?;
                        window.sendEvent(&event);
                    }
                    Event::Insert(text) => {
                        let text = NSString::from_str(text);
                        let nowhere = NSRange::new(NSNotFound as usize, 0);
                        // SAFETY: WKWebView implements NSTextInputClient's insertText:replacementRange:.
                        let _: () = unsafe { msg_send![view, insertText: &*text, replacementRange: nowhere] };
                    }
                    Event::Mouse { kind, x, y, clicks } => {
                        let at = window_point(view, *x, *y);
                        let content = window.contentView().ok_or("the window has no content view")?;
                        // SAFETY: on the main thread, reading the view hierarchy.
                        let frame = unsafe { content.superview() }.unwrap_or_else(|| content.clone());
                        let target = content
                            .hitTest(frame.convertPoint_fromView(at, None))
                            .filter(|hit| hit.isDescendantOf(view))
                            .ok_or("the point is not over the page")?;
                        let pressure = if matches!(*kind, NSEventType::LeftMouseDown | NSEventType::RightMouseDown) { 1.0 } else { 0.0 };
                        let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                            *kind,
                            at,
                            NSEventModifierFlags(0),
                            NSProcessInfo::processInfo().systemUptime(),
                            window.windowNumber(),
                            None,
                            0,
                            *clicks,
                            pressure,
                        )
                        .ok_or("AppKit made no mouse event")?;
                        if *kind == NSEventType::RightMouseDown {
                            menu_watch = Some(crate::menu::arm());
                        }
                        give(&target, *kind, &event);
                    }
                }
            }
            Ok::<Value, &str>(json!({ "ok": true, "menu_watch": menu_watch }))
        })();
        let _ = tx.send(reply.unwrap_or_else(|e| failure("error", e)));
    });
    if let Err(e) = dispatched {
        return failure("error", format!("could not reach the webview: {e}"));
    }
    rx.recv_timeout(DELIVER_TIMEOUT).unwrap_or_else(|_| {
        failure(
            "timeout",
            "the main thread did not deliver the events in time",
        )
    })
}

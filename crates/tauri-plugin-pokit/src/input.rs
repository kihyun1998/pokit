//! Delivering key events and text to a webview's own window, from inside the app.

use crate::channel::failure;
use objc2::msg_send;
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType};
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
}

fn parse(event: &Value) -> Result<Event, String> {
    let kind = match event["type"].as_str() {
        Some("insert") => {
            let text = event["text"].as_str().ok_or("insert needs `text`")?;
            return Ok(Event::Insert(text.to_string()));
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
                }
            }
            Ok::<Value, &str>(json!({ "ok": true }))
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

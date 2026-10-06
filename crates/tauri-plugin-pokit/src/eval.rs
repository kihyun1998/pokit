//! Evaluating pokit's JavaScript in a WKWebView, awaiting a promise, with the value as JSON.

use crate::channel::failure;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{MainThreadMarker, NSDictionary, NSError, NSString};
use objc2_web_kit::{WKContentWorld, WKWebView};
use serde_json::{json, Value};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tauri::{Runtime, WebviewWindow};

/// The function body for a single expression, compiled with the expression in place.
fn expression_body(js: &str) -> String {
    format!(
        "let __pokitValue;\ntry {{ __pokitValue = await (\n{js}\n); }} catch (e) {{ return __pokitThrown(e); }}\nreturn __pokitJson(__pokitValue);{HELPERS}"
    )
}

/// The function body for anything else, such as a list of statements: the completion value of
/// an indirect `eval` of `expression`.
const STATEMENTS_BODY: &str = "let __pokitValue;\ntry { __pokitValue = await (0, eval)(expression); } catch (e) { return __pokitThrown(e); }\nreturn __pokitJson(__pokitValue);";

/// Turn what the code produced into the JSON text handed back: `{"value": …}` with `undefined`
/// and functions as `null`, or `{"thrown": …}` with what it threw as text.
const HELPERS: &str = "\nfunction __pokitJson(v) { const j = JSON.stringify(v === undefined ? null : v); return '{\"value\":' + (j === undefined ? 'null' : j) + '}'; }\nfunction __pokitThrown(e) { return JSON.stringify({ thrown: String(e) }); }";

/// WebKit's error for a promise nothing can resolve any more.
const UNREACHABLE: &str = "Completion handler for function call is no longer reachable";

/// Evaluates `js` in the page world of `webview`'s main frame and waits up to `timeout`.
pub fn evaluate<R: Runtime>(webview: &WebviewWindow<R>, js: &str, timeout: Duration) -> Value {
    let deadline = Instant::now() + timeout;
    let reply = call(webview, expression_body(js), None, timeout);
    if !is_compile_error(&reply) {
        return without_ran(reply);
    }
    without_ran(call(
        webview,
        format!("{STATEMENTS_BODY}{HELPERS}"),
        Some(js.to_string()),
        deadline.saturating_duration_since(Instant::now()),
    ))
}

/// The reply as pokit gets it, without the mark `is_compile_error` reads.
fn without_ran(mut reply: Value) -> Value {
    if let Some(error) = reply["error"].as_object_mut() {
        error.remove("ran");
    }
    reply
}

/// Whether the body failed to compile: a `SyntaxError` from WebKit, not one the code threw while
/// running (`ran`).
fn is_compile_error(reply: &Value) -> bool {
    reply["error"]["kind"] == "js_error"
        && reply["error"]["ran"] != true
        && reply["error"]["message"]
            .as_str()
            .is_some_and(|m| m.starts_with("SyntaxError"))
}

/// One `callAsyncJavaScript`, with `expression` passed as an argument when given.
fn call<R: Runtime>(
    webview: &WebviewWindow<R>,
    body: String,
    expression: Option<String>,
    timeout: Duration,
) -> Value {
    let (tx, rx) = mpsc::channel::<Value>();
    let dispatched = webview.with_webview(move |platform| {
        let Some(mtm) = MainThreadMarker::new() else {
            let _ = tx.send(failure("error", "with_webview ran off the main thread"));
            return;
        };
        // SAFETY: on macOS `inner` is this webview's live WKWebView, and we are on the main thread.
        let view: &WKWebView = unsafe { &*(platform.inner() as *const WKWebView) };
        let key = NSString::from_str("expression");
        let value = NSString::from_str(expression.as_deref().unwrap_or(""));
        let arguments: Retained<NSDictionary<NSString, AnyObject>> =
            NSDictionary::from_slices(&[&*key], &[value.as_ref() as &AnyObject]);
        let handler = RcBlock::new(move |result: *mut AnyObject, error: *mut NSError| {
            // SAFETY: WebKit hands the completion handler either null or a valid object for each.
            let reply = unsafe { reply_of(result.as_ref(), error.as_ref()) };
            let _ = tx.send(reply);
        });
        // SAFETY: the arguments dictionary maps NSString keys to NSString values, as WebKit expects.
        unsafe {
            view.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
                &NSString::from_str(&body),
                Some(&arguments),
                None,
                &WKContentWorld::pageWorld(mtm),
                Some(&handler),
            );
        }
    });
    if let Err(e) = dispatched {
        return failure("error", format!("could not reach the webview: {e}"));
    }
    rx.recv_timeout(timeout).unwrap_or_else(|_| {
        failure(
            "timeout",
            format!("the page did not answer within {} ms", timeout.as_millis()),
        )
    })
}

fn reply_of(result: Option<&AnyObject>, error: Option<&NSError>) -> Value {
    if let Some(error) = error {
        let message = error
            .userInfo()
            .objectForKey(&NSString::from_str("WKJavaScriptExceptionMessage"))
            .and_then(|m| m.downcast_ref::<NSString>().map(|s| s.to_string()))
            .unwrap_or_else(|| error.localizedDescription().to_string());
        if message.contains(UNREACHABLE) {
            return failure(
                "timeout",
                "the page's promise can never settle: nothing can resolve it, so WebKit dropped it",
            );
        }
        return failure("js_error", message);
    }
    let text = result
        .and_then(|r| r.downcast_ref::<NSString>())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "{\"value\":null}".into());
    match serde_json::from_str::<Value>(&text) {
        Ok(answer) if answer.get("thrown").is_some() => {
            let mut thrown = failure("js_error", answer["thrown"].as_str().unwrap_or_default());
            thrown["error"]["ran"] = json!(true);
            thrown
        }
        Ok(answer) => json!({ "ok": true, "value": answer["value"] }),
        Err(e) => failure("error", format!("the page's answer is not JSON: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An error as WebKit hands it to the completion handler, with `message` as the exception.
    fn webkit_error(message: &str) -> Retained<NSError> {
        let key = NSString::from_str("WKJavaScriptExceptionMessage");
        let value = NSString::from_str(message);
        let info: Retained<NSDictionary<NSString, AnyObject>> =
            NSDictionary::from_slices(&[&*key], &[value.as_ref() as &AnyObject]);
        // SAFETY: the user info maps an NSString key to an NSString value.
        unsafe {
            NSError::errorWithDomain_code_userInfo(
                &NSString::from_str("WKErrorDomain"),
                4,
                Some(&info),
            )
        }
    }

    #[test]
    fn a_promise_webkit_dropped_is_a_timeout() {
        let reply = reply_of(None, Some(&webkit_error(UNREACHABLE)));
        assert_eq!(reply["error"]["kind"], "timeout", "{reply}");
    }

    #[test]
    fn an_exception_is_a_js_error_with_its_message() {
        let reply = reply_of(None, Some(&webkit_error("Error: fixture boom")));
        assert_eq!(reply["error"]["kind"], "js_error", "{reply}");
        assert_eq!(reply["error"]["message"], "Error: fixture boom", "{reply}");
    }

    #[test]
    fn a_value_comes_back_parsed() {
        let text = NSString::from_str("{\"value\":{\"a\":[1,\"x\"]}}");
        let reply = reply_of(Some(text.as_ref()), None);
        assert_eq!(reply, json!({ "ok": true, "value": { "a": [1, "x"] } }));
    }

    #[test]
    fn what_the_code_threw_is_a_js_error() {
        let text = NSString::from_str("{\"thrown\":\"SyntaxError: JSON Parse error\"}");
        let reply = reply_of(Some(text.as_ref()), None);
        assert_eq!(reply["error"]["kind"], "js_error", "{reply}");
        assert_eq!(reply["error"]["message"], "SyntaxError: JSON Parse error");
        assert!(
            !is_compile_error(&reply),
            "a throw while running read as a compile error"
        );
    }
}

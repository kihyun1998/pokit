# pokit

A Rust CLI that feeds real input into a running webview desktop app and reports what the page did.

## Language

**Fixture app**:
The webview app this repo owns as a test target, built so the right measurement is known before pokit takes it (for example, a page that stalls a fixed time on every key).
_Avoid_: test app, sample app, dummy app

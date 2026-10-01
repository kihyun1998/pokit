# pokit

A Rust CLI that feeds real input into a running webview desktop app and reports what the page did.

## Language

**Fixture app**:
The webview app this repo owns as a test target, built so the right measurement is known before pokit takes it (for example, a page that stalls a fixed time on every key).
_Avoid_: test app, sample app, dummy app

**Snapshot**:
The page's structure as text — each element's role, name, value and state — with a ref beside every element an agent can act on.
_Avoid_: DOM dump, tree, outline

**Ref**:
A short label (`e12`) that names one element of the most recent snapshot, and only of that snapshot.
_Avoid_: id, handle, element number

**Session**:
The background pokit process that holds the connection to one app instance, from `launch` or `attach` until `close`; every other command is a request to it.
_Avoid_: daemon, server, connection

**Test build**:
A build of the measured app with pokit's plugin feature turned on, optimised like a release, and never shipped.
_Avoid_: debug build, profiling build, dev build

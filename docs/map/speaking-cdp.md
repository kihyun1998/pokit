# Speaking CDP

## What it is

The hand-written Chrome DevTools Protocol client pokit talks to a WebView2 page with, and the two DevTools HTTP endpoints it reads on the debugging port. One WebSocket per page target carries pokit's commands and brings back their replies and the page's events; `/json/list` says which page targets exist and `/json/version` whether a browser answers at all. Which pages are followed, and what is done with their events, is [The app's pages](the-apps-pages.md).

## Governing decisions

- [ADR-0002](../adr/0002-hand-written-cdp-client.md): a hand-written client carrying only the methods pokit uses, not `chromiumoxide`, so that every byte sent to the app is pokit's choice, including firing `hold`'s key-downs without awaiting replies. Re-verified at HEAD: `crates/pokit/src/cdp.rs` sends a message only from `Cdp::send`, which only a caller's method reaches; `State::cdp_hold` (session/measure.rs) sends every key event with `Cdp::send` and awaits the `Reply`s after the last.

## Design model

The rules come from ADR-0002 and from reading the source; the HTTP trap below was found in #3.

- **One connection per page target, opened on its `webSocketDebuggerUrl`** (`Cdp::connect`). A writer task drains an unbounded queue into the socket; a reader task matches each reply to its call by `id` and forwards every message without an `id` as an event `(target id, method, params)` on the channel given to `connect` (`cdp::Event`). Ids are per connection, counted from 1 (the `next` field of `Cdp`).
- **Nothing is sent that a caller did not ask for.** `connect` sends nothing on its own; the first call on a new connection is `Runtime.enable`, made by `State::sync_targets` ([The app's pages](the-apps-pages.md)). The methods pokit sends at HEAD, by tool:
  ```sh
  rg -oIN '"(Runtime|Page|Input|DOM|Accessibility|Tracing|Profiler|IO)\.[a-z][a-zA-Z]+"' crates/pokit/src | sort -u
  ```
  (this also lists the events pokit listens for, such as `Runtime.consoleAPICalled`).
- **A call waits 15 s by default** (`Cdp::call`); `Cdp::call_timeout` takes another limit, as `eval` and the page helpers do (`evaluate_within`, session/commands.rs). `Cdp::send` returns a `Reply` at once, to be awaited later with `Reply::wait`.
- **A reply that is no longer awaited is discarded.** Dropping a `Reply`, as a timed-out wait does, removes its entry from the pending map (`Reply::drop`), so an answer arriving after the timeout matches nothing and is ignored.
- **Three ways a call fails** (`CdpError`): `Timeout` (no answer in time), `Closed` (the connection is gone, or went while waiting), `Protocol` (the page answered with an `error`). A `Timeout` becomes a failure of kind `timeout`, the other two of kind `error` (`impl From<CdpError> for Failure`, output.rs); what the kinds print is [The command-line contract](the-command-line-contract.md).
- **When the reader stops, the connection is closed for good.** The reader loop ends on the socket's end or on the first read error; it then marks the connection closed (`Cdp::is_closed`) and fails every pending call with "the page's connection closed". A later `send` fails at once with `Closed`. Nothing reconnects inside `Cdp`; `State::sync_targets` opens a new connection for a target whose old one is closed, on its next pass.
- **The DevTools HTTP endpoints are read with a hand-written HTTP/1.1 GET to `127.0.0.1:<port>`** (`devtools::get_json`), the whole exchange limited to 5 s.
  - **The body is cut at `Content-Length`.** The DevTools HTTP server can keep the connection open after answering, even when asked to close it (`Connection: close` is sent), so a reader that waits for end-of-stream hangs until its timeout (found in #3, 2026-10-01). `get_json` returns as soon as the body reaches `Content-Length` (`content_length`); a chunked body is returned once it ends in the zero chunk (`dechunk`); only without either does it read to the end of the stream.
  - `devtools::pages` keeps the entries of `/json/list` whose `type` is `page`. `/json/version` is read only by `doctor` (`doctor.rs`, [Checking preconditions](checking-preconditions.md)).
- **Size and count of what comes in.** pokit sets no limit of its own on a WebSocket message; `tokio-tungstenite` 0.24.0's defaults apply, 64 MiB a message and 16 MiB a frame (`tungstenite` 0.24.0, `WebSocketConfig::default`, `src/protocol/mod.rs`, read 2026-10-30). A message over that is a read error, which ends the reader and closes the connection (above). Events go into an unbounded channel (`mpsc::unbounded_channel` in `start_over_cdp`, session/mod.rs, drained by `collect_events`); the pending map holds one entry per call in flight. An HTTP reply is buffered whole with no size limit; only the 5 s limit bounds it.

## Code

- `crates/pokit/src/cdp.rs` — `Cdp`, `Cdp::connect`, `Cdp::call`, `Cdp::call_timeout`, `Cdp::send`, `Cdp::is_closed`, `Reply`, `Reply::wait`, `CdpError`, `Event`
- `crates/pokit/src/devtools.rs` — `get_json`, `content_length`, `dechunk`, `pages`
- `crates/pokit/src/output.rs` — `impl From<crate::cdp::CdpError> for Failure`

## Reference behaviour

- **`/json/version` on WebView2 returns a browser-level `webSocketDebuggerUrl`.** `Target.getTargets`, `Browser.getVersion` and `Target.attachToTarget` with `flatten` all work there (probe, 2026-10-01). pokit still connects to each page's own WebSocket from `/json/list` ([The app's pages](the-apps-pages.md)).
- `tungstenite` 0.24.0, `src/protocol/mod.rs`, `WebSocketConfig::default` (read 2026-10-30): `max_message_size` 64 MiB, `max_frame_size` 16 MiB — the only bound on a CDP message pokit receives.
- The DevTools HTTP server keeping the connection open after a `Connection: close` request: seen in #3 (2026-10-01) on WebView2; no upstream source was read for it.
- Prior art compared: `chromiumoxide`'s `Browser::connect`, in [ADR-0002](../adr/0002-hand-written-cdp-client.md).

## Cross-cutting invariants

**None.**

## Blast radius

- [The app's pages](the-apps-pages.md) — it treats a closed connection as a target to reconnect, and relies on events arriving tagged with the target id; a change to when `is_closed` turns true or to the event's shape breaks discovery, readiness and logs.
- [Measuring the page](measuring-the-page.md) — `hold` relies on `Cdp::send` putting a message on the wire without waiting, so its key-downs keep their schedule; a `send` that awaited anything would bend every interval it measures.
- [Collecting logs](collecting-logs.md) — console and exception entries are the events the reader forwards; dropping or reordering events loses log lines.
- [The command-line contract](the-command-line-contract.md) — a CDP timeout is reported as kind `timeout` and the rest as `error`; changing `CdpError` or its conversion changes exit codes agents act on.
- [Launching a test instance](launching-a-test-instance.md) and [Checking preconditions](checking-preconditions.md) — both read `/json/list` or `/json/version` through `get_json`; a reader that waited for the stream to end would hang them until the 5 s limit.
- [Testing against the fixture app](testing-against-the-fixture.md) — the harness talks to the debugging port itself and shares the `Content-Length` trap.

## Known holes / open

- A CDP message over 64 MiB, such as a `Page.captureScreenshot` reply for a very large clip, would end the page's connection rather than fail one call (read from `Cdp::connect` and the `tungstenite` default, not measured).
- The event channel is unbounded: a page that logs faster than `collect_events` drains grows the session's memory without limit (read from the code, not measured).
- An HTTP reply is buffered without a size limit; a debugging port listing very many targets is bounded only by the 5 s limit.

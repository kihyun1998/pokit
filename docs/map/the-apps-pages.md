# The app's pages

## What it is

How a session over CDP keeps track of the app's open pages and decides which one a command goes to: page targets found on the debugging port, one connection each, the main page and the current page, `targets` to list and switch, the readiness check before input, and the seam (`Page`) that lets `eval`, `wait` and `read` run on either engine, CDP or the macOS plugin. `eval` and `wait` live here. How the connection itself speaks is [Speaking CDP](speaking-cdp.md); how the main page is found and first made ready at launch is [Launching a test instance](launching-a-test-instance.md).

## Governing decisions

- [ADR-0001](../adr/0001-commands-talk-to-a-session.md): the session holds the connections between commands, which is why page targets, the current page and readiness are state that outlives one command.
- [ADR-0002](../adr/0002-hand-written-cdp-client.md) decides the client each page's connection uses, not which pages are followed.

## Design model

The rules come from reading the source; the readiness definition was found while making launch reliable (#3, #27; see [Launching a test instance](launching-a-test-instance.md)).

- **Every page target on the instance's own port is followed, and nothing else.** `discover` reads `/json/list` every 500 ms and `State::sync_targets` connects to each page target it does not hold, or holds on a closed connection, and drops targets no longer listed (with their waiters). A `webSocketDebuggerUrl` that does not start with `ws://127.0.0.1:<port>/` or `ws://localhost:<port>/` is not followed and a `session` warning is logged. `sync_targets` runs one at a time (`State::sync_lock`). Each new connection gets `Runtime.enable`, which is what brings console messages, exceptions and context events ([Collecting logs](collecting-logs.md)).
- **Targets keep the order they were first seen** (`State::order`); a target's index in `targets` is its place in that order.
- **The main page is fixed at launch; the current page is where commands go.** `wait_ready` sets both to the page matching `--main-url`, or the first page listed ([Launching a test instance](launching-a-test-instance.md)). `targets --select <s>` sets the current page (`State::targets_cmd`): `s` is tried as a target id, then as an index, then as a substring of a URL or title, in that order; nothing matching is `not_found` (pinned by `a_second_window_is_listed_and_can_be_switched_to`, tests/fixture.rs). `targets` reads `/json/list` and syncs first, so a window opened a moment ago is listed without waiting for `discover`.
- **A current page that closes is not replaced.** `State::current` then fails with `not_found` ("target … is no longer open") until `targets --select` picks another (read from `sync_targets`, which drops the target, and `State::current`, which does not fall back to the main page).
- **A page is ready for input when its real document has loaded, is visible, and has painted two frames** (`READY_PROBE`, `probe_ready`, session/launch.rs). Before input, `State::ensure_ready` checks it for the target the input goes to, once per document: a target found ready is kept in `State::ready`, and a new default execution context for the target's main frame (`Runtime.executionContextCreated` with `auxData.isDefault` and `frameId` equal to the target id, in `collect_events`) removes it, so the next input checks again. The check waits up to 10 s, then fails with `timeout` and the last probe in `found`. The two traps behind this definition (a target that starts on `about:blank`, input dropped before the first frames) are launch-time findings and stay in [Launching a test instance](launching-a-test-instance.md).
  - Called before input by `click`, `type`, `key`, `hold`, `drag` and `wheel` on CDP and OS routes (`State::focus_then`, `State::click_cmd`, `State::drag_cmd`, `State::wheel_cmd`, session/commands.rs; session/measure.rs), and by the native and profiling commands that need a painting page (session/native.rs, session/profiling.rs). Not by `eval`, `wait`, `read`, `snapshot` or `capture`, which work on a page in any state.
- **A reload is seen in two ways, for two uses.** For input, the new execution context above. For a run that spans two commands, by `performance.timeOrigin` changing between its start and its end, which fails the run with kind `page_reloaded`, exit 8 (`Kind::Reloaded`, raised only by `measure` and by `trace`/`profile` stop; [Measuring the page](measuring-the-page.md), [Tracing the page](tracing-and-profiling.md)).
- **`Page` is the one place a command chooses its engine** (`State::page`, session/page.rs). On a plugin session it is the plugin's main webview, otherwise the current CDP target; `Page::eval` evaluates there and awaits a promise. `eval`, `wait` and `read` go through it and so answer on macOS too ([Reaching the page through the plugin](reaching-the-page-through-the-plugin.md)); every other page command calls `State::current` and is CDP only.
- **`eval` evaluates an expression on the page and returns its value as JSON** (`State::eval_cmd`), awaiting a promise, within the command's `--timeout` (`timeout_ms` set from it in main.rs). A promise resolving after 4 s is answered (pinned by `a_command_the_session_takes_longer_than_3_s_over_still_gets_its_answer`, tests/fixture.rs). A thrown exception is kind `js_error` (`evaluate_within`).
- **`wait` polls a condition on the page every 100 ms until it holds or `--for` passes** (default 10 000 ms; `State::wait_cmd`). The conditions: an element matching `--selector`, `--text` inside `document.body.innerText`, or `--expr` truthy. Each poll may take up to `EVALUATE_TIMEOUT` (15 s). A timeout fails with `timeout` and what was found; a JavaScript error fails at once with `js_error` instead of waiting out the time (pinned by `wait_holds_until_an_element_appears_and_times_out_on_one_that_never_comes` and `wait_on_a_broken_expression_fails_at_once_instead_of_waiting_out_its_timeout`, tests/fixture.rs).
- **Waiting for an event.** `State::expect_event` registers a one-shot waiter for one method on one target; `collect_events` hands the event to the first waiter that matches instead of logging it. It must be registered before the command that causes the event.
- **Size and count.** Nothing bounds the number of page targets followed: each listed page gets a connection. The text of `eval` and `wait --expr` is the caller's; its result comes back whole, within the message limit in [Speaking CDP](speaking-cdp.md).

## Code

- `crates/pokit/src/session/pages.rs` — `discover`, `collect_events`, `Waiter`, `State::expect_event`, `State::sync_targets`, `State::cdp`, `State::current`, `State::ensure_ready`
- `crates/pokit/src/session/page.rs` — `Page`, `Page::eval`, `PluginPage`, `State::page`
- `crates/pokit/src/session/commands.rs` — `State::targets_cmd`, `State::eval_cmd`, `State::wait_cmd`, `Condition`, `evaluate`, `evaluate_within`, `EVALUATE_TIMEOUT`
- `crates/pokit/src/session/launch.rs` — `READY_PROBE`, `probe_ready`
- `crates/pokit/src/session/mod.rs` — `Target`, `State::cdp_status`

## Reference behaviour

**None.** The readiness traps measured against WebView2 are in [Launching a test instance](launching-a-test-instance.md).

## Cross-cutting invariants

**None.**

## Blast radius

- [Sending input over CDP](sending-input-over-cdp.md) — it assumes the page it is handed is ready and current; a readiness check that passed too early lets keys be dropped while CDP still reports success.
- [Sending OS input](sending-os-input.md) — the OS route checks readiness through the same `focus_then`, and finds the window through the current page.
- [Finding elements on the page](finding-elements-on-the-page.md) — a selector resolves on the current page and a ref on the page its snapshot was taken on; changing what "current" means changes which page an element is looked for in.
- [Launching a test instance](launching-a-test-instance.md) — launch sets the main and current page and fills `ready`; both must keep the same definition of ready.
- [Collecting logs](collecting-logs.md) — log lines exist only for targets `sync_targets` connected and enabled `Runtime` on, and a waiter takes its event out of the log.
- [Measuring the page](measuring-the-page.md) and [Tracing the page](tracing-and-profiling.md) — they detect a reload by `timeOrigin`, not by the execution-context event readiness uses; a change to one does not change the other.
- [Reaching the page through the plugin](reaching-the-page-through-the-plugin.md) — `Page::eval` is the contract both engines answer; a new caller of `State::current` is CDP only and is refused on macOS.

## Known holes / open

- Nothing bounds the number of page targets, and so of connections, a session holds.
- A current page that closes leaves every later command failing until `targets --select`; whether it should fall back to the main page has not been decided.
- Only frames that are page targets are followed; an `<iframe>` inside a page is reached only through its parent's document.

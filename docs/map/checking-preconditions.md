# Checking preconditions

## What it is
`pokit doctor`: whether a debugging port pokit could use is there and answering, whether a WebView2 browser is behind it, and whether it has page targets. It runs against `--port` on its own, or against the running session's port, and fails with `check_failed` (exit 3) naming the checks that did not pass. It is answered on Windows only, and refused on macOS.

## Governing decisions
**None.** [ADR-0001](../adr/0001-commands-talk-to-a-session.md) makes `doctor` without `--port` a request to the session; it does not decide what is checked. The checks come only from the source.

## Design model
The rules come only from the source (`doctor.rs`, the `doctor` arms of `main::dispatch`, `State::handle`).

Input from outside: the DevTools endpoints `/json/version` and `/json/list` on a local port, read through `devtools::get_json`, which gives up after 5 s ([Speaking CDP](speaking-cdp.md)); nothing bounds the body's size there.

- **Three ways in** (`main::dispatch`): with `--port`, the command line runs the checks itself against that port, no session needed; without it and with a session in `Ready`, it sends `Request::Doctor`, and the session runs the same checks against its own debugging port (`State::handle`, `config.cdp_port`); without either, it answers a single failing `session` check, "no session; run launch or attach, or pass --port", without starting anything.
- **The checks** (`doctor::checks`): `port_reachable`, whether `/json/version` answers (detail `127.0.0.1:<port>`, or the error); `webview2`, whether its `Browser` starts with `Edg/` (detail the browser string), only when the port answered; `page_targets`, whether `/json/list` holds at least one target of type `page` (detail the count; `devtools::pages`), only when the list could be read.
- **Every check passing is success with the `checks` list; any failing is `check_failed` (exit 3)**, the message `failed: <names>` and the list in the error (`doctor::outcome`). Pinned by `doctor_and_capabilities_describe_the_platform` (tests/fixture.rs): exit 3 with no session, exit 0 and a passing `webview2` check on a launched fixture.
- **macOS refuses `doctor`**, with or without `--port`, before anything runs (`main::dispatch`, `support::refused`); `capabilities` says so ([Platform support](platform-support.md)). Pinned by `commands_not_built_on_macos_exit_unsupported` (tests/macos.rs).

## Code
- crates/pokit/src/doctor.rs — checks, outcome
- crates/pokit/src/main.rs — dispatch (the `Command::Doctor` arms), Command::Doctor
- crates/pokit/src/session/commands.rs — State::handle (the `Request::Doctor` arm)
- crates/pokit/src/devtools.rs — get_json, pages

## Reference behaviour
**None.** The checks were not compared with another tool's.

## Cross-cutting invariants
**None.**

## Blast radius
- [Speaking CDP](speaking-cdp.md) — `doctor` reads the DevTools endpoints through `devtools::get_json` and its 5 s limit; a change there changes what `port_reachable` reports.
- [The app's pages](the-apps-pages.md) — `page_targets` counts what `devtools::pages` lists as pages, the same filter the session uses to find its pages; a change to that filter changes both.
- [Platform support](platform-support.md) — `doctor` is refused on macOS by its own arm in `main::dispatch`, not by `support::answers`, so a change to which platforms answer `doctor` must change both.
- [The command-line contract](the-command-line-contract.md) — a failed check is `check_failed`, exit 3, the only command that raises it.

## Known holes / open
- On macOS `doctor` checks nothing; checking the Accessibility and Screen Recording permissions there is designed, not built. Tracked: #32.
- When `/json/list` cannot be read, the `page_targets` check is left out rather than failed, so `doctor` can pass on a port whose page list does not answer (`doctor::checks`).
- `doctor` does not check what `launch` itself needs on Windows: whether pokit is elevated, or the WebView2 runtime's version, which together decide whether the port opens ([Launching a test instance](launching-a-test-instance.md)).
- Off Windows and macOS, `doctor --port` runs its checks although `capabilities` says `doctor` is unsupported ([Platform support](platform-support.md)).

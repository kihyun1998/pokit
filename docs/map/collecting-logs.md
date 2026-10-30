# Collecting logs

## What it is
The session's log buffer and the `logs` command that reads it: the page's console output and uncaught errors, the launched app's stdout and stderr, and the session's own notes about what it did between commands. The session collects them as they happen, so an agent sees what the app did while no command was running. Built on Windows; macOS refuses `logs`.

## Governing decisions
- [ADR-0001](../adr/0001-commands-talk-to-a-session.md): a command that connected on its own would miss whatever happens between commands, console errors and backend output named first; the session exists partly so that this buffer can.

## Design model
That logs are collected between commands comes from ADR-0001; the rest comes only from the source (`session/logs.rs` and the places that push entries).

Input from outside: page console messages and exceptions over CDP, and the app's output lines. The buffer keeps at most `MAX_LOGS` (10 000) entries and drops the oldest first (`State::log`). Nothing bounds an entry's text: a console message, an exception description or an output line is kept whole. See Known holes.

- **An entry is `seq`, `t_ms`, `source`, `level`, `target`, `text`** (`LogEntry`, `State::logs_since`). `seq` counts from 1 across the session's life and is not reused when old entries are dropped; `t_ms` is milliseconds since the session's state was made; `target` is the page's URL where there is one.
- **Four sources push entries:**
  - `console`, from `Runtime.consoleAPICalled` on every page the session holds, the level being the call's `type` (`log`, `warning`, `error`, …) and the text its arguments joined by spaces, each a string's value, else its description (`pages::collect_events`, `remote_text`);
  - `exception`, from `Runtime.exceptionThrown`, level `error`, the text the exception's description or else its text (`pages::collect_events`, `exception_text` in session/commands.rs);
  - `backend-stdout` and `backend-stderr`, each line the launched app writes, level `info` (`start_over_cdp`); an attached session has no app output to read;
  - `session`, the session's own notes: the idle end and the app's exit (`watchdog`), each foreground give-back during launch (`launch::keep_foreground`), a page socket off the instance's port that it ignored (`State::sync_targets`), the clipboard not given back (`State::restore_clipboard`), and a failed clock alignment (`State::align_main_clock`).
- **Console and exception entries come from the pages the session is connected to**: `State::sync_targets` sends `Runtime.enable` on each page it connects, and the events reach the buffer through `pages::collect_events`. An event a command is waiting for (`State::expect_event`) goes to that command instead of the log; today only `Tracing.tracingComplete` is waited for.
- **`logs` returns the entries after `--since` and the number to pass next** (`State::logs_since`): `next` is the last entry's `seq`, or `--since` itself when there are none. Pinned by `page_errors_and_backend_output_reach_logs` (tests/fixture.rs): an uncaught page error and a backend line appear, and `--since <next>` returns none.
- **A failed launch prints the last five backend lines** (`State::backend_tail`), in `wait_ready`'s error when the app exits during launch ([Launching a test instance](launching-a-test-instance.md)).
- **Entries are stored as they came and masked when printed**: `State::handle` masks every answer, `logs` included ([Masking secrets](masking-secrets.md)). Pinned by `a_secret_is_masked_in_output_logs_and_the_run_record` (tests/fixture.rs), which finds `password=***` in `logs`.
- **The run record does not copy `logs` output** ([The run record](the-run-record.md)).
- **macOS refuses `logs`** ([Platform support](platform-support.md)). The macOS start pipes the app's output to null and connects no CDP events (`through_plugin::spawn_app`, `through_plugin::session`), so there would be nothing in the buffer but the session's own notes.

## Code
- crates/pokit/src/session/logs.rs — LogEntry, MAX_LOGS, State::log, State::backend_tail, State::logs_since
- crates/pokit/src/session/pages.rs — collect_events, remote_text, State::sync_targets (`Runtime.enable`, the off-port note), State::expect_event
- crates/pokit/src/session/mod.rs — start_over_cdp (the backend readers), watchdog, State::align_main_clock
- crates/pokit/src/session/launch.rs — keep_foreground (its note)
- crates/pokit/src/session/clipboard.rs — State::restore_clipboard (its note)
- crates/pokit/src/session/commands.rs — State::handle (the `Request::Logs` arm), exception_text

## Reference behaviour
**None.** What reaches `logs` was not compared with another tool's console capture.

## Cross-cutting invariants
**None.**

## Blast radius
- [Masking secrets](masking-secrets.md) — the buffer holds secrets in clear and relies on every answer being masked on the way out; a new path that prints entries without going through `State::handle` would print them.
- [The run record](the-run-record.md) — the record leaves the log out by the key `entries`; renaming it copies the whole buffer into every `logs` line.
- [Launching a test instance](launching-a-test-instance.md) — the backend lines come from the app's stdout and stderr being piped at start; a failed launch's message depends on them.
- [The app's pages](the-apps-pages.md) — console and exception entries exist only for pages the session has connected and enabled `Runtime` on; a change to discovery changes which pages are heard.
- [Speaking CDP](speaking-cdp.md) — every page's events arrive through one channel to `pages::collect_events`; a change to how events are delivered or matched to waiters changes what is logged.

## Known holes / open
- macOS has no console messages or page errors in `logs`. Tracked: #13.
- Nothing bounds the size of one entry's text.
- Whether a console message or error raised before the session connected to a page (a page that opens and logs at once) reaches the buffer is not measured.
- A session that reaches more than 10 000 entries between two `logs` loses the oldest without saying so; `logs` does not report that entries were dropped.

# The command-line contract

## What it is
What every pokit command prints and exits with, and how the command line turns its arguments into a request for the session. Every command prints exactly one JSON object on stdout, with `ok`, `command` and `engine`, and either the command's own fields or an `error` with a `kind` a caller branches on; the exit code follows from that kind. This note also holds `--timeout`, usage errors, the arguments the command line settles itself before sending (secret text, `--file`, relative paths), and the request's shape on the wire. How the command line finds and proves the session it sends to is [Starting the session process](starting-the-session-process.md).

## Governing decisions
**None.** [ADR-0001](../adr/0001-commands-talk-to-a-session.md) decides that every command but the ones that start a session is a request to the session, which is why there is a request shape at all; it does not decide the printed object, the kinds or the exit codes. Those come only from the source.

## Design model
The rules here come only from the source (`output.rs`, `request.rs`, `main.rs`); no record or maintainer's call fixes them.

Input from outside: the command line's arguments, parsed by clap; text read from stdin for `--secret` (read to its end, no bound, `main::secret_text`); a file read whole for `eval --file` (no bound, `main::dispatch`). The session's answer line is read with no bound (`client::request`). The session bounds the request line at 4 MiB ([Starting the session process](starting-the-session-process.md)).

**The printed object**

- **One JSON object per command, always** (`main::main`, `output::render`). It holds `ok`, `command` and `engine`; on success the command's fields beside them; on failure `error` with `kind`, `message` and whatever detail the failure carries (`Failure::with`), and an exit code from the kind. `main::main` adds `command` when an answer lacks it.
- **`command` is the request's name** (`CommandKind::name`): the command-line name for a single command (`click`, `eval`), and command and action joined by `_` for one with actions (`measure_start`, `trace_stop`, `clipboard_write`, `window_activate`, `native_pick`). `launch` and `attach` take their `Mode::name`.
- **`engine` says what reaches the page on this build**: `cdp` on Windows, `plugin` on macOS (`output::ENGINE`). It is fixed when pokit is compiled, not read from the session.
- **Help and version are clap's own text, not JSON** (`main::main` lets clap print them for `DisplayHelp`, `DisplayVersion` and `DisplayHelpOnMissingArgumentOrSubcommand`).

**Kinds and exit codes**

`output::Kind::exit_code`, with the doc comment beside it:

| Exit | Kind (`Kind::name`) |
|---|---|
| 0 | success |
| 1 | `error`, `session_exists`, `js_error` |
| 2 | `usage` |
| 3 | `check_failed` |
| 4 | `timeout` |
| 5 | `not_found`, `stale_ref`, `no_session` |
| 6 | `guard_refused` |
| 7 | `unsupported` |
| 8 | `page_reloaded` |
| 9 | `not_delivered` |

- **A failure with no kind of its own is `error`** (`From<String> for Failure`). A CDP failure is `timeout` when the CDP call timed out and `error` otherwise (`From<CdpError> for Failure`).
- **Where each kind is decided** is the territory that raises it: `no_session` and `session_exists` here and in [Starting the session process](starting-the-session-process.md); `unsupported` in [Platform support](platform-support.md); `check_failed` in [Checking preconditions](checking-preconditions.md); `stale_ref` in [Finding elements on the page](finding-elements-on-the-page.md); `guard_refused` and `not_delivered` in [Sending input over CDP](sending-input-over-cdp.md) and [Sending OS input](sending-os-input.md); `page_reloaded` in [Measuring the page](measuring-the-page.md) and [Tracing the page](tracing-and-profiling.md); `js_error` in [The app's pages](the-apps-pages.md).
- **Every failure to reach the session is `no_session` (exit 5)** (`main::send`): no session file in `Ready`, and every error `client::request` returns, including a listener that is not the session and the command line's own wait running out (`no answer from the session within <n>s`). A command the session is still running past that wait is therefore reported as `no_session`, not `timeout`.

**Usage errors**

- **A command line clap rejects is still one JSON object, with kind `usage` and exit 2** (`main::main`), its message clap's rendered error. Pinned by `a_usage_error_is_still_one_json_object_with_the_usage_code` (tests/fixture.rs). Its `command` is the first argument as typed (`std::env::args().nth(1)`), which is a flag when one comes before the command (`pokit --timeout 5 …`).
- **Numbers that JSON cannot carry are refused by clap, as usage errors**: `main::finite` (a finite number), `main::positive` (finite and above 0), `main::point` (`X,Y`, both finite) on `window move`, `window resize` and `drag`, and `drag --steps` in 1 to 1000.
- **Arguments missing past clap are `error` (exit 1), not `usage`**: `type` or `clipboard write` with no text, `--secret` or `--secret-env` (`main::secret_text`); an unset `--secret-env` variable; `eval` with neither an expression nor `--file`, or a file it cannot read; a `launch --env` that is not `KEY=VALUE` (`main::parse_env`). The session also raises `usage` itself for some requests it refuses ([Sending OS input](sending-os-input.md), [Reaching an app's native UI](reaching-native-ui.md)).

**`--timeout` and how long the command line waits**

- **`--timeout` is global, in seconds, default 30** (`Cli::timeout`). For a request the command line waits that long plus `Request::extra_wait` for the answer (`main::dispatch`): `wait`'s `--for` and `eval`'s own time-out (which is `--timeout` again, so `eval` waits up to twice it); for `hold`, its `count` × `interval` plus `HOLD_ACK_WAIT` (15 s), and with `--compare` that plus `COMPARE_SETTLE` (10 s), twice; for `measure stop`, `--quiet` plus `--ceiling`; for `trace stop`, `TRACE_STOP_WAIT` (60 s). `launch` and `attach` wait up to `--ready-timeout` plus `--timeout` for the session to be ready, then up to `--timeout` for its status ([Starting the session process](starting-the-session-process.md)).

**What the command line settles before sending**

- **Secret text** (`main::secret_text`): `--secret-env NAME` reads the variable, else `--secret` reads stdin to its end and trims the trailing CR and LF, else the positional text; the first two mark the request secret (`Request::secret`). What masking does with it is [Masking secrets](masking-secrets.md).
- **`eval --file`** is read by the command line, and its text sent as the expression (`main::dispatch`).
- **Relative paths are made absolute against the command line's working directory** (`main::absolute`), for `capture --out` and `native pick`, because the session runs elsewhere. Pinned by `a_relative_capture_path_is_relative_to_where_the_command_ran` (tests/fixture.rs).
- **`launch --port` and `--data-dir` are refused on macOS before any session starts** ([Platform support](platform-support.md)).

**The request on the wire**

- **A request is `{"command": <name>, "args": {…}}`** (`Request`, `serde(tag = "command", content = "args")`), the name being its `CommandKind::name`. Pinned by `the_wire_name_of_each_request_is_its_kind_name` (request.rs), which round-trips one of every request. `client::request` sends it as `{"token": …, "request": …}` on one line and reads `{"exit": …, "output": …}` back, the exit code and printed object the session decided (`State::handle`).
- **`ping` and `status` are requests with no command of their own**: `client::alive` and `main::start_session` use them, and they are not recorded (`CommandKind::is_public`); `capabilities` does not list them, as it lists clap's subcommands and they have none.
- **`route` defaults to the platform's own when absent** (`Route`, `serde(default)`): `cdp` off macOS, `plugin` on macOS.

## Code
- crates/pokit/src/output.rs — Kind, Kind::name, Kind::exit_code, Failure, Failure::new, Failure::with, Fields, Outcome, fields, ENGINE, render
- crates/pokit/src/request.rs — Request, Request::kind, Request::extra_wait, Request::secret, Request::route, Request::sends_os_input, Request::checks_keydowns, CommandKind, CommandKind::name, Route, Route::name, HOLD_ACK_WAIT, TRACE_STOP_WAIT, COMPARE_SETTLE
- crates/pokit/src/main.rs — Cli, Command, main, dispatch, send, outcome_json, secret_text, finite, positive, point, parse_env, absolute
- crates/pokit/src/client.rs — request (the request and answer lines)

## Reference behaviour
**None.** The printed object and the exit codes were not compared with another tool's.

## Cross-cutting invariants
**None.**

## Blast radius
- [Starting the session process](starting-the-session-process.md) — the command line's wait is `--timeout` plus `extra_wait`, and running out of it reads as `no_session`; a session command that waits longer than that (`HOLD_ACK_WAIT`, `TRACE_STOP_WAIT`, a page wait) is reported as a lost session although it ran.
- [The run record](the-run-record.md) — the record keeps each command's printed object and exit code as `render` made them, and its `args` are the request's `args` on the wire; a change to either shape changes every record line.
- [Masking secrets](masking-secrets.md) — only `Request::secret` tells the session which text to mask; a new command taking secret text must be added there, or its text is printed and recorded.
- [Platform support](platform-support.md) — every refusal is `unsupported`, exit 7, which callers and tests branch on (`UNSUPPORTED`, tests/macos.rs).
- [Testing against the fixture app](testing-against-the-fixture.md) — the tests read exit codes and `error.kind` from this one object; a change to a kind's code or name reddens them across the suite.

## Known holes / open
- `no_session` covers both "there is no session" and "the session did not answer in time"; a caller cannot tell a lost session from a slow command by kind alone, only by the message.
- Arguments missing past clap exit 1 (`error`) where clap's own refusals exit 2 (`usage`); whether that difference is intended is not recorded.
- Nothing bounds the stdin a `--secret` reads, the file `eval --file` reads, or the answer line the command line reads.

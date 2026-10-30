# The run record

## What it is
The folder each session writes as it runs: `record.jsonl`, one JSON line for the session's start, for every command it answers and for its end, and beside it the files commands leave there, a screenshot where a command failed, captures, traces and CPU profiles. It is what an agent or a person reads afterwards to see what a session did, on Windows and macOS alike.

## Governing decisions
**None.** [ADR-0001](../adr/0001-commands-talk-to-a-session.md) is why one process sees every command and can keep one record of them; it does not decide what the record holds. That comes only from the source.

## Design model
The rules come only from the source (`session/record.rs`, `serve`, `State::handle`, `State::shutdown`).

Input from outside: every command's printed object goes into the record whole (the `logs` entries excepted, below). Nothing bounds a line's size, the file's size, or how many run folders accumulate under `runs/`; see Known holes.

- **One folder per session, `runs/<stamp>-<session pid>` under the pokit home** (`serve`, `home::stamp`, `home::home`), made before the start is tried, so a start that fails leaves an empty one. `launch` and `attach` report it as `record_dir` (`State::status`).
- **A line is `seq`, `t_ms`, `command`, `args`, `exit`, `output`** (`State::record`): `seq` counts from 1, `t_ms` is milliseconds since the session's state was made (`State::started`), and the file is opened for append on each line.
- **The first line is the start** (`serve`): `launch` or `attach`, with `exe`, `args`, `env_keys` and `main_url`, and the status the command line was given as its output. Only the names of `--env` pairs are kept, not their values. Pinned by `the_run_record_starts_with_the_launch_and_holds_no_copy_of_the_logs` (tests/fixture.rs).
- **Every public command is recorded after it is answered** (`State::handle`, `CommandKind::is_public`: every request but `ping` and `status`), its `args` the request's `args` on the wire and its output the object printed, both masked; a secret's own text is replaced by `***` in `args` ([Masking secrets](masking-secrets.md)). A request the session cannot parse is recorded too, as `unknown command`, its `args` as received (`serve`).
- **`logs` output is not copied**: its `entries` become `"<n> entries, not copied"` in the record (`State::handle`), so the record does not grow with the log buffer a second time. Pinned by the same test, which checks the fixture's backend line is not in the record.
- **The last line is `shutdown`** (`State::shutdown`), with the mode and, for a launched session, how the app was ended (`kill`) and whether the profile was left (`profile_left`). A session killed outright writes none ([Starting the session process](starting-the-session-process.md)).
- **A failed command of certain kinds leaves a screenshot, `fail-<n>.png`, and its path in `error.screenshot`** (`State::failure_screenshot`, called from `State::handle`). The kinds are `read`, `click`, `type`, `key`, `hold`, `drag`, `wheel`, `wait`, `eval` and `snapshot` (`CommandKind::screenshots_failure`). It is a CDP `Page.captureScreenshot` of the current page (`record::screenshot_to`), so the plugin's session, which has no current CDP page, leaves none ([Reaching the page through the plugin](reaching-the-page-through-the-plugin.md)). A command carrying a secret leaves none, so a typed secret is not photographed. Pinned by `a_failed_check_is_recorded_with_a_screenshot` and `a_failed_secret_type_leaves_no_screenshot` (tests/fixture.rs).
- **Files a command leaves are numbered after the record line it will get**: the record's last `seq` plus one, read when the file is named. `fail-<n>.png` (`State::failure_screenshot`), `capture-<n>.png` for a `capture` without `--out` (`State::capture_path`, also for `capture --window`), `trace-<n>.json` and `profile-<n>.cpuprofile` (`State::record_file`). What each holds is [Capturing the page](capturing-the-page.md), [Capturing a window with its native UI](capturing-a-window.md), [Tracing the page](tracing-and-profiling.md) and [Profiling the page](profiling-the-page.md).
- **The macOS session writes the same record**, through the same `State`. Pinned by `launch_reports_the_session_and_the_run_record_keeps_each_command` (tests/macos.rs): `launch`, `eval`, `close`, then `shutdown`.

## Code
- crates/pokit/src/session/record.rs — State::record, State::failure_screenshot, screenshot_to
- crates/pokit/src/session/mod.rs — serve (the record folder, the start line, the unknown-command line), State::shutdown (the last line), State::status (`record_dir`), State::started, State::record_dir, State::record_seq
- crates/pokit/src/session/commands.rs — State::handle (the line per command, the `logs` cut, the failure screenshot), State::capture_path
- crates/pokit/src/session/profiling.rs — State::record_file
- crates/pokit/src/request.rs — CommandKind::is_public, CommandKind::screenshots_failure
- crates/pokit/src/home.rs — stamp, home

## Reference behaviour
**None.** The record was not compared with another tool's.

## Cross-cutting invariants
**None.**

## Blast radius
- [Masking secrets](masking-secrets.md) — the record is masked line by line in `State::handle`; the start line, the `shutdown` line and the unknown-command line are written elsewhere and are not passed through `redact`, so anything a secret could reach there is written as it is.
- [The command-line contract](the-command-line-contract.md) — each line keeps the printed object and the request's wire `args`; a change to either shape changes what a reader of old records finds.
- [Collecting logs](collecting-logs.md) — the record leaves the log out on purpose; a command that returned log text under another key than `entries` would copy it in.
- [Capturing the page](capturing-the-page.md), [Tracing the page](tracing-and-profiling.md), [Profiling the page](profiling-the-page.md) — their default file names assume the next record line is theirs; a command recorded in between takes that number.
- [Reaching the page through the plugin](reaching-the-page-through-the-plugin.md) — the failure screenshot is CDP only; when macOS capture is built, the plugin's session still leaves none until `State::failure_screenshot` uses it.
- [Testing against the fixture app](testing-against-the-fixture.md) — tests read the record and every file under `runs/` (`Pokit::all_written_text`) to check what pokit wrote.

## Known holes / open
- Two commands answered at the same time can read the same `seq` and give their files the same number, the later overwriting the earlier: the number is read when the file is named, not reserved (`State::record_file`, `State::capture_path`, `State::failure_screenshot`). From reading the code; not tested.
- Nothing removes old run folders; `runs/` grows with every session.
- Nothing bounds the size of a record line or of `record.jsonl`: every command's output is copied whole except `logs`.
- The unknown-command line keeps the request's `args` unmasked: a request with secret text that the session cannot parse (a command line newer than the session, say) would be written with it. From reading the code; not tested.
- macOS leaves no failure screenshot until capture is built there. Tracked: #35.

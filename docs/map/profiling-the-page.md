# Profiling the page

## What it is
How `profile start` / `profile stop` sample the current page's JavaScript with V8's CPU profiler on Windows, save the profile to the run record, and rank self time by function and by file, with the profiler's idle, program and garbage-collector time reported apart. On macOS `profile` is refused: there is no scripted path to WebKit's profiler (`NO_TIMELINE`, support.rs, #1).

## Governing decisions
- [ADR-0001](../adr/0001-commands-talk-to-a-session.md): a profile lives in the session between `start` and `stop` (`State::profiling`), on the connection the session holds.

[ADR-0002](../adr/0002-hand-written-cdp-client.md) is adjacent: it decides that pokit speaks CDP through its own client, which `Profiler.*` goes through, not how a profile is taken or summarised.

## Design model
The rules come only from the source and from issues #4 and #22; no maintainer's call is recorded for profiling. The rules a profile shares with a trace — saved in the run record, a reload voids the run, state cleared only once the browser's stop succeeds, one `start` at a time with pokit's own refusal, the files not masked, nothing cleaning up a recording left running — are written once in [Tracing the page](tracing-and-profiling.md); this note holds what is the profile's own. Size: nothing bounds how long a profile runs. `Profiler.stop` returns the whole profile in one CDP reply, which the session holds as one JSON value and writes out whole (`State::profile_stop_cmd`); the CDP client's message limit is what caps it ([Speaking CDP](speaking-cdp.md)).

- **Starting takes three calls:** `Profiler.enable`, `Profiler.setSamplingInterval` with `--interval-us` (default 100 µs, main.rs), and `Profiler.start` (`State::profile_start_cmd`). `--interval-us 0` is refused. `stop` sends `Profiler.stop`, then `Profiler.disable` without minding its answer.
- **The file is `profile-<n>.cpuprofile`**, the `Profiler.Profile` as V8 returned it, which opens in the DevTools profiler. How it is placed and numbered in the run record is in [Tracing the page](tracing-and-profiling.md).
- **A profile's self time comes from sample spans, not hit counts.** Each sample runs from its own time to the next sample's, and the last one to the profile's end (`profile::summarize`; `each_sample_runs_until_the_next_and_the_last_until_the_end`).
- **Functions are named `name @url:line:col`, counted from 1, as DevTools shows them** (`profile::location`), so a minified build still points at a place. A function with no name is `(anonymous)`; one with no URL is its bare name and its time goes to the file `(native)` (`files_add_up_their_functions_and_native_code_has_no_file`). `--top` (default 20) keeps that many functions and files (`profile::ranked`; `only_the_top_entries_are_kept`). Pinned on the fixture by `a_profile_of_a_stalling_page_names_the_stalling_function` (tests/profile.rs), which requires `stallOnKey @…:line:col` among the top 3.
- **`(idle)`, `(program)` and `(garbage collector)` are reported apart from the page's functions** as `idle_ms`, `program_ms` and `gc_ms` (`profile::SPECIAL`); `(root)` is left out.
- **A second `start` would be refused by V8 too, but unclearly:** V8 says "Cannot change sampling interval when profiling", which does not say a profile is running. pokit refuses first, with what to do ([Tracing the page](tracing-and-profiling.md)).
- **A profile left running ends with the session's connection.** V8's session destructor disables the profiler agent, which stops its profiles (`v8-inspector-session-impl.cc` 164–173, `v8-profiler-agent-impl.cc` 234–248). Why nothing in pokit cleans it up is in [Tracing the page](tracing-and-profiling.md). No test holds this for a profile, unlike the trace's `closing_an_attached_session_ends_its_trace`.

## Code
- crates/pokit/src/session/profiling.rs — State::profile_start_cmd, State::profile_stop_cmd, write
- crates/pokit/src/profile.rs — SPECIAL, summarize, location, ranked, list
- crates/pokit/tests/profile.rs — a_profile_of_a_stalling_page_names_the_stalling_function, and the shared tests named in [Tracing the page](tracing-and-profiling.md)

## Reference behaviour
- **V8 stops a detached session's profiles:** `v8-inspector-session-impl.cc` 164–173 and `v8-profiler-agent-impl.cc` 234–248, as above.
- **Ten keys on the fixture, 60 ms stall each:** the profile gave `stallOnKey` 329 ms of self time and the native `now` it calls 261 ms (2026-10-06).

## Cross-cutting invariants
**None.**

## Blast radius
- [Tracing the page](tracing-and-profiling.md) — the shared code there (`check_same_document`, `State::record_file`, `time_origin`) and the start-lock shape are used by both; a change made for one recording changes the other.
- [Speaking CDP](speaking-cdp.md) — the whole profile arrives as one `Profiler.stop` reply and must fit the client's message limit; a long profile that outgrows it would fail `stop`.
- [Masking secrets](masking-secrets.md) — masking assumes a saved profile holds no typed text; it holds function names and script URLs with query strings.

## Known holes / open
- **Nothing bounds a profile's length or size**, and the profile is held and written whole in memory; the length at which `Profiler.stop`'s reply outgrows the CDP client's message limit, and what then happens to the connection and the profile, is unmeasured.
- **No test shows a profile ending when an attached session closes**; it rests on V8's source as cited.
- **The `start` lock was never seen to be needed for `profile`** (0 in 20 tries without it, 2026-10-08, #22); it is there because the code has the trace's shape.
- **macOS has no profile:** refused (`NO_TIMELINE`, #1).

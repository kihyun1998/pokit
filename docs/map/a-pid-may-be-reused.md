# A pid may be reused

## The fact

pokit never kills a process, or removes what belongs to one, on a pid alone: before it acts on a pid
it recorded earlier, it checks that the pid still names the process it recorded, or it does not
act. A pid outlives its process — after a reboot, or once the process has exited and been reaped —
and may then be the user's own process.

## Why it is cross-cutting

Three sites act on a recorded pid, each with its own check, and none calls another or a shared
primitive for the check:

- `crates/pokit/src/main.rs — clear_stale_session` kills a stale session's orphaned app only when the
  image name matches the recorded executable (`proc::image_name`) *and* the process start time matches
  the one recorded at launch (`proc::process_start_time`, `SessionInfo::app_started`).
- `crates/pokit/src/session/mod.rs — State::shutdown` does not kill the app's pid once the app has
  exited and been reaped (`app_exited`, set by `watchdog`).
- `crates/pokit/src/session/launch.rs — sweep_profiles` treats a profile `<stamp>-<pid>` as ended, and
  removes it, only when its pid is not a running pokit process (`proc::image_name` compared with pokit's
  own), so a pid another pokit took keeps its profile.

The process helpers they use (`image_name`, `process_start_time`, `kill_tree`) only read or act; the
decision that a pid is still the same process is made at each site. Loosening one — image name alone,
or killing without the `app_exited` check — kills or deletes something of the user's while the
others stay safe.

## Territories it holds in

- [Starting the session process](starting-the-session-process.md) — `main::clear_stale_session`,
  `State::shutdown`.
- [Launching a test instance](launching-a-test-instance.md) — `launch::sweep_profiles`.

## What a violation looks like

`pokit launch` after a reboot kills the user's own copy of the app, which took the recorded pid; or a
session ending after its app exited kills an unrelated process; or the sweep deletes the profile of a
session still running. Each needs pid reuse, so it shows rarely and never in a test run that does not
force it.

## Discovery history

- The stale-session rule names its reason: a pid can be reused, for example after a reboot, by the
  user's own copy of the same app, and image name alone would get that copy killed
  ([Starting the session process](starting-the-session-process.md)); pinned by
  `a_stale_session_file_never_gets_a_process_pokit_did_not_start_killed` (tests/fixture.rs).
- Off Windows `image_name` and `process_start_time` return `None`, so the stale check never matches
  and a macOS orphan is never cleared — the rule holds by not acting (see that note's Known holes).
- Named as an invariant from the map rewrite's reading of the three sites (2026-10-30); the maintainer
  chose to record it (2026-10-30).

## Where it will recur

A function that kills, signals or deletes something by a pid it did not just get from a live handle
is subject to this: it checks the pid still names the recorded process (image and start time), or it
does not act.

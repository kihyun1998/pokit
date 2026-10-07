# Testing against the fixture app

How the integration tests run pokit against the fixture app: most on one instance their test binary shares, the rest on their own, one test of a binary at a time (#58). Code: `shared_fixture`, `Shared`, `Turn`, `Pokit::new`, `Pokit::launch_fixture` (crates/pokit/tests/common/mod.rs).

## Design model

- **Tests that only read and act inside the page share one instance per test binary; the rest launch their own.** **Maintainer's call (2026-10-07, #58):** they were shown that each test launched its own instance, said to be 27 at once in `tests/fixture.rs` (the harness runs as many tests at once as there are logical processors, 16 here, so 16 at most, not measured), with the flaky failures (#47), the windows on their desktop and the leftovers (#56) that came with it, and the options of sharing one instance per file, capping how many run at once, or mixing the two. They asked why not one app, were shown which tests cannot share (below) and what sharing needs, and chose one shared instance per test file with only those tests on their own.
- **A test keeps its own instance when it is about the instance's life or the session's record**: launch and close, the idle end, a session dying or left stale, profiles, attaching, the run record and logs, secrets, the foreground at launch, native UI, OS input, windows, the clipboard. These read what a shared session has accumulated from other tests, or change what a reload does not undo.
- **One test of a test binary runs at a time** (`Turn`): a shared session has one page, and tests driving it at once would mix their focus and events. Every `Pokit::new` takes the turn, and `shared_fixture` takes it too, so at most the shared instance and one of a test's own are up. A thread that holds the turn takes it again without waiting, for a test that makes a second `Pokit`.
- **Between tests the shared instance is reloaded, scrolled to the top and checked** (`reset`): any measurement, trace or profile is stopped, the page is reloaded and waited for by its `timeOrigin`. When the last test left another window, a dialog or a context menu, or the session ended, the instance is launched again instead.
- **The shared instance is closed, and its home removed, when its test binary exits** (`atexit`, `close_shared`); a static is never dropped, so nothing else would. Its session also ends after 20 s without commands (`SHARED_IDLE`), for a binary that aborts, where `atexit` does not run. Reporting on a failing test never panics again (`eval_quietly`), since a panic while one is unwinding aborts.
- **A second handle on a home leaves it alone** (`Pokit::on_home`): the `wait` test drives its instance from a second thread, and that handle's drop used to remove the home, which on the shared instance is the running session's.
- **Traps in the turn:** a `Pokit` dropped on a thread other than the one that made it would leave the turn taken for the rest of the binary, since the turn is counted per thread; a thread spawned by a test that calls `Pokit::new` waits for its own parent forever; and `shared_fixture` twice in one test waits for itself. No test does any of these.

## Measured

- **Before and after, two full runs each (2026-10-07, 16 logical processors):** before, 155 s and 243 s, no failures; after, 218 s and 207 s, no failures, and at most 3 fixture processes up at once (sampled every 0.5 s). Earlier full runs of the old structure that day failed 3–4 fixture tests a run, a different set each time (#47); two clean runs on each side do not show the failure rate has changed.
- **A shared instance left running holds the fixture's executable,** and the next test binary, which builds the fixture when it starts (`build_fixture`), failed with "failed to remove file … pokit-fixture.exe … access denied". Closing it at exit is what lets the binaries follow each other.
- `input_carries_its_send_time_on_the_page_clock` now reads a clock aligned at an earlier test's launch; it asserts only that the times are not negative, so it holds either way.

## Testing

- Skipping the reload between tests turned `input_is_refused_when_focus_is_outside_the_required_target` and `refused_input_leaves_focus_where_it_was` red (2026-10-07): the reset is load-bearing.
- Not tested: relaunching after a test left another window or a dialog, and the at-exit close beyond the measurement above.

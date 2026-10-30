# Capabilities match behaviour

## The fact

What `pokit capabilities` declares for a command is what the command does on that platform: whether
it is answered, which input routes it has and which is the default, whether each route or action
takes the user's focus (`takes_focus`), and the route-specific traits it names (`ime_composition`,
`leaves_the_page`, `html5_drag`, `right_click_menu`, `native`'s foreground note, `capture`'s
`window.native_ui`). An agent picks a route, or decides whether it may run a command while a person
uses the machine, from this output alone (#1, story 42).

## Why it is cross-cutting

The declarations are hand-written values, in `crates/pokit/src/support.rs — routes` and
`crates/pokit/src/main.rs — capabilities`; nothing derives them from the handlers. The behaviour they
describe lives in each territory's own code, which never reads them. Only which commands are
answered comes from one list (`support::ON_MACOS`, read by `answers`, `plugin_answers` and
`session_answers`), so that part cannot drift; every other value can. A change to a command's
focus, route or delivery behaviour made without the matching declaration — or a declaration changed
without the behaviour — leaves `capabilities` telling an agent something false.

## Territories it holds in

- [Platform support](platform-support.md) — the declarations: `support::routes`,
  `support::route_refused`, `main::capabilities`.
- [Sending input over CDP](sending-input-over-cdp.md) — the cdp route: `takes_focus: false`,
  `ime_composition: true`, `leaves_the_page: false`, `html5_drag: "always"`, `right_click_menu`.
- [Typing Korean through IME](typing-korean-through-ime.md) — `ime_composition` per route.
- [Sending OS input](sending-os-input.md) — the os route: `takes_focus: true`, `needs` the app in
  front, `leaves_the_page: true`, `html5_drag`; `window activate` takes focus.
- [Measuring the page](measuring-the-page.md) — `hold`'s routes, and `hold --compare`, which needs
  the app in front.
- [Placing the app's windows](placing-windows.md) — `window move` and `resize`: `takes_focus: false`.
- [Capturing a window with its native UI](capturing-a-window.md) — `capture --window`:
  `takes_focus: false`, `needs` a launched session.
- [Reaching an app's native UI](reaching-native-ui.md) — `native`'s `takes_focus` text and `tray`
  limit.
- [Sending input on macOS](sending-input-on-macos.md) — the plugin route: `takes_focus: false`,
  `ime_composition: false`, `right_click_menu`; `--route os` refusals.

## What a violation looks like

An agent runs a command declared `takes_focus: false` while the user works, and the user's foreground
or cursor moves; or it avoids a route that would have worked; or it picks `--route os` for a drag the
declaration says reaches the page, and the drop never arrives. Nothing fails: the declaration and the
behaviour each pass their own tests.

Seen at HEAD b316a02 (read 2026-10-30, map rewrite; left as they are):

- `native` and `clipboard` on Windows declare `route: "cdp"` (`output::ENGINE`; `clipboard` through
  the generic arm of `main::capabilities`), though both go through Win32, not CDP.
- `hold --compare` sends OS input and needs the app in front (`State::hold_compare_cmd`); `hold`'s
  declaration gives the `os` route's needs, not that `--compare` takes them whatever `--route` says.
- `window` declares no `needs: "a launched session"`, though `activate`, `move` and `resize` refuse an
  attached session with exit 7; `capture.window` does declare it.
- On a platform other than Windows and macOS, `doctor` is declared unsupported (`support::answers`),
  but `doctor --port` runs its checks there (the `Doctor` arms of `main::dispatch`).

## Discovery history

- `capabilities_say_which_routes_take_focus` (tests/os_input.rs) and the capabilities checks in
  tests/macos.rs pin the declared values; `doctor_and_capabilities_describe_the_platform`
  (tests/fixture.rs) pins the shape. No test compares a declared value with the observed behaviour;
  the behaviour is pinned separately in each territory (the foreground tests in
  [Launching a test instance](launching-a-test-instance.md), [Reaching an app's native UI](reaching-native-ui.md),
  [Placing the app's windows](placing-windows.md)).
- Named as an invariant from reading `support.rs` beside the handlers (map rewrite, 2026-10-30).

## Where it will recur

A change that alters whether a command moves the user's focus or cursor, adds or removes a route,
changes the default route, or changes what a route can reach (a composition, a drop, leaving the
page) is subject to this: change `support::routes` or `main::capabilities` in the same change, and
the capabilities test that pins it.

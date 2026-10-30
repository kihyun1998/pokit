# Platform support

## What it is
Which commands each platform answers, how a command is refused where it is not, which input routes each command has and which is the default, and what `capabilities` reports about all of that. Windows answers every command; macOS answers a short list through the app's plugin; any other platform answers only `capabilities`. A refusal is always the `unsupported` kind with exit 7 and the platform named.

## Governing decisions
**None.** [ADR-0001](../adr/0001-commands-talk-to-a-session.md) decides that commands are requests to a session, not which platform answers which; the route names and what `capabilities` must say are maintainer's calls on issues, carried in the territories that made them ([Sending input on macOS](sending-input-on-macos.md) for `--route plugin`, #33; [Sending OS input](sending-os-input.md) for `takes_focus`, #1 story 42).

## Design model
The rules come from the source (`support.rs`, `main::capabilities`) and from the maintainer's calls linked above; this note adds none of its own.

Input from outside: none beyond the command's name and `--route`, both fixed sets (clap's subcommands, `Route`).

**Who answers what**

- **Windows answers every command** (`support::answers` returns true under `cfg!(windows)`).
- **macOS answers the commands in `ON_MACOS`**: `capabilities`, `launch`, `close`, `eval`, `read`, `wait`, `click`, `key`, `type` (`support::ON_MACOS`). Everything else is refused: `trace` and `profile` with `NO_TIMELINE` ("there is no scripted path to WebKit's Timeline or profiler (#1)"), every other command with "not built on macOS yet (#6)" (`support::reason`).
- **Any other platform answers only `capabilities`**; the rest is refused with "only the Windows and macOS backends exist" (`support::reason`).
- **A request kind belongs to the command before its first `_`** (`support::command_of`: `trace_start` → `trace`, `window_activate` → `window`), so a command with actions is answered or refused as a whole. Pinned by `a_request_kind_belongs_to_its_command` (support.rs).

**Where a command is refused**

Refusals happen at several places that each ask `support.rs`, not one gate:

- **On the command line, before any session is reached**: `main::start_session` for `launch` and `attach` (`support::answers`); `main::send` for every request (`support::session_answers`, which also lets `ping` and `status` through); the `doctor` arm of `main::dispatch` on macOS ([Checking preconditions](checking-preconditions.md)); and `launch --port` or `--data-dir` on macOS, because they name WebView2's debugging port or profile (`main::dispatch`).
- **In the session**: `State::handle` refuses a request the plugin's session does not answer (`support::plugin_answers`), and an input route the session cannot take (`support::route_refused`): the plugin's session takes only `plugin`; a CDP session takes `cdp` and `os` but not `plugin` (`support::plugin_route_on_cdp`, also used by `measure`'s, `drag`'s and `wheel`'s own route checks). On macOS an `os` click is refused as having no OS click that leaves the cursor alone, and other `os` input as not built.
- **Every refusal is the same failure**: kind `unsupported`, exit 7, a `platform` field with `std::env::consts::OS`, and the reason as the message (`support::refused`, `support::route_refused`).
- Pinned by `commands_not_built_on_macos_exit_unsupported` (tests/macos.rs), which checks `capabilities` and the refusal of `attach`, `doctor`, `snapshot`, `trace`, `profile`, `hold` and `launch --port`/`--data-dir`, without a session and with one; `trace_and_profile_carry_the_timeline_reason_by_either_name` and `macos_clicks_through_the_plugin_and_names_no_os_route` (support.rs).

**Routes**

- **The default route is the platform's own**: `cdp` off macOS, `plugin` on macOS (`request::Route`, chosen when compiled). What each route does is its territory's: [Sending input over CDP](sending-input-over-cdp.md), [Sending OS input](sending-os-input.md), [Sending input on macOS](sending-input-on-macos.md).
- **`support::routes` names, per input command and platform, each route, which one is the default, and whether it takes the user's focus**, with what an OS route needs (`window activate`), whether `type` composes through an IME, what a right-click does to the engine's menu, and for `drag` whether the path may leave the page and which HTML5 drags it reaches. On Windows: `click`, `key`, `type`, `hold`, `wheel`, `drag` have `cdp` and `os`. On macOS: `click`, `key`, `type` have `plugin` only.

**`capabilities`**

- **`capabilities` lists every command clap defines, except the hidden `__session`** (`main::capabilities` walks `Cli::command()`'s subcommands), with `platform` beside them. It needs no session and is answered on every platform.
- **Each command gets one of these shapes** (`main::capabilities`): `supported: true` and its `routes` for an input command; `supported: true` and `route` (`output::ENGINE`) for any other answered command; `supported: false` and the `reason` from `support::reason` for a refused one. Three are spelled out by hand: `window` with `takes_focus` per action (`activate` takes it, `move` and `resize` do not); `native` on Windows with what holds the foreground and that only `tray-icon` trays are reached; `capture` on Windows with what `--window` does (native UI, no focus taken, needs a launched session).
- What `capabilities` declares is what each command must do: [Capabilities match behaviour](capabilities-match-behaviour.md). Pinned by `capabilities_say_which_routes_take_focus` (tests/os_input.rs) and `doctor_and_capabilities_describe_the_platform` (tests/fixture.rs).

## Code
- crates/pokit/src/support.rs — ON_MACOS, NO_TIMELINE, command_of, answers, plugin_answers, session_answers, routes, reason, route_refused, plugin_route_on_cdp, refused
- crates/pokit/src/main.rs — capabilities, start_session, send, dispatch (the `doctor` arm on macOS and `launch`'s WebView2-only flags)
- crates/pokit/src/session/commands.rs — State::handle (the plugin and route refusals)
- crates/pokit/src/request.rs — Route
- crates/pokit/src/output.rs — ENGINE

## Reference behaviour
**None.** No other tool's capability report was compared.

## Cross-cutting invariants
- [Capabilities match behaviour](capabilities-match-behaviour.md) — here at `main::capabilities` and `support::routes`, which declare what the commands do, and at the refusals in `support::answers`, `support::session_answers` and `support::plugin_answers`, which must agree with `supported`.

## Blast radius
- [Capabilities match behaviour](capabilities-match-behaviour.md) — every route, `takes_focus` and `supported` written here is a promise about another territory's code; a change to how a command sends input or whether it takes focus must change `support::routes` too.
- [Reaching the page through the plugin](reaching-the-page-through-the-plugin.md) — `ON_MACOS` is the list of what the plugin session answers; a command built through the plugin stays refused until it is added, and one added before it is built runs code written for CDP in a session that has no CDP connection.
- [Sending input on macOS](sending-input-on-macos.md) — the macOS `routes` and the `os` refusal messages describe what the plugin's input does; they move together.
- [Sending OS input](sending-os-input.md) — the `os` route's `takes_focus` and `needs` say what OS input requires of the user's screen.
- [The command-line contract](the-command-line-contract.md) — a refusal is `unsupported`, exit 7; callers branch on that code.
- [Starting the session process](starting-the-session-process.md) — `launch` and `attach` are refused before a session starts, so a platform that does not answer them leaves no session file.

## Known holes / open
- `capabilities` and the commands disagree in a few places, from reading the code (none is tested): off Windows and macOS, `capabilities` says `doctor` is unsupported, yet `doctor --port` runs its checks there, because `main::dispatch` refuses `doctor` only on macOS; on Windows `native` and `clipboard` are declared with `route: "cdp"` (`output::ENGINE`), though they go through Win32, not CDP; `hold --compare` sends OS input and needs the app in front (`Request::sends_os_input`), which `hold`'s declared routes do not mention.
- Most commands are refused on macOS. Tracked: #6, and per command #13 (logs), #14 (snapshot), #15 (measure), #32 (doctor), #35 (capture), #36 (clipboard), #38 (hold), #113 (`key` and `type` on `--route os`), #119 (native).

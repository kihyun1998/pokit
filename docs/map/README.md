# The territory map

<!-- grill-map build stamp: d525035 -->
<!-- grill-map synced at: b316a02 (2026-10-26) -->

The territory map. A comment's why, trap and measured value go to a note here.
Before starting a note, search this folder and the code symbol for one that
already holds it; name a new note for what the system does there, not for the file.
Notes are added as they come up, or all at once with `grill-map`, which also
reads hand-written notes as leads and keeps their paths.

## The two questions

- **If I touch this, what else moves?** Open the territory the change lands in and walk its
  `## Blast radius` and `## Cross-cutting invariants`.
- **What decision is this shape derived from?** Read its `## Governing decisions`, then the
  maintainer's calls written with their date and issue in `## Design model`.

## How to read it

- `## Blast radius` is a checklist: each edge names the assumption the two territories share, and an
  edge with nothing to do for a given change is a correct outcome.
- `## Governing decisions` answers why. `**None.**` there is itself the answer: only
  [ADR-0001](../adr/0001-commands-talk-to-a-session.md) and
  [ADR-0002](../adr/0002-hand-written-cdp-client.md) exist; most choices are maintainer's calls
  recorded on issues, and the notes carry them with their date and issue.
- Every claim carries its evidence (a symbol, a test, a constant) or its source and date
  (`Maintainer's call (2026-10-06, #4)`, `measured 2026-10-07`, a pinned upstream file).
- Sections are the same strings in every note; an empty one says `**None.**`. Code is named by
  symbol, never by line. Links are plain relative markdown. A fact lives in one note and the others
  link it.
- The map cannot answer what is not a note: source files and issues are text inside notes, not
  nodes.

## Scope

The map covers the three workspace members — the CLI and its session (`crates/pokit`), the macOS
test-build plugin (`crates/tauri-plugin-pokit`) and the fixture app (`fixtures/app`) — with their
tests and CI. Out of scope: `logo/` (brand assets, with their own README), `docs/agents/` (how agent
skills read this repo), and the v2 spec (#2: file drop, screenshot diff, parallel sessions, IPC
observation), which nothing in the repo builds or designs beyond that issue yet. A territory appears
when something in the repo commits to it — code, a decision record, or an open issue with a design;
an absent note means nothing has.

## The shape

pokit is a CLI an agent drives to test a running webview desktop app. Every command but `launch`,
`attach`, `capabilities` and `doctor` is a request to one background session that holds the app
instance (ADR-0001). On Windows the session reaches the page over its own CDP client (ADR-0002) and
reaches the OS through Win32; on macOS it reaches the page through a plugin compiled into the app's
test build, and most commands are not built there yet (the open `macos` issues). The largest gap is
that macOS half: snapshot, capture, logs, measure, clipboard and native UI exist only on Windows.

## Territories

The session and its contract
- [Starting the session process](starting-the-session-process.md)
- [Launching a test instance](launching-a-test-instance.md)
- [The command-line contract](the-command-line-contract.md)
- [Platform support](platform-support.md)
- [Checking preconditions](checking-preconditions.md)
- [The run record](the-run-record.md)
- [Collecting logs](collecting-logs.md)
- [Masking secrets](masking-secrets.md)

The page over CDP
- [Speaking CDP](speaking-cdp.md)
- [The app's pages](the-apps-pages.md)
- [Finding elements on the page](finding-elements-on-the-page.md)
- [Sending input over CDP](sending-input-over-cdp.md)
- [Typing Korean through IME](typing-korean-through-ime.md)
- [Capturing the page](capturing-the-page.md)

Measuring
- [Measuring the page](measuring-the-page.md)
- [Tracing the page](tracing-and-profiling.md)
- [Profiling the page](profiling-the-page.md)

The OS on Windows
- [Sending OS input](sending-os-input.md)
- [Placing the app's windows](placing-windows.md)
- [Capturing a window with its native UI](capturing-a-window.md)
- [Reaching an app's native UI](reaching-native-ui.md)
- [Using the user's clipboard](using-the-clipboard.md)

macOS
- [Reaching the page through the plugin](reaching-the-page-through-the-plugin.md)
- [Sending input on macOS](sending-input-on-macos.md)
- [Seeing the page on macOS](seeing-the-page-on-macos.md)

Testing
- [Testing against the fixture app](testing-against-the-fixture.md)

## Cross-cutting invariants

- [Page pixels on the screen](page-pixels-on-the-screen.md)
- [Capabilities match behaviour](capabilities-match-behaviour.md)
- [A pid may be reused](a-pid-may-be-reused.md)

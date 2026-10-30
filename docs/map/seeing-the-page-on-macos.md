# Seeing the page on macOS

## What it is

How pokit is to read a WKWebView page's structure (`snapshot` and refs) and capture its window (`capture`) on macOS without bringing the app forward (#6, #14, #35). Designed and measured, not built: the routes are the Accessibility API for structure and ScreenCaptureKit for pixels, as measured in #10 with a throwaway probe against the fixture app: macOS 26.6.2 (25G83), SDK 26.1, WebKit 21624.5.1.11.3, Tauri 2.12.1. Runs on 2026-10-02 are marked; the rest are 2026-10-06.

## Governing decisions

- [ADR-0001](../adr/0001-commands-talk-to-a-session.md): a snapshot's refs live in the session into the next command; a macOS snapshot keeps that, whatever route builds it.

[ADR-0002](../adr/0002-hand-written-cdp-client.md) sits adjacent: it decides the CDP client the Windows snapshot and capture use; neither macOS route goes through CDP.

## Design model

Nothing here is built; the design comes from the measurements of #10 and the issues that hold the work (#14, #35, #119, #32). #14 leaves the snapshot's route open between the Accessibility API and structure computed in the page; #10 measured the Accessibility API.

- **Structure through the Accessibility API.** The webview's accessibility tree carries what a snapshot needs (see Reference behaviour): role, accessible name, value, focus, the DOM `id`, and a subrole for a password field. The roles are AppKit's, not ARIA's, so they are to be mapped to the names the Windows snapshot prints ([Finding elements on the page](finding-elements-on-the-page.md)); #14 asks for each remaining difference to be listed in the map with its cause.
- **Ask twice, and look windows up from `AXWindows`.** The tree is built on first request, and an inactive app may have no focused window (see Reference behaviour), so a first walk is not to be trusted empty and `AXFocusedWindow` is not to be relied on.
- **Capture through ScreenCaptureKit, the window alone.** `SCContentFilter(desktopIndependentWindow:)` with `SCScreenshotManager.captureImage` gives the window's current pixels covered, minimized or on another Space; an element is to be the window capture cropped to its rectangle (#35). The window-list capture is not an option: it does not compile against SDK 26.1.
- **The window with its native UI waits for `native`.** A display filter limited to the app's windows includes an open app menu but fails while the window is minimized or on another Space (see Reference behaviour); #35 defers capture with native UI to the `native` contract (#5, #119).
- **A command-line process must touch `NSApplication.shared` first**, or the window capture aborts (see Reference behaviour).
- **Both need a permission granted to the app that started pokit**: Accessibility for the tree, Screen Recording for capture; `doctor` is to check them (#32, [Checking preconditions](checking-preconditions.md)).

**Input bounds.** Nothing is to bound the tree's size: a snapshot walks whatever the page exposes (see Known holes).

## Code

**None.** What does not exist: no Accessibility API or ScreenCaptureKit code in either crate; `support::ON_MACOS` (`crates/pokit/src/support.rs`) does not name `snapshot` or `capture`, so `State::handle` refuses them on a plugin session; a ref given to `read`, `click`, `type` or `key` is refused by `refs_unsupported` (`crates/pokit/src/session/commands.rs`), naming #14; and `failure_screenshot` (`crates/pokit/src/session/record.rs`) reads the current CDP target, so a failed macOS command leaves no screenshot.

Where a first slice would start: `State::snapshot_cmd` and `State::capture_cmd` (`crates/pokit/src/session/commands.rs`) are CDP-only today and would take a plugin arm, as `State::read_cmd` did for `read` ([Reaching the page through the plugin](reaching-the-page-through-the-plugin.md), #16's call that each macOS ticket widens the seam with its real implementation); with them `snapshot` and `capture` join `support::ON_MACOS`. Which process walks the tree and takes the picture, pokit's or the plugin inside the app, is not decided.

## Reference behaviour

All measured in #10 against the fixture app, macOS 26.6.2 (25G83), SDK 26.1, WebKit 21624.5.1.11.3, Tauri 2.12.1.

**Accessibility API**

- Under `AXWindow` → `AXScrollArea` → `AXWebArea` each element has its role (`AXTextField`, `AXButton`, `AXHeading`), its accessible name in `AXTitle`, its value, `AXFocused`, and the DOM `id` in `AXDOMIdentifier`; a password field has the subrole `AXSecureTextField` (2026-10-02).
- The tree is built on first request: the first walk of a fresh window found the web area with no children; a second walk found all 43 nodes (2026-10-02). Polled every 25 ms from the moment the app had a focused window, the tree was there after 234 ms (one launch). Setting `AXEnhancedUserInterface` or `AXManualAccessibility` on the app is refused (-25208, -25205) and is not needed.
- After the window is restored while the app is inactive, `AXFocusedWindow` is empty, though `AXWindows` still lists the window.
- Elements report their frame (`AXPosition`, `AXSize`, in screen points) and their actions (`AXPress`, `AXShowMenu`, `AXScrollToVisible` on a button). On a text field, `AXShowMenu` opened no menu with the app frontmost. `AXPress` as a click route is in [Sending input on macOS](sending-input-on-macos.md).

**Capture**

- `CGWindowListCreateImage` does not compile against SDK 26.1: "'CGWindowListCreateImage' is unavailable in macOS: Please use ScreenCaptureKit instead." The header marks it `SCREEN_CAPTURE_OBSOLETE(10.5,14.0,15.0)` (`CGWindow.h` 223–224, 271–274). xcap 0.9.8 still calls it (`src/macos/capture.rs` 15), so it is not used.
- `SCContentFilter(desktopIndependentWindow:)` with `SCScreenshotManager.captureImage` returned the fixture's own pixels, in about 150–240 ms, with the window partly covered, fully covered by two other apps, minimized, and on another Space. The minimized and other-Space captures were current, not a last frame: a key handled while minimized (`KeyA` in the page) and text inserted on another Space both showed. It leaves out native UI over the window: with the app menu open over it, the capture matched one without.
- A display filter limited to the app's windows, cropped to the window (`SCContentFilter(display:including:)`, `sourceRect`), included the open app menu, which is a separate layer-101 window of the app. It fails while the window is minimized or on another Space (`SCStreamErrorDomain` -3811).
- Without touching `NSApplication.shared` first, a command-line process's `desktopIndependentWindow` capture aborts with `Assertion failed: (did_initialize), function CGS_REQUIRE_INIT`.

## Cross-cutting invariants

**None.**

## Blast radius

- [Finding elements on the page](finding-elements-on-the-page.md) — the macOS snapshot is to print the same format and refs as the Windows one; a change to the Windows snapshot's role names or ref rules changes what the AppKit-role mapping must produce.
- [Capturing the page](capturing-the-page.md) — macOS `capture` is to take the same options and give the same output (#35); a change to the Windows command's contract is a change to this design.
- [The run record](the-run-record.md) — the failure screenshot slot assumes a capture path; on macOS there is none until #35.
- [Reaching native UI](reaching-native-ui.md) — `native` on macOS is to read the same Accessibility tree (#119), and the window-with-native-UI capture waits on its contract.
- [Checking preconditions](checking-preconditions.md) — both routes need a permission `doctor` is to check (#32); the permissions this design needs are the ones `doctor` must name.
- [Sending input on macOS](sending-input-on-macos.md) — `click`, `type` and `key` refuse refs until a snapshot exists; once refs come, they must resolve to a page point the plugin route can click.
- [Reaching the page through the plugin](reaching-the-page-through-the-plugin.md) — the plugin's channel answers requests only and holds no handle on an element, which decides whether a macOS ref can be anything but an Accessibility element or a page-side label.

## Known holes / open

- `snapshot` and refs on macOS are not built. Tracked: #14.
- `capture` on macOS is not built. Tracked: #35.
- `native list`, `choose` and `dismiss` through the Accessibility API are not built. Tracked: #119.
- `doctor` on macOS does not check Accessibility or Screen Recording. Tracked: #32.
- The snapshot's route is open between the Accessibility API and structure computed in the page (#14), and so is which process does the walking and the capture.
- Nothing bounds the size of the tree a snapshot would walk.
- Not measured: the tree on a page larger than the fixture's 43 nodes, and capture of an element alone.

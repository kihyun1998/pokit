# Capturing a window with its native UI

## What it is

How `capture --window` gives the window holding the current page as the user would see it, with the app's own menus and dialogs over it, as a PNG, on Windows (#5, #1 story 7). `capture` without `--window` is the page or an element alone, over CDP: [Capturing the page](capturing-the-page.md).

## Governing decisions

**None.** [ADR-0001](../adr/0001-commands-talk-to-a-session.md) is adjacent: an attached session, which it allows, does not know the app's process, so `capture --window` refuses there (exit 7); the ADR decides where commands run, not how a window is drawn.

## Design model

The rules come from a maintainer's call on #53 and otherwise from the source. The input is the app's own windows, at most as many as it has visible; their size is whatever the window's frame is, and the canvas allocates width × height × 4 bytes for it (`window::capture`). An element and `--window` together are a usage error (clap `conflicts_with = "target"`, `crates/pokit/src/main.rs — Command::Capture`).

- **Each of the app's windows is drawn as it draws itself, and the drawings are stacked; the screen is never read.** The window holding the page and every visible window of the app above it in z-order (`os_input::top_level_windows`, which lists in z-order) are drawn with `PrintWindow(PW_RENDERFULLCONTENT)` (`RENDER_FULL_CONTENT`, `print`), bottom first, onto a canvas the size of that window. So a dialog, an open menu or the menu bar is in the capture, and nothing of another app is, whether it covers the window or not.
  - The alternative, copying the screen over the window's rectangle (`BitBlt` from the screen DC), was measured and refused: with the fixture behind the terminal, it captured the terminal. It would hand the user's other windows to whoever reads the capture, and it needs the app uncovered.
  - Neither way takes the user's focus; `capabilities` declares `capture.window.takes_focus: false`.
- **The canvas is what the user sees of the window,** its frame without Windows' invisible resize borders (`DWMWA_EXTENDED_FRAME_BOUNDS`, `frame`, falling back to `GetWindowRect`); each window drawn on it is cut to its own visible frame the same way, so no black border shows. A dialog that reaches past the window's frame is cut at that frame. All of it is in physical pixels (`physical`); the page's CSS pixels are not involved, and the only page-to-screen step is finding the window, which is placing-windows' `window::matching`.
- **The result names the windows drawn**, bottom first, by title and class (`#32770` a dialog, `#32768` a menu), with the part of the capture each covers (`window::Drawn`).
- **A window DWM cloaks (on another virtual desktop, for one) is left out** (`cloaked`, `DWMWA_CLOAKED`), though Windows reports it visible. Not tested.
- **Each window's drawing is used only inside the rectangle it was drawn at,** so a window that moves between being located and being drawn is cut, not read past.
- **Layered windows are drawn opaque** (alpha 255). With a menu open, the app had no shadow or other layered window among its top-level windows, only the `#32768` menu (2026-10-07).
- **A minimized window is refused (exit 1);** `PrintWindow` has nothing to draw.
- **`PrintWindow` waits for the app's UI thread to draw,** so a hung app hangs the capture; there is no timeout of its own.
- **The window is found through the page** (`State::current_page_window`, as in [Placing the app's windows](placing-windows.md)), with the page given 1 s to say where it is (`PAGE_ANSWER`, as `window activate` does, [Sending OS input](sending-os-input.md)). **When it cannot be found, the app's main window is captured instead, and the result says so** (`window: "main"`, `page_not_placed` with the reason; otherwise `window: "page"`; the main window is `os_input::main_window`): a page stuck in a script, two windows of one size on top of each other, a minimized window. **Maintainer's call (2026-10-22, #53):** this over refusing, the same call as for `window activate` in #93, because a stuck page is when the user's view matters most. `ensure_ready` is not asked first: the capture draws the window whatever the page's state. Until #53 a page stuck for longer than the evaluate time limit (15 s) failed the capture after waiting it out.
  - `capture --window` resolves no element, so it releases no remote objects and waits on no other page afterwards ([Finding elements on the page](finding-elements-on-the-page.md)). Before #103 the release went to every page and waited out 15 s on a stuck one: the fallback capture took 16.3 s with it, 1.15 s without.
  - Measured (2026-10-22, #53): with the page stuck 12 s, the capture before #53 waited 12.1 s and succeeded; stuck 20 s, it waited 20 s and failed ("Runtime.evaluate: no answer within 15s"). After #53 the 20 s case returns the main window in about 1.2 s (finding the page's window 1.0 s, drawing 0.15 s).
- **Popups the page opens are not the app's windows.** WebView2 draws a `<select>` list or a tooltip in its browser process, which is a different process, so they are left out, as they are from `capture`.
- **The PNG is encoded by the `png` crate,** RGBA at 8 bits (`window::png`), and written to `--out` or the next numbered file in the run record (`State::capture_path`, [The run record](the-run-record.md)).
- Pinned by `a_window_capture_holds_the_apps_native_ui_and_nothing_of_other_apps` (tests/window.rs): the page is drawn (a third of the pixels are white); a topmost window of another process over the fixture changes under 1% of the capture; with the dialog open, the dialog is named and over a fifth of the pixels in the rectangle the result gives for it change; an element and `--window` together are a usage error. Reddened 2026-10-07: drawing only the main window, and listing the dialog without drawing it, turned the dialog checks red; copying the screen instead of `PrintWindow` turned the covered check red. Not reddened: dropping `PW_RENDERFULLCONTENT` (see Reference behaviour).
- Pinned by `a_window_capture_of_a_page_that_does_not_answer_is_of_the_main_window` (tests/window.rs): it stalls the page for 20 s and expects the main window, drawn, within 3 s, with `window: "main"` and a reason; the first capture test expects `window: "page"`. Reddened 2026-10-22: refusing instead of falling back turned it red on the exit code, and releasing remote objects on every page after it turned it red on the time (16.3 s).

## Code

- `crates/pokit/src/window.rs — capture, png, Shot, Drawn, frame, print, cloaked, text, RENDER_FULL_CONTENT`
- `crates/pokit/src/session/native.rs — State::capture_window_cmd, State::current_page_window, PAGE_ANSWER`

## Reference behaviour

- **Both ways, 2026-10-07, 150%, fixture with its dialog open and behind the terminal:** the stacked `PrintWindow` drawings showed the window, its menu bar, the page and the dialog in about 55 ms; the screen copy showed the terminal, in about 30 ms.
- **An open menu is drawn too (2026-10-07):** with Help opened by a click (the app in front), the `#32768` menu window was one of the app's windows above the main one and appeared in the capture.
- **`PW_RENDERFULLCONTENT` was not needed here (2026-10-07, WebView2 154):** without it the page was still drawn. The flag is kept.
- **The fixture's `alert()` does not block its page:** `tauri-plugin-dialog` (2.8.1, `init-iife.js`) replaces `window.alert` with an async `plugin:dialog|message` invoke, so `eval` answered and `capture --window` returned in 238 ms with an "alert" pending; no dialog window appeared, probably for lack of the plugin's permission (not confirmed).

## Cross-cutting invariants

- [Capabilities match behaviour](capabilities-match-behaviour.md) — `capture` declares `window: { native_ui: true, takes_focus: false, needs: "a launched session" }`.
## Blast radius

- [Placing the app's windows](placing-windows.md) — which window is drawn is the one `page_window` matches to the page; a change there changes what is captured and how often the main-window fallback is reported.
- [Sending OS input](sending-os-input.md) — `window activate` shares `State::current_page_window`, `PAGE_ANSWER` and the main window (`os_input::main_window`); changing the timeout or the fallback for one changes the other's `window`/`page_not_placed`. Both also rely on `os_input::top_level_windows` listing in z-order.
- [Reaching an app's native UI](reaching-native-ui.md) — the capture names dialogs (`#32770`) and menus (`#32768`) by the same window classes `native` reads; a dialog or menu `native` can see is expected to be in the capture.
- [Capturing the page](capturing-the-page.md) — `capture` and `capture --window` share the command, `--out` and the run record's numbering (`State::capture_path`); a change to the command's options or output path changes both.

## Known holes / open

- No test: an open menu (it needs the app in front and a click), a minimized window, a cloaked window.
- Whether another WebView2 or Windows version needs `PW_RENDERFULLCONTENT` was not checked.
- WebView2's own blocking `alert` was not produced with the fixture, so how it draws and whether the capture reaches it is not measured.
- A hung app hangs the capture: `PrintWindow` has no timeout, and pokit sets none around it beyond the command's own time limit.
- `capture --window` exists only on Windows; macOS refuses it as not built. The macOS capture design (ScreenCaptureKit) is [Seeing the page on macOS](seeing-the-page-on-macos.md)'s; it notes that native UI drawn over the window waits for `native` on macOS. Tracked: #35, #119.

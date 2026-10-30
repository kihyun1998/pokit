# Placing the app's windows

## What it is

How pokit finds the app's top-level window that holds the current page, and how `window move` and `window resize` place that window without bringing it to the front, on Windows (#5, #1 story 18). The same finding is what `window activate` and the OS input guard ([Sending OS input](sending-os-input.md)) and `capture --window` ([Capturing a window with its native UI](capturing-a-window.md)) use to know which window is the page's.

## Governing decisions

**None.** [ADR-0001](../adr/0001-commands-talk-to-a-session.md) is adjacent: an attached session, which it allows, does not know the app's process, so these commands refuse there; the ADR decides that commands go to a session, not how a window is found or placed.

## Design model

The rules come from #5 and #1 story 18, and otherwise only from the source; no maintainer's call is recorded for this area beyond the commands existing. Outside input: `--x`, `--y`, `--width`, `--height` from the command line, checked finite (and the size above 0) by clap before they reach the session (`crates/pokit/src/main.rs — finite, positive`), and the page's own report of where it is.

- **The window is found from the page, not from a title or a label.** The page reports where its area is on the screen (`screenX`, `screenY`) and how big it is (`innerWidth`, `innerHeight`), in CSS pixels, with its `devicePixelRatio` (`State::page_box_within`, `window::PageBox`). Of the app's visible top-level windows, owned ones included (`os_input::top_level_windows`), the one whose WebView2 render widget (`os_input::render_widget`) has its client area there, within a CSS pixel on every side, is the page's window (`window::matching`, over the areas `page_area` reads with `ClientToScreen` and `GetClientRect`). Two windows exactly on top of each other with the same size cannot be told apart, and pokit says so rather than guess (`"N of the app's windows sit exactly where this page is"`). Nothing app-specific is needed: a Tauri window label would be. Pinned by `the_page_is_in_the_window_whose_page_area_matches_it_within_a_css_pixel` (window.rs unit test).
  - The size is compared at the page's `devicePixelRatio`. The origin is compared at that ratio or at the window's own scale, because the two differ when the page is zoomed, and it is not measured which one `screenX` follows then. Pinned by `a_zoomed_page_is_matched_by_its_origin_at_the_window_scale`.
  - The window areas are read with the thread switched to per-monitor DPI awareness (`physical`), so they are physical pixels.
  - None found: exit 5 (`not_found`); none found while one of the app's windows is minimized: exit 1, naming the minimized window (`window::Unplaced::Minimized`, mapped in `State::page_window`).
  - `window move` and `window resize` ask the page with the evaluate time limit (`State::page_box`, 15 s, `EVALUATE_TIMEOUT`) after `ensure_ready` ([The app's pages](the-apps-pages.md)); `window activate`, the guard and `capture --window` ask with 1 s (`PAGE_ANSWER`, through `State::current_page_window`) and fall back or refuse instead.
- **Coordinates are the window's logical pixels:** physical pixels over the window's own scale (`GetDpiForWindow` / 96, `window::scale`), as Tauri's `LogicalPosition`/`LogicalSize` and WebDriver's Set Window Rect take them. At 150% `--width 600` makes the window 900 physical pixels wide. Across monitors with different scales, the scale is the one the window has before the move (`window::set`); the reported `rect` and `scale` are read after the page has followed (`window::placed`), so they are the window's after any rescale.
- **`resize` sets the outer window by default, `--viewport` sets the page.** The outer size is what `GetWindowRect` gives, including Windows' invisible resize borders, so the page is smaller than the window by the borders, the title bar and the menu bar. With `--viewport`, the window grows or shrinks by what the page lacks or has over (`outer_for_viewport`: the wanted CSS size times the page's ratio, less the page area now, over the window's scale), as Tauri's `set_size` sets the inner size; layout bugs usually follow the viewport's width. The result always reports the page's new viewport and `devicePixelRatio`. Pinned by `the_outer_size_for_a_viewport_grows_the_window_by_what_the_page_lacks`.
- **Neither command takes the user's focus or changes the stacking order.** `SetWindowPos` is called with `SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER` (`window::set`). `capabilities` reports `takes_focus` per `window` action: `activate` takes it, `move` and `resize` do not (#1 story 42).
  - `SetWindowPos` without `SWP_NOACTIVATE` did not take the foreground either, with the test run from the terminal in front: the foreground lock kept it back. The flag is kept so that the call never asks for it. Without `SWP_NOZORDER` the resized window did come above the app's other window (2026-10-07).
- **A minimized or maximized window is refused (exit 1), not restored** (`window::set`). Restoring would change what the caller sees in a way they did not ask for, and `SW_RESTORE` activates. Unlike WebDriver's Set Window Rect, which restores.
- **A place or size that is not a finite number, or a size not above 0, is a usage error (exit 2)**, caught by the command line before it reaches the session: JSON has no NaN or infinity.
- **The command waits for the page to follow** (up to 3 s, `PLACE_SETTLE`, polled every 50 ms in `State::window_place_cmd`): the page must be where the window's page area now is, by `matching` against that one area. Otherwise exit 4 (`timeout`). A change under one CSS pixel cannot be told from no change, so the viewport reported for it may be the old one.
- **An attached session refuses** (exit 7): it does not know the app's process, so it cannot tell its windows from other apps'.
- Pinned by `window_move_and_resize_reshape_the_page_without_taking_the_foreground` (tests/window.rs). It compares sizes and places with each other, not with pixels, so it holds at any scaling: two resizes 100×50 apart shrink the page by 100×50, two moves 60×40 apart move it by 60×40, and `--viewport 640x400` gives a 640×400 page. It resizes the second window from its page, then the main window while the second is on top, which is what fails if the wrong window is picked, and checks the second stays on top. Reddened 2026-10-07: dropping the scale, always taking the topmost window, not noticing a minimized window, dropping `SWP_NOZORDER`, ignoring `--viewport`, and letting infinite or non-positive numbers through each turned it red. Not reddened: removing the wait for the page to follow, and removing `SWP_NOACTIVATE`, for the reasons measured (Reference behaviour, and above). The foreground check runs only when the fixture is not already in front.

## Code

- `crates/pokit/src/window.rs — PageBox, Placed, Area, Unplaced, matching, outer_for_viewport, page_window, page_area, area, placed, set, scale, physical`
- `crates/pokit/src/session/native.rs — State::window_place_cmd, State::page_box, State::page_box_within, State::page_window, PLACE_SETTLE`

## Reference behaviour

- **`screenX`/`screenY` are the page area's screen position in CSS pixels, and follow a move at once (2026-10-07, WebView2 154, Tauri 2.12.1, 150%).** The fixture's main window at physical (342, 342) reported `screenX 235, screenY 278`: its render widget was at (352.5, 417) physical, past an 11 px border and a 75 px title and menu bar. After `SetWindowPos` to (500, 400) the page reported (341, 317) = (511, 475) / 1.5 by the next `eval`. `outerWidth` was `innerWidth + 1`, not the window's width, so it is not used.
- **A resize reaches the page by the next `eval` too (2026-10-07):** the window resized to 900×700 physical had a client area of 878×614, and the page reported 586×410.
- WebDriver's Set Window Rect restores a minimized or maximized window; pokit refuses instead (above).

## Cross-cutting invariants

- [Page pixels on the screen](page-pixels-on-the-screen.md) — `window::matching` divides a window's physical page area by the ratio to compare it with the page's CSS report; `outer_for_viewport` multiplies a CSS size by the same ratio.
- [Capabilities match behaviour](capabilities-match-behaviour.md) — `window move` and `window resize` declare `takes_focus: false`.

## Blast radius

- [Sending OS input](sending-os-input.md) — the guard demands in front, and `window activate` raises, the window `page_window` returns; a looser match lets OS input go to a window that is not the page's, a stricter one refuses input that would have reached it.
- [Capturing a window with its native UI](capturing-a-window.md) — it draws the window `page_window` returns and falls back to the main window when it fails; a change to matching changes which window is captured and how often `page_not_placed` appears.
- [The app's pages](the-apps-pages.md) — `window move` and `resize` rely on `ensure_ready` and on the page answering `screenX`/`innerWidth` within the evaluate time limit; a page that is not ready or stuck makes them wait or fail.
- [Platform support](platform-support.md) — `capabilities` declares `move` and `resize` as not taking focus; adding activation or a restore here makes that false.

## Known holes / open

- Not measured: a zoomed page. Tauri turns zoom hotkeys off by default and the fixture has no way to zoom, so whether `screenX` follows the zoom is open; `matching` accepts either.
- No test: the maximized refusal, an owned window, a zoomed page.
- `window move` and `window resize` exist only on Windows; macOS refuses them as "not built on macOS yet (#6)" (`support::reason`). No issue under #6 names them.

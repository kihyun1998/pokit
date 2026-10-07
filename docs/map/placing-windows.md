# Placing the app's windows

How `window move` and `window resize` find the window that holds the current page and place it without bringing it to the front, on Windows (#5, #1 story 18). Code: `window::page_window`, `window::matching`, `window::outer_for_viewport`, `window::set`, `window::placed`, `window::area` (window.rs), `State::window_place_cmd` (session/native.rs), `os_input::top_level_windows`, `os_input::render_widget`.

## Design model

- **The window is found from the page, not from a title or a label.** The page reports where its area is on the screen (`screenX`, `screenY`) and how big it is (`innerWidth`, `innerHeight`), in CSS pixels. Of the app's visible top-level windows, owned ones included, the one whose WebView2 render widget sits there, within a CSS pixel on every side, is the page's window (`matching`). Two windows exactly on top of each other with the same size cannot be told apart, and pokit says so rather than guess. Nothing app-specific is needed: a Tauri window label would be.
  - The size is compared at the page's `devicePixelRatio`. The origin is compared at that ratio or at the window's own scale, because the two differ when the page is zoomed, and it is not measured which one `screenX` follows then (see Measured).
- **Coordinates are the window's logical pixels:** physical pixels over the window's own scale (`GetDpiForWindow` / 96), as Tauri's `LogicalPosition`/`LogicalSize` and WebDriver's Set Window Rect take them. At 150% `--width 600` makes the window 900 physical pixels wide. Across monitors with different scales, the scale is the one the window has before the move; the reported `rect` and `scale` are read after the page has followed, so they are the window's after any rescale.
- **`resize` sets the outer window by default, `--viewport` sets the page.** The outer size is what `GetWindowRect` gives, including Windows' invisible resize borders, so the page is smaller than the window by the borders, the title bar and the menu bar. With `--viewport`, the window grows or shrinks by what the page lacks or has over (`outer_for_viewport`), as Tauri's `set_size` sets the inner size; layout bugs usually follow the viewport's width. The result always reports the page's new viewport and `devicePixelRatio`.
- **Neither command takes the user's focus or changes the stacking order.** `SetWindowPos` is called with `SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER`. `capabilities` reports `takes_focus` per `window` action: `activate` takes it, `move` and `resize` do not (#1 story 42).
- **A minimized or maximized window is refused (exit 1), not restored.** Restoring would change what the caller sees in a way they did not ask for, and `SW_RESTORE` activates. Unlike WebDriver's Set Window Rect, which restores.
- **A place or size that is not a finite number, or a size not above 0, is a usage error (exit 2)**, caught by the command line before it reaches the session: JSON has no NaN or infinity.
- **The command waits for the page to follow** (up to 3 s, `PLACE_SETTLE`): the page must be where the window's page area now is. A change under one CSS pixel cannot be told from no change, so the viewport reported for it may be the old one.
- **An attached session refuses** (exit 7): it does not know the app's process, so it cannot tell its windows from other apps'.

## Measured

- **`screenX`/`screenY` are the page area's screen position in CSS pixels, and follow a move at once (2026-10-07, WebView2 154, Tauri 2.12.1, 150%).** The fixture's main window at physical (342, 342) reported `screenX 235, screenY 278`: its render widget was at (352.5, 417) physical, past an 11 px border and a 75 px title and menu bar. After `SetWindowPos` to (500, 400) the page reported (341, 317) = (511, 475) / 1.5 by the next `eval`. `outerWidth` was `innerWidth + 1`, not the window's width, so it is not used.
- **A resize reaches the page by the next `eval` too:** the window resized to 900×700 physical had a client area of 878×614, and the page reported 586×410.
- **`SetWindowPos` without `SWP_NOACTIVATE` did not take the foreground either,** with the test run from the terminal in front: the foreground lock kept it back. The flag is kept so that the call never asks for it. Without `SWP_NOZORDER` the resized window did come above the app's other window.
- **Not measured: a zoomed page.** Tauri turns zoom hotkeys off by default and the fixture has no way to zoom, so whether `screenX` follows the zoom is open; `matching` accepts either.

## Testing

`window_move_and_resize_reshape_the_page_without_taking_the_foreground` (tests/window.rs) compares sizes and places with each other, not with pixels, so it holds at any scaling: two resizes 100×50 apart shrink the page by 100×50, two moves 60×40 apart move it by 60×40, and `--viewport 640x400` gives a 640×400 page. It resizes the second window from its page, then the main window while the second is on top, which is what fails if the wrong window is picked, and checks the second stays on top.

- Reddened 2026-10-07: dropping the scale, always taking the topmost window, not noticing a minimized window, dropping `SWP_NOZORDER`, ignoring `--viewport`, and letting infinite or non-positive numbers through each turned it red.
- Not reddened: removing the wait for the page to follow, and removing `SWP_NOACTIVATE`, for the reasons measured above. The foreground check runs only when the fixture is not already in front.
- No test: the maximized refusal, an owned window, a zoomed page.

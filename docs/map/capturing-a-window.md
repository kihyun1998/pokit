# Capturing a window with its native UI

How `capture --window` gives the window holding the current page as the user would see it, with the app's own menus and dialogs over it, on Windows (#5, #1 story 7). `capture` without `--window` is the page alone, over CDP. Code: `window::capture`, `window::png` (window.rs), `State::capture_window_cmd` (session/native.rs).

## Design model

- **Each of the app's windows is drawn as it draws itself, and the drawings are stacked; the screen is never read.** The window holding the page (found as in [[placing-windows]]) and every visible window of the app above it in z-order are drawn with `PrintWindow(PW_RENDERFULLCONTENT)`, bottom first, onto a canvas the size of that window. So a dialog, an open menu or the menu bar is in the capture, and nothing of another app is, whether it covers the window or not.
  - The alternative, copying the screen over the window's rectangle (`BitBlt` from the screen DC), was measured and refused: with the fixture behind the terminal, it captured the terminal. It would hand the user's other windows to whoever reads the capture, and it needs the app uncovered.
  - Neither way takes the user's focus.
- **The canvas is what the user sees of the window,** its frame without Windows' invisible resize borders (`DWMWA_EXTENDED_FRAME_BOUNDS`); each window drawn on it is cut to its own visible frame the same way, so no black border shows. A dialog that reaches past the window's frame is cut at that frame.
- **The result names the windows drawn**, bottom first, by title and class (`#32770` a dialog, `#32768` a menu), with the part of the capture each covers.
- **A window DWM cloaks (on another virtual desktop, for one) is left out**, though Windows reports it visible. Not tested.
- **Each window's drawing is used only inside the rectangle it was drawn at,** so a window that moves between being located and being drawn is cut, not read past.
- **Layered windows are drawn opaque** (alpha 255). With a menu open, the app had no shadow or other layered window among its top-level windows, only the `#32768` menu (2026-10-07).
- **A minimized window is refused (exit 1);** `PrintWindow` has nothing to draw.
- **`PrintWindow` waits for the app's UI thread to draw,** so a hung app hangs the capture; there is no timeout of its own.
- **The window is found through the page** (as in [[placing-windows]]), which takes a `Runtime.evaluate`. A page stuck in a script, or under its own `alert()`, cannot be captured this way.
- **Popups the page opens are not the app's windows.** WebView2 draws a `<select>` list or a tooltip in its browser process, which is a different process, so they are left out, as they are from `capture`.
- **The PNG is encoded by the `png` crate,** RGBA at 8 bits.

## Measured

- **Both ways, 2026-10-07, 150%, fixture with its dialog open and behind the terminal:** the stacked `PrintWindow` drawings showed the window, its menu bar, the page and the dialog in about 55 ms; the screen copy showed the terminal, in about 30 ms.
- **An open menu is drawn too:** with Help opened by a click (the app in front), the `#32768` menu window was one of the app's windows above the main one and appeared in the capture.
- **`PW_RENDERFULLCONTENT` was not needed here:** without it the page was still drawn (WebView2 154). The flag is kept; whether another WebView2 or Windows version needs it was not checked.

## Testing

`a_window_capture_holds_the_apps_native_ui_and_nothing_of_other_apps` (tests/window.rs): the page is drawn (a third of the pixels are white); a topmost window of another process over the fixture changes under 1% of the capture; with the dialog open, the dialog is named and over a fifth of the pixels in the rectangle the result gives for it change; an element and `--window` together are a usage error.

- Reddened 2026-10-07: drawing only the main window, and listing the dialog without drawing it, turned the dialog checks red; copying the screen instead of `PrintWindow` turned the covered check red.
- Not reddened: dropping `PW_RENDERFULLCONTENT`, for the reason measured above. No test: an open menu (it needs the app in front and a click), a minimized window.

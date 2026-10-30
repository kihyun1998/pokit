# Page pixels on the screen

## The fact

A point of the page, in CSS pixels from the page's top left, is at the page view's own origin plus
that point times the page's scale. On Windows the point is a screen point in physical pixels: the
origin is the client origin of the WebView2 render widget (`Chrome_RenderWidgetHostHWND`) and the
scale is the page's `devicePixelRatio`. On macOS it is a window point in AppKit points: the origin is
the `WKWebView`'s top left moved by its safe-area insets (left and top), the scale is
`pageZoom × magnification` only (no backing scale factor), and `convertPoint_toView(None)` turns the
view point into a window point. Every site that turns a page point into a screen or window point, or
a screen rectangle back into page pixels, uses this rule.

## Why it is cross-cutting

Four sites do the arithmetic, each on its own, and none calls another:

- `crates/pokit/src/os_input.rs — to_screen` multiplies a CSS point by the ratio and adds the widget's
  `ClientToScreen` corner, which `at_page_point` reads, to aim OS clicks, wheels and a drag's first
  point.
- `crates/pokit/src/os_input.rs — drag` places every later point of an OS drag by its own
  `start + (x − first.x) × ratio`, from the screen point the first one landed at, without
  `to_screen`.
- `crates/pokit/src/window.rs — matching` divides a window's physical page area by the ratio and
  compares it with what the page reports (`screenX`, `screenY`, `innerWidth`, `innerHeight`), to find
  the window that holds a page. `page_area` reads the same widget's corner by its own `ClientToScreen`.
- `crates/tauri-plugin-pokit/src/input.rs — window_point` adds the safe-area inset and multiplies by
  `pageZoom × magnification`, to aim the plugin's mouse events on macOS.

They share only `os_input::render_widget`, which finds the widget; the conversion itself is written
four times. A change to how one of them treats zoom, scaling or an inset, made without the others,
puts input at a point other than the one the page was found at.

## Territories it holds in

- [Sending OS input](sending-os-input.md) — `os_input::to_screen`, `at_page_point`, `os_input::drag`;
  the ratio is read from the page per command (`session/os_input.rs`).
- [Placing the app's windows](placing-windows.md) — `window::matching`, `page_area`,
  `outer_for_viewport`. `window activate`, the OS input guard and `capture --window` find the page's
  window through `window::page_window`, so they reach this site rather than being sites of their own.
- [Sending input on macOS](sending-input-on-macos.md) — `input::window_point` in the plugin.

## What a violation looks like

An OS click, wheel or drag start lands beside its element, or the guard refuses it because
`WindowFromPoint` finds another window there; `window move`/`resize` waits out `PLACE_SETTLE`
because the page never seems to follow; `window activate` or `capture --window` falls back to the
main window with `page_not_placed`. On macOS, a click lands off by the title bar's height (32 points
on the fixture) or by the zoom. Reproduces at a scaling other than 100%, on a zoomed page, or in a
window with a full-size content view.

## Discovery history

- At 150% (DPI 144, ratio 1.5) an OS click on `#target` produced `click`, a double click `dblclick`,
  a right click `contextmenu` (measured 2026-10-07; [Sending OS input](sending-os-input.md)).
- `screenX`/`screenY` were found to be the render widget's position over the ratio, past an 11 px
  border and a 75 px title and menu bar (measured 2026-10-07, 150%;
  [Placing the app's windows](placing-windows.md)).
- On macOS a point taken without the safe-area inset landed 32 pixels too high, `clientY` 252 for
  284 (measured 2026-10-25, #34; [Sending input on macOS](sending-input-on-macos.md)).
- Unit tests pin each site's arithmetic: `os_input::tests` (`to_screen((100, 200), (10.0, 20.0), 1.5)`
  is `(115, 230)`) and `window::tests::a_zoomed_page_is_matched_by_its_origin_at_the_window_scale`.
  Named as an invariant from reading the three sites (map rewrite, 2026-10-30).

## Where it will recur

A function that takes a page point and produces a screen, window or view point — or reads a screen
rectangle and compares it with page pixels — is subject to this. Open questions it carries: which
scale `screenX` follows on a zoomed page (`matching` accepts either), and a drag path that crosses
onto a monitor with another scale, which uses one ratio for the whole path (both unmeasured; see the
territories' Known holes).

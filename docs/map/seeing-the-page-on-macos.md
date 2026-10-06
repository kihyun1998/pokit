# Seeing the page on macOS

How pokit can read a WKWebView page's structure and capture its window on macOS without bringing the app forward (#6, #14). Code: none yet. Measured in #10 with a throwaway probe against the fixture app: macOS 26.6.2 (25G83), SDK 26.1, WebKit 21624.5.1.11.3, Tauri 2.12.1. Runs on 2026-10-02 are marked; the rest are 2026-10-06.

## Structure, through the Accessibility API

- **The webview's accessibility tree carries what a snapshot needs.** Under `AXWindow` → `AXScrollArea` → `AXWebArea` each element has its role (`AXTextField`, `AXButton`, `AXHeading`), its accessible name in `AXTitle`, its value, `AXFocused`, and the DOM `id` in `AXDOMIdentifier`; a password field has the subrole `AXSecureTextField` (2026-10-02). This is the route for `snapshot` (#14). The roles are AppKit's, not ARIA's, so they are mapped to the names the Windows snapshot prints.
- **The tree is built on first request.** The first walk of a fresh window found the web area with no children; a second walk found all 43 nodes (2026-10-02). Polled every 25 ms from the moment the app had a focused window, the tree was there after 234 ms (one launch). Setting `AXEnhancedUserInterface` or `AXManualAccessibility` on the app is refused (-25208, -25205) and is not needed.
- **An inactive app may have no focused window.** After the window is restored while the app is inactive, `AXFocusedWindow` is empty, though `AXWindows` still lists the window. Look elements up from `AXWindows`.
- **Elements report their frame** (`AXPosition`, `AXSize`, in screen points) and their actions (`AXPress`, `AXShowMenu`, `AXScrollToVisible` on a button). On a text field, `AXShowMenu` opened no menu with the app frontmost.

## Capture, through ScreenCaptureKit

- **The window-list capture is gone.** `CGWindowListCreateImage` does not compile against SDK 26.1: "'CGWindowListCreateImage' is unavailable in macOS: Please use ScreenCaptureKit instead." The header marks it `SCREEN_CAPTURE_OBSOLETE(10.5,14.0,15.0)` (`CGWindow.h` 223–224, 271–274). xcap 0.9.8 still calls it (`src/macos/capture.rs` 15), so it is not used.
- **A window alone: `SCContentFilter(desktopIndependentWindow:)` with `SCScreenshotManager.captureImage`.** It returned the fixture's own pixels, in about 150–240 ms, with the window partly covered, fully covered by two other apps, minimized, and on another Space. The minimized and other-Space captures were current, not a last frame: a key handled while minimized (`KeyA` in the page) and text inserted on another Space both showed. It leaves out native UI over the window: with the app menu open over it, the capture matched one without.
- **The window with its native UI: a display filter limited to the app's windows, cropped to the window** (`SCContentFilter(display:including:)`, `sourceRect`). It included the open app menu, which is a separate layer-101 window of the app. It fails while the window is minimized or on another Space (`SCStreamErrorDomain` -3811).
- **A command-line process must touch `NSApplication.shared` first.** Without it, `desktopIndependentWindow` capture aborts with `Assertion failed: (did_initialize), function CGS_REQUIRE_INIT`.

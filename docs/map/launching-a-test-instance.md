# Launching a test instance

How `pokit launch` gets a WebView2 app running with its own profile and a debugging port, and when it counts a page as ready. Code: `session::launch::spawn_app`, `session::launch::active_port`, `session::launch::wait_ready`, `State::ensure_ready` (session/pages.rs), `launch::READY_PROBE`, `devtools::get_json`.

## Isolation: what pokit can and cannot separate

- **The WebView2 profile and the debugging port, yes.** `WEBVIEW2_USER_DATA_FOLDER` *replaces* the folder the app passes to `CreateCoreWebView2EnvironmentWithOptions`. `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` is *appended* to the arguments the app passes. Tauri passes both explicitly (`app_local_data_dir`, and wry's `--disable-features=msWebOOUI,…`), and neither blocks the variables. Source: `WebView2.idl` lines 8873–8886 in SDK 1.0.4258.31, and a probe on runtime 154.0.4258.37 (2026-10-01).
- **A debugging port on a profile that is already in use, no.** Creating the second controller fails with `0x8007139F` (`ERROR_INVALID_STATE`), and the port never opens. A test instance therefore always gets a fresh folder.
- **The app's own data, no.** Tauri resolves `app_data_dir` and the related folders through `SHGetKnownFolderPath`, so `APPDATA` and `LOCALAPPDATA` have no effect. Redirecting `USERPROFILE` does move them on a default machine, but that behaviour is undocumented, breaks under folder-redirection policy, and moves `~/.ssh` too. **Maintainer's call (2026-10-01, #3):** pokit isolates only the WebView2 profile and the port; the caller isolates app data through `launch --env` or app arguments. The alternative shown was pokit redirecting `USERPROFILE`.
- **A profile pokit created is deleted when the session ends** (≈8 MB each). It takes retries: WebView2's processes hold its files for a moment after the app is killed. A `--data-dir` the caller supplied is left alone.

## The debugging port

- **By default the browser picks the port.** pokit passes `--remote-debugging-port=0`, and WebView2 writes the port it bound to the first line of `<profile>/EBWebView/DevToolsActivePort` (probe, 2026-10-01). Picking a free port in pokit and handing its number over leaves a gap in which another process can take it.
- `/json/version` on WebView2 returns a browser-level `webSocketDebuggerUrl`. `Target.getTargets`, `Browser.getVersion` and `Target.attachToTarget` with `flatten` all work there (probe, 2026-10-01). pokit still connects to each page's own WebSocket from `/json/list`, and follows only sockets on the instance's own port.
- The DevTools HTTP server can keep the connection open after answering, even when asked to close it. A reader that waits for end-of-stream hangs until its timeout. The body has to be cut at `Content-Length`.
- **The port has no authentication.** Any local process can drive the instance while it runs. This comes with WebView2 remote debugging; a pipe instead of a port is not available, because the WebView2 loader, not pokit, starts the browser process.

## Readiness

A page is ready when its real document has loaded, is visible, and has painted two frames. It is checked for the main page at launch, and again before the first input to any page whose document has changed since. A new default execution context for the page's main frame (`Runtime.executionContextCreated`) marks the page not ready. Two traps sit behind that definition:

- **The page target starts on `about:blank`.** About 100 ms later it navigates to the app's URL *under the same target id* (seen in 3 out of 3 launches). `about:blank` reports `readyState === "complete"`, so a check on load state alone declares the wrong document ready. URL and state are read in one evaluation so they describe the same document.
- **Input sent before the first frames is dropped, and CDP still reports success.** Under load (14 parallel launches), the first `type` after `launch` sometimes delivered no `keydown` at all. The fixture's event log showed `focusin` and then nothing. `performance.timeOrigin` was unchanged, so this was not a reload. Waiting for `visibilityState === "visible"` and two `requestAnimationFrame` callbacks before reporting ready took the suite from failing about one run in four to 15 clean runs in a row. Reverting the wait made it fail on the first run. The Chromium mechanism (input dropped while the first paint is held) is inferred, not read from source.

## Ruled out

Tested while finding the above, and they did not hold:

- Another window taking OS focus mid-typing. Windows' foreground lock keeps a background-launched window from taking focus at all; `document.hasFocus()` stayed true.
- A page reload after ready. `timeOrigin` was unchanged at the point of failure.
- Two instances sharing a debugging port. 14 parallel launches got 14 distinct ports and browsers.

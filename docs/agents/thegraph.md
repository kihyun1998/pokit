# thegraph

## What this project is

A Rust CLI an AI agent drives to test a running webview desktop app (Tauri and the like), functional and performance — launch, see, act, verify, measure and record — over CDP on Windows, through a plugin on macOS.

## References

| Source | Informs | Reached by | Binding |
|---|---|---|---|
| Chrome DevTools Protocol — Input, Runtime, Tracing, IO, Profiler, Page | how it works | chromedevtools.github.io/devtools-protocol — **summarized** | binding |
| WebView2 — remote debugging port via `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`, `AcceleratorKeyPressed` | how it works | Microsoft Learn — **summarized** | binding |
| Tauri v2 — plugin authoring, webview eval, `cfg(debug_assertions)`-only build | how it works | v2.tauri.app — **summarized** | binding |
| Win32 — `SendInput` (incl. UIPI), foreground-window check, per-window DPI | how it works | Microsoft Learn — **summarized** | binding |
| macOS — `CGEventPost`/`CGEventPostToPid`, `AXIsProcessTrusted`, ScreenCaptureKit | how it works | Apple developer docs — **summarized** | binding |
| Chromium Trace Event Format — summarising a trace into style/layout/paint | how it works | its published spec — **summarized** | binding |
| penterm — `docs/agents/dogfooding.md` § Driving the running app over CDP, `measure-held-size-keys.mjs` | how it works | its source tree (`D:\github\penterm`) — raw | example |
| Playwright — `connectOverCDP`, key/mouse events over CDP | how it works | its source tree — raw | example |
| chromiumoxide — write our own CDP client or use this crate | how it works | its source tree — raw | example |
| enigo, xcap, core-graphics, windows-rs — OS input, capture, platform API candidates | how it works | their source trees — raw | example |
| tauri-driver, CrabNebula macOS WebDriver — whether an existing tool already overlaps | how it works | tauri-driver: its source tree — raw; CrabNebula: its docs — **summarized** | example |

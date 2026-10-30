# Capturing the page

## What it is

How `capture` without `--window` takes a PNG of the current page, or of one element, over CDP, and how the same screenshot call writes the failure screenshot a failed command leaves in the run record. It is the page alone, as the renderer draws it: the window holding it, with the app's native UI over it, is [Capturing a window](capturing-a-window.md); where the failure screenshot goes and when one is taken is [The run record](the-run-record.md).

## Governing decisions

**None.** [ADR-0002](../adr/0002-hand-written-cdp-client.md) is adjacent: it decides the client the screenshot call goes through, not what is captured.

## Design model

The rules come from reading the source; no maintainer's call or record decides them.

- **A capture is one `Page.captureScreenshot` in PNG, decoded from base64 and written to a file** (`screenshot_to`, session/record.rs), which returns the size in bytes. `capture` reports `path` and `bytes` (`State::capture_cmd`).
- **Without a target, it is the current page's viewport** (no clip). **With a target** (a ref or a selector, resolved as in [Finding elements on the page](finding-elements-on-the-page.md)), the element is scrolled to the centre of the view and the capture is clipped to its rectangle in document coordinates (its viewport position plus the page's scroll offset) at scale 1 (`scroll_and_measure`). Pinned by `capture_writes_a_png_of_the_page_or_one_element` (tests/fixture.rs): both files start with the PNG signature, and the element's is smaller than the page's.
- **Where the file goes:** `--out`, made absolute against the directory the command ran in before it reaches the session (`absolute`, main.rs; pinned by `a_relative_capture_path_is_relative_to_where_the_command_ran`, tests/fixture.rs), or else the next numbered file in the run record, `capture-<n>.png` (`State::capture_path`).
- **The failure screenshot is the same call on the current page, unclipped,** written as `fail-<n>.png` in the run record (`State::failure_screenshot`); when it cannot be taken the failure is reported without it. Which commands leave one is [The run record](the-run-record.md)'s; a command carrying a secret leaves none ([Masking secrets](masking-secrets.md)).
- **No readiness check is made first** (`State::capture_cmd` does not call `ensure_ready`): a page in any state is captured as it is.
- **Popups the page opens are left out.** WebView2 draws a `<select>` list or a tooltip in its browser process, outside the page's rendering, so a capture of the page does not hold them (stated with #50, 2026-10-07, [Capturing a window](capturing-a-window.md)).
- **On macOS `capture` is refused**: it is not among the commands the plugin answers (`support::plugin_answers`); [Seeing the page on macOS](seeing-the-page-on-macos.md).
- **Size and count.** Nothing bounds a capture's size but the page's: a large element clip makes a large PNG, and its base64 comes back as one CDP message, limited as in [Speaking CDP](speaking-cdp.md). Every capture without `--out` adds a file to the run record.

## Code

- `crates/pokit/src/session/commands.rs` — `State::capture_cmd`, `State::capture_path`, `scroll_and_measure`
- `crates/pokit/src/session/record.rs` — `screenshot_to`, `State::failure_screenshot`
- `crates/pokit/src/main.rs` — `absolute`

## Reference behaviour

- WebView2 draws `<select>` lists and tooltips in its browser process, a different process from the app's (#50, 2026-10-07, recorded in [Capturing a window](capturing-a-window.md)).

## Cross-cutting invariants

**None.**

## Blast radius

- [The run record](the-run-record.md) — the failure screenshot and unnamed captures are files in its directory, numbered from its sequence; a change to `screenshot_to` or `capture_path` changes what the record holds.
- [Capturing a window](capturing-a-window.md) — `capture` and `capture --window` share one command and one default path; the window capture is defined as what this one leaves out.
- [Masking secrets](masking-secrets.md) — a capture holds whatever the page shows, secrets included; masking does not reach pixels.
- [Finding elements on the page](finding-elements-on-the-page.md) — an element capture resolves and releases its object like any element command.
- [Speaking CDP](speaking-cdp.md) — a screenshot is the largest message pokit receives; a capture over the message limit closes the page's connection.

## Known holes / open

- Whether the page capture really leaves out a `<select>` list or tooltip has no test.
- A capture of an element larger than the viewport, and of one inside an `<iframe>`, are untested.
- A screenshot whose base64 exceeds 64 MiB would end the page's connection rather than fail the command (read from the code, not measured).
- Capture on macOS is not built. Tracked: #35.

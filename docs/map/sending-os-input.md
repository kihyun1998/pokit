# Sending OS input

How `window activate` brings the launched app to the front, and how `key`, `type` and `click` reach it on `--route os` through `SendInput` on Windows. Code: `os_input::activate`, `os_input::key`, `os_input::text`, `os_input::click`, `os_input::chord_strokes`, `os_input::us_scan`, `os_input::to_screen` (os_input.rs), `State::os_*_cmd`, `State::refuse_unless_front` (session/os_input.rs), `State::window_activate_cmd` (session/native.rs), `support::routes`.

## Design model

- **OS input goes ahead only when it would reach the page pokit is on, and is refused otherwise (exit 6, nothing sent, nothing scrolled or focused).** #1 story 38. Three checks:
  - before anything happens on the page, the app must own the foreground window and the current page must have the input focus (`document.hasFocus()`), which it does not when another of the app's windows or one of its own dialogs is in front;
  - `os_input` checks the foreground window again right before `SendInput`;
  - a click is placed in the app's foreground window, which must hold a page, and the window found at the point (`WindowFromPoint`) must be that window, so a window covering the page cannot take the click.
- **`window activate` is how the app gets in front, and it takes the user's focus.** It tries `SetForegroundWindow`, then the same call while attached to the foreground window's input queue (`AttachThreadInput`), and refuses with exit 6 when Windows keeps the app back. The result names which way worked. **Maintainer's call (2026-10-06, #5):** an explicit command in pokit, not a test-only workaround and not a person bringing the app forward. **Maintainer's call (2026-10-07):** plain, then `AttachThreadInput`, never the synthetic Alt key tao uses. The Alt key would put a keystroke into the user's app, which can open its menu bar.
- **A chord is pressed as a hand presses it, by physical key.** Modifiers go down, then the key goes down and up, then the modifiers come up in reverse (`chord_strokes`), all in one `SendInput` call so that no other input interleaves. A page counting keydowns sees Control as a key of its own here, unlike on the CDP route ([[sending-input-over-cdp]]).
  - Each key is sent as its scan code on a US keyboard (`us_scan`, `KEYEVENTF_SCANCODE`), the same physical key whatever the user's layout: the page's `KeyboardEvent.code` comes from the scan code, and with no scan code `Ctrl+KeyK` arrived as `code ""`.
  - What the key types then follows the user's layout and input method, as a real keyboard's would: on AZERTY `KeyA` types `q`, and with a Korean IME in Hangul mode a letter key arrives as `Process`. The result therefore reports the key's `code`, not a `key` value.
  - The first version sent virtual keys, which `MapVirtualKeyW` mapped through the session thread's layout, and so pressed a different physical key on a non-US layout (independent review, 2026-10-07).
  - If Windows takes only part of a chord, every modifier already pressed is released again, so none is left down for the user.
- **`type` sends each character as Unicode (`KEYEVENTF_UNICODE`), and a newline as the Enter key.** There is no IME composition on this route; `capabilities` says so (`ime_composition: false`), and the CDP route composes ([[sending-input-over-cdp]]).
- **A click lands at the element's centre in physical pixels.** The page's top-left corner is the client origin of the WebView2 render widget (`Chrome_RenderWidgetHostHWND`) inside the app's foreground window. The element's CSS position is multiplied by the page's `devicePixelRatio` and added to that corner (`to_screen`). This is worked out with the thread switched to per-monitor DPI awareness, because the session is otherwise unaware and Windows would scale the coordinates it returns. The mouse moves there and clicks, so the user's cursor moves too.
- **`capabilities` says, per input command, which routes exist, which is the default, and whether each takes focus** (#1 story 42). `window` takes focus.

## Measured

- **Activation (2026-10-07, Windows 11, terminal Orca in front):** all three ways tried (plain, `AttachThreadInput`, synthetic Alt) brought the fixture to the front 11 times out of 11. That held both with the user idle for longer than the foreground lock time-out and while the user was moving the mouse. The foreground window belonged to an Orca process that is not an ancestor of pokit. *Unverified:* Orca may call `AllowSetForegroundWindow(ASFW_ANY)`, which would let any process take the foreground. So the conditions in which the plain call fails were not produced here.
- **At 150% scaling (DPI 144, device pixel ratio 1.5):** an OS click on `#target` produced `click`, a double click `dblclick`, and a right click `contextmenu`.
- **Two clicks closer together than the double-click time (500 ms here) are one sequence**, as a hand's are: a single click followed at once by `--double` made a triple, and the page's last event was `click`.
- **`document.hasFocus()` tells which of the app's pages takes the input when the app is in front (2026-10-07).** With the fixture's second window in front, the main page reported `false` and the second `true`; with the app's own dialog in front, `false`. With the app behind, a page reported `true` after a CDP focus, so the foreground check is still needed.

## Testing

- `os_input_reaches_the_page_as_a_hand_would_send_it` brings its fixture to the front. Its cases run in one test, because two fixtures taking the foreground at once would fail each other.
- The guard test passes while either foreground check holds, so a mutation has to remove both to turn it red (done 2026-10-07). Removing the page-focus check turns the second-window case red; moving the click's check after scrolling turns the scroll assertion red.
- The second-window and dialog cases run in the same test as the rest, after the clicks.
